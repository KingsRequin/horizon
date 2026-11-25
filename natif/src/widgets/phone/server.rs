//! Serveur du module « Téléphone » : TLS puis WebSocket, un fil d'écoute et un fil par
//! téléphone connecté. Il ne connaît ni le registre ni l'interface : il passe les messages à
//! un [`Handler`].
//!
//! Rien ne tourne à vide : l'écoute dort dans `accept`, et un fil de téléphone ne se réveille
//! qu'à l'arrivée d'un message ou, une fois par seconde, pour envoyer ce qui attend et voir
//! s'il doit s'arrêter.

use crate::util::lock;
use anyhow::Context;
use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::phone::pairing::is_private;
use horizon_core::phone::proto::{self, FromPhone, ToPhone};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

pub use horizon_core::phone::firewall::PORT_TRIES;

/// Délai accordé à un client, en tout, pour la poignée de main et sa première trame.
pub const FIRST_MESSAGE: Duration = Duration::from_secs(10);
/// Taille maximale d'une trame ; au-delà, la connexion est fermée.
pub const MAX_FRAME: usize = 64 * 1024;
/// Téléphones connectés en même temps.
pub const MAX_CLIENTS: usize = 4;
/// Connexions qui n'ont pas encore ouvert de session ; au-delà, la plus ancienne cède sa place.
pub const MAX_PENDING: usize = 8;
/// Réveil d'un fil de téléphone sans message entrant (envois en attente, arrêt).
const POLL: Duration = Duration::from_secs(1);
/// Silence après lequel Windows sonde une connexion, et écart entre deux sondes.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(120);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// Réveil d'une connexion en attente (au plus [`FIRST_MESSAGE`]) pour voir si elle est abandonnée.
const PENDING_POLL: Duration = Duration::from_millis(250);
/// Un envoi qui ne part pas (téléphone disparu sans fermer) : la connexion est abandonnée.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Attente des fils de téléphone à l'arrêt.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// Ce que le serveur demande au module pour chaque téléphone.
pub trait Handler: Send + Sync + 'static {
    /// Première trame (`pair.request` ou `hello`) venue de `peer` : l'identifiant du téléphone
    /// et la réponse à lui envoyer, ou le refus (la connexion est alors fermée).
    fn first(&self, msg: FromPhone, peer: IpAddr) -> Result<(String, ToPhone), ToPhone>;
    /// Message d'un téléphone dont la session est ouverte.
    fn message(&self, phone_id: &str, msg: FromPhone);
    /// La session du téléphone est finie.
    fn closed(&self, phone_id: &str);
}

enum Out {
    Message(ToPhone),
    Close,
}

struct Client {
    n: u64,
    /// La connexion a sa place parmi les sessions ([`MAX_CLIENTS`]) ; avant, elle n'est
    /// qu'en attente ([`MAX_PENDING`]).
    admitted: bool,
    /// Connu une fois la session ouverte.
    phone_id: Option<String>,
    out: Sender<Out>,
    /// Levé pour abandonner une connexion en attente (évincée, ou arrêt du serveur) : son fil
    /// le voit dans la demi-seconde. Fermer la connexion depuis un autre fil ne suffirait pas,
    /// Windows ne réveille pas une lecture en cours.
    cancel: Arc<AtomicBool>,
}

impl Client {
    fn cut(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

/// Connexion dont les lectures, tant que la session n'est pas ouverte, sont bornées par une
/// échéance globale (un client qui envoie un octet de temps en temps ne garde pas sa place)
/// et abandonnées dès que `cancel` est levé.
struct Guarded {
    stream: TcpStream,
    deadline: Option<Instant>,
    cancel: Arc<AtomicBool>,
}

impl std::io::Read for Guarded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(deadline) = self.deadline else { return self.stream.read(buf) };
        loop {
            if self.cancel.load(Ordering::SeqCst) {
                return Err(ErrorKind::ConnectionAborted.into());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left.min(PENDING_POLL)))?;
            match self.stream.read(buf) {
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                other => return other,
            }
        }
    }
}

impl std::io::Write for Guarded {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    clients: Mutex<Vec<Client>>,
    next: AtomicU64,
}

/// Le serveur en marche. Le lâcher l'arrête.
pub struct ServerHandle {
    port: u16,
    wake: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

/// Écoute sur `bind`, au premier port libre à partir de `first_port` ([`PORT_TRIES`] essais).
pub fn spawn(bind: IpAddr, first_port: u16, tls: Arc<ServerConfig>, handler: Arc<dyn Handler>) -> anyhow::Result<ServerHandle> {
    let listener = (0..PORT_TRIES)
        .filter_map(|i| first_port.checked_add(i))
        .find_map(|port| TcpListener::bind((bind, port)).ok())
        .with_context(|| format!("aucun port libre de {first_port} à {}", first_port.saturating_add(PORT_TRIES - 1)))?;
    let port = listener.local_addr().context("port d'écoute")?.port();
    let wake = SocketAddr::new(if bind.is_unspecified() { IpAddr::V4(Ipv4Addr::LOCALHOST) } else { bind }, port);
    let shared = Arc::new(Shared::default());
    let s = shared.clone();
    let accept = std::thread::Builder::new()
        .name("telephone".into())
        .spawn(move || accept_loop(&listener, &tls, &handler, &s))
        .context("fil d'écoute du module Téléphone")?;
    Ok(ServerHandle { port, wake, shared, accept: Some(accept) })
}

impl ServerHandle {
    /// Port réellement écouté.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Envoie `msg` à tous les téléphones dont la session est ouverte (au plus tard à leur
    /// prochain réveil, dans la seconde).
    pub fn send_all(&self, msg: &ToPhone) {
        for client in lock(&self.shared.clients).iter().filter(|c| c.phone_id.is_some()) {
            let _ = client.out.send(Out::Message(msg.clone()));
        }
    }

    /// Ferme la session du téléphone `phone_id` (il vient d'être retiré).
    pub fn disconnect(&self, phone_id: &str) {
        for client in lock(&self.shared.clients).iter().filter(|c| c.phone_id.as_deref() == Some(phone_id)) {
            let _ = client.out.send(Out::Close);
        }
    }

    /// Téléphones dont la session est ouverte.
    pub fn connected(&self) -> Vec<String> {
        lock(&self.shared.clients).iter().filter_map(|c| c.phone_id.clone()).collect()
    }

    /// Ferme l'écoute et toutes les sessions, et attend la fin de leurs fils : au retour, le
    /// port est libre.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let Some(accept) = self.accept.take() else { return };
        self.shared.stop.store(true, Ordering::SeqCst);
        for client in lock(&self.shared.clients).iter() {
            // Une session ouverte se ferme proprement à son prochain réveil ; une connexion
            // en attente est bloquée dans sa poignée de main : elle est coupée.
            if client.admitted {
                let _ = client.out.send(Out::Close);
            } else {
                client.cut();
            }
        }
        // Le fil d'écoute dort dans `accept` : une connexion à soi-même le réveille.
        let _ = TcpStream::connect_timeout(&self.wake, Duration::from_secs(1));
        let _ = accept.join();
        let start = Instant::now();
        while !lock(&self.shared.clients).is_empty() && start.elapsed() < STOP_TIMEOUT {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = lock(&self.shared.clients).len();
        if left > 0 {
            log::warn!("téléphone : {left} connexion(s) encore ouverte(s) {STOP_TIMEOUT:?} après l'arrêt");
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn accept_loop(listener: &TcpListener, tls: &Arc<ServerConfig>, handler: &Arc<dyn Handler>, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else {
            // Erreur d'écoute passagère (plus de ressources) : ne pas tourner à vide.
            std::thread::sleep(Duration::from_millis(200));
            continue;
        };
        let Ok(peer) = stream.peer_addr().map(|a| a.ip()) else { continue };
        if !is_private(peer) {
            log::warn!("téléphone : connexion refusée, {peer} n'est pas une adresse du réseau local");
            continue;
        }
        keep_alive(&stream);
        let (out, inbox) = unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let n = shared.next.fetch_add(1, Ordering::Relaxed);
        {
            let mut clients = lock(&shared.clients);
            // Réserve des connexions en attente pleine : la plus ancienne cède sa place.
            if clients.iter().filter(|c| !c.admitted).count() >= MAX_PENDING
                && let Some(oldest) = clients.iter().position(|c| !c.admitted)
            {
                clients.remove(oldest).cut();
            }
            clients.push(Client { n, admitted: false, phone_id: None, out, cancel: cancel.clone() });
        }
        let (tls, handler, s) = (tls.clone(), handler.clone(), shared.clone());
        let spawned = std::thread::Builder::new().name(format!("telephone-{n}")).spawn(move || {
            let phone_id = serve(stream, peer, tls, handler.as_ref(), &s, n, &inbox, cancel);
            // `closed` d'abord : quand la liste est vide, tout a été signalé (`shutdown`).
            if let Some(id) = phone_id {
                handler.closed(&id);
            }
            lock(&s.clients).retain(|c| c.n != n);
        });
        if spawned.is_err() {
            lock(&shared.clients).retain(|c| c.n != n);
        }
    }
}

/// Un téléphone qui sort du Wi-Fi ne ferme pas sa connexion : sans rien, sa session resterait
/// « Connecté » pour toujours. Windows sonde donc la connexion muette (après 2 min, puis toutes
/// les 10 s, dix fois) et la lecture échoue quand plus rien ne répond. Les sondes se règlent
/// entre les deux systèmes : l'appli n'est pas réveillée, contrairement à un ping applicatif.
fn keep_alive(stream: &TcpStream) {
    let probes = socket2::TcpKeepalive::new().with_time(KEEPALIVE_IDLE).with_interval(KEEPALIVE_INTERVAL);
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&probes) {
        log::warn!("téléphone : surveillance de la connexion impossible : {e}");
    }
}

type Socket = WebSocket<StreamOwned<ServerConnection, Guarded>>;

fn timed_out(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut))
}

/// Sert un téléphone jusqu'à la fin de sa session ; rend son identifiant si elle a été ouverte.
#[allow(clippy::too_many_arguments)]
fn serve(
    stream: TcpStream,
    peer: IpAddr,
    tls: Arc<ServerConfig>,
    handler: &dyn Handler,
    shared: &Shared,
    n: u64,
    inbox: &Receiver<Out>,
    cancel: Arc<AtomicBool>,
) -> Option<String> {
    let mut ws = match open(stream, tls, cancel) {
        Ok(ws) => ws,
        Err(e) => {
            log::debug!("téléphone : {peer} n'a pas ouvert de session : {e:#}");
            return None;
        }
    };
    let first = match ws.read() {
        Ok(Message::Text(text)) => proto::parse(&text),
        _ => None,
    };
    let Some(first) = first else {
        let _ = ws.close(None);
        return None;
    };
    // La place est prise avant de consulter le module : un jeton d'appairage n'est pas
    // consommé pour une session qui serait refusée faute de place.
    {
        let mut clients = lock(&shared.clients);
        let full = clients.iter().filter(|c| c.admitted).count() >= MAX_CLIENTS;
        match clients.iter_mut().find(|c| c.n == n) {
            Some(me) if !full => me.admitted = true,
            // Plus de place, ou connexion évincée entre-temps.
            _ => {
                log::warn!("téléphone : session de {peer} refusée, {MAX_CLIENTS} sessions déjà ouvertes");
                drop(clients);
                let _ = ws.close(None);
                return None;
            }
        }
    }
    let phone_id = match handler.first(first, peer) {
        Ok((id, reply)) => {
            if ws.send(Message::text(reply.to_json())).is_err() {
                return None;
            }
            id
        }
        Err(refusal) => {
            let _ = ws.send(Message::text(refusal.to_json()));
            let _ = ws.close(None);
            return None;
        }
    };
    {
        // Le même téléphone encore connu d'une session morte : elle cède la place.
        let mut clients = lock(&shared.clients);
        for old in clients.iter().filter(|c| c.n != n && c.phone_id.as_deref() == Some(phone_id.as_str())) {
            let _ = old.out.send(Out::Close);
        }
        if let Some(me) = clients.iter_mut().find(|c| c.n == n) {
            me.phone_id = Some(phone_id.clone());
        }
    }
    // Session ouverte : plus d'échéance, mais un réveil périodique.
    ws.get_mut().sock.deadline = None;
    let _ = ws.get_ref().sock.stream.set_read_timeout(Some(POLL));
    'session: loop {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        for out in inbox.try_iter() {
            match out {
                Out::Message(msg) if ws.send(Message::text(msg.to_json())).is_ok() => {}
                _ => break 'session,
            }
        }
        match ws.read() {
            Ok(Message::Text(text)) => match proto::parse(&text) {
                Some(FromPhone::Unknown) | None => {}
                Some(msg) => handler.message(&phone_id, msg),
            },
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(e) if timed_out(&e) => {}
            Err(_) => break,
        }
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    Some(phone_id)
}

/// Poignée de main TLS puis WebSocket. Elles et la première trame doivent tenir, en tout,
/// dans [`FIRST_MESSAGE`] à compter de maintenant.
fn open(stream: TcpStream, tls: Arc<ServerConfig>, cancel: Arc<AtomicBool>) -> anyhow::Result<Socket> {
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let guarded = Guarded { stream, deadline: Some(Instant::now() + FIRST_MESSAGE), cancel };
    let tls = StreamOwned::new(ServerConnection::new(tls).context("session TLS")?, guarded);
    let config = WebSocketConfig::default().max_message_size(Some(MAX_FRAME)).max_frame_size(Some(MAX_FRAME));
    tungstenite::accept_with_config(tls, Some(config)).map_err(|e| anyhow::anyhow!("ouverture WebSocket : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accepted_connection_watches_for_a_vanished_phone() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        assert!(!socket2::SockRef::from(&stream).keepalive().unwrap());
        keep_alive(&stream);
        assert!(socket2::SockRef::from(&stream).keepalive().unwrap());
    }
    use crate::widgets::phone::identity::Identity;
    use horizon_core::phone::proto::{FromPhone, ToPhone};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Seen {
        batteries: Mutex<Vec<(String, u8, bool)>>,
        closed: Mutex<Vec<String>>,
    }

    impl Seen {
        fn batteries(&self) -> Vec<(String, u8, bool)> {
            self.batteries.lock().unwrap().clone()
        }
    }

    struct TestHandler(Arc<Seen>);

    impl Handler for TestHandler {
        fn first(&self, msg: FromPhone, _peer: std::net::IpAddr) -> Result<(String, ToPhone), ToPhone> {
            match msg {
                // « Pixel » est le téléphone des tests à un seul appareil ; tout autre nom fait un
                // téléphone distinct.
                FromPhone::PairRequest { token, device_name, .. } if token == "jeton-ok" => {
                    let id = if device_name == "Pixel" { "tel-test".to_owned() } else { device_name };
                    Ok((id, ToPhone::PairAccepted { device_token: "jeton-permanent".into(), pc_id: "pc-test".into(), pc_name: "Test".into() }))
                }
                FromPhone::Hello { device_token, .. } if device_token == "jeton-permanent" => Ok((
                    "tel-test".into(),
                    ToPhone::HelloOk { pc_id: "pc-test".into(), pc_name: "Test".into(), protocol: 1, features: vec!["battery".into()], granted: vec!["battery".into()], accent: "#f97316".into() },
                )),
                FromPhone::Hello { .. } => Err(ToPhone::HelloRejected { reason: "bad_token".into() }),
                _ => Err(ToPhone::PairRejected { reason: "unknown".into() }),
            }
        }

        fn message(&self, phone_id: &str, msg: FromPhone) {
            if let FromPhone::Battery { devices } = msg {
                for d in devices {
                    self.0.batteries.lock().unwrap().push((phone_id.to_owned(), d.percent, d.charging));
                }
            }
        }

        fn closed(&self, phone_id: &str) {
            self.0.closed.lock().unwrap().push(phone_id.to_owned());
        }
    }

    fn test_identity() -> Identity {
        Identity::load_or_create(|| None, |_| {}).unwrap()
    }

    fn test_server() -> (ServerHandle, Arc<Seen>, String) {
        let (seen, id) = (Arc::new(Seen::default()), test_identity());
        let server = spawn("127.0.0.1".parse().unwrap(), 0, id.tls_config(), Arc::new(TestHandler(seen.clone()))).unwrap();
        (server, seen, id.fingerprint())
    }

    fn qr(server: &ServerHandle, fingerprint: &str, token: &str) -> String {
        format!("horizon://pair?v=1&h=127.0.0.1&n=Test&p={}&fp={fingerprint}&t={token}", server.port())
    }

    fn wait_for(what: &str, cond: impl Fn() -> bool) {
        let start = Instant::now();
        while !cond() {
            assert!(start.elapsed() < Duration::from_secs(5), "toujours pas : {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_phone_pairs_then_reports_its_battery() {
        let (server, seen, fp) = test_server();
        let mut phone = faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").unwrap();
        assert_eq!(phone.device_token, "jeton-permanent");
        phone.send_battery(82, true).unwrap();
        wait_for("batterie reçue", || seen.batteries() == vec![("tel-test".to_owned(), 82, true)]);
        drop(phone);
        wait_for("fermeture signalée", || seen.closed.lock().unwrap().as_slice() == ["tel-test"]);
        server.stop();
    }

    #[test]
    fn a_paired_phone_comes_back_with_its_token() {
        let (server, seen, fp) = test_server();
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], server.port()));
        let mut phone = faux_telephone::Client::hello(addr, &fp, "tel-test", "jeton-permanent").unwrap();
        phone.send_battery(40, false).unwrap();
        wait_for("batterie reçue", || seen.batteries() == vec![("tel-test".to_owned(), 40, false)]);
        let refused = faux_telephone::Client::hello(addr, &fp, "tel-test", "faux").unwrap_err();
        assert!(refused.to_string().contains("bad_token"), "{refused}");
        let wrong = faux_telephone::Client::pair(&qr(&server, &fp, "mauvais"), "Pixel").unwrap_err();
        assert!(wrong.to_string().contains("unknown"), "{wrong}");
        server.stop();
    }

    #[test]
    fn garbage_and_oversized_frames_do_not_bring_the_server_down() {
        let (server, seen, fp) = test_server();
        let mut phone = faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").unwrap();
        phone.send_raw("pas du json").unwrap();
        phone.send_raw(r#"{"type":"futur.message"}"#).unwrap();
        phone.send_battery(55, false).unwrap();
        wait_for("le message valide passe après les autres", || seen.batteries() == vec![("tel-test".to_owned(), 55, false)]);
        assert!(phone.send_raw(&"x".repeat(5 * 1024 * 1024)).is_err() || phone.receive().is_err(), "trame trop grosse : connexion fermée");
        let mut again = faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").unwrap();
        again.send_battery(40, false).unwrap();
        wait_for("le serveur sert toujours", || seen.batteries().last() == Some(&("tel-test".to_owned(), 40, false)));
        server.stop();
    }

    #[test]
    fn a_wrong_fingerprint_or_a_silent_client_is_refused() {
        let (server, _seen, fp) = test_server();
        assert!(faux_telephone::Client::pair(&qr(&server, &"A".repeat(43), "jeton-ok"), "Pixel").is_err());
        let silent = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        silent.set_read_timeout(Some(FIRST_MESSAGE + Duration::from_secs(3))).unwrap();
        let start = Instant::now();
        assert_eq!(std::io::Read::read(&mut &silent, &mut [0u8; 1]).unwrap_or(0), 0, "fermée par le serveur");
        assert!(start.elapsed() < FIRST_MESSAGE + Duration::from_secs(2), "au bout de {:?}", start.elapsed());
        assert!(faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").is_ok(), "le serveur sert toujours");
        server.stop();
    }

    #[test]
    fn a_busy_port_moves_to_the_next_one_and_stop_frees_it() {
        let id = test_identity();
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let first = taken.local_addr().unwrap().port();
        let server = spawn("127.0.0.1".parse().unwrap(), first, id.tls_config(), Arc::new(TestHandler(Arc::default()))).unwrap();
        assert!(server.port() > first && server.port() < first + PORT_TRIES, "{} après {first}", server.port());
        let port = server.port();
        server.stop();
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "port rendu");
    }

    #[test]
    fn messages_reach_every_phone_and_stop_closes_them() {
        let (server, seen, fp) = test_server();
        let mut phone = faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").unwrap();
        server.send_all(&ToPhone::Accent { color: "#a06cd5".into() });
        assert_eq!(phone.receive().unwrap(), r##"{"type":"accent","color":"#a06cd5"}"##);
        server.stop();
        assert!(phone.receive().is_err(), "connexion fermée par l'arrêt");
        assert_eq!(seen.closed.lock().unwrap().as_slice(), ["tel-test"]);
    }

    #[test]
    fn a_fifth_phone_waits_for_a_free_place() {
        let (server, _seen, fp) = test_server();
        let mut phones: Vec<_> = (0..MAX_CLIENTS).map(|i| faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), &format!("tel-{i}")).unwrap()).collect();
        assert!(faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "tel-de-trop").is_err(), "pas de cinquième");
        assert_eq!(server.connected().len(), MAX_CLIENTS, "les quatre premiers gardent leur session");
        phones.pop();
        wait_for("une place se libère", || faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "tel-suivant").is_ok());
        server.stop();
    }

    #[test]
    fn a_dripping_client_is_cut_at_the_deadline() {
        use std::io::{ErrorKind, Read, Write};
        let (server, _seen, fp) = test_server();
        let mut slow = std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap();
        slow.set_read_timeout(Some(Duration::from_millis(900))).unwrap();
        // Début d'un enregistrement TLS valide (poignée de main, 512 octets annoncés), puis un
        // octet par seconde : chaque lecture du serveur aboutit, sans jamais finir.
        slow.write_all(&[0x16, 0x03, 0x01, 0x02, 0x00]).unwrap();
        let start = Instant::now();
        let closed = loop {
            if start.elapsed() > FIRST_MESSAGE + Duration::from_secs(4) {
                break false;
            }
            if slow.write_all(&[0]).is_err() {
                break true;
            }
            match slow.read(&mut [0u8; 64]) {
                Ok(0) => break true,
                Ok(_) => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break true,
            }
        };
        assert!(closed, "connexion gardée au-delà de l'échéance");
        assert!(start.elapsed() < FIRST_MESSAGE + Duration::from_secs(3), "coupée au bout de {:?}", start.elapsed());
        assert!(faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), "Pixel").is_ok(), "le serveur sert toujours");
        server.stop();
    }

    #[test]
    fn silent_connections_cannot_take_the_places_of_phones() {
        let (server, seen, fp) = test_server();
        let silent: Vec<_> = (0..MAX_PENDING).map(|_| std::net::TcpStream::connect(("127.0.0.1", server.port())).unwrap()).collect();
        let mut phones: Vec<_> = (0..MAX_CLIENTS).map(|i| faux_telephone::Client::pair(&qr(&server, &fp, "jeton-ok"), &format!("tel-{i}")).unwrap()).collect();
        phones[0].send_battery(61, false).unwrap();
        wait_for("batterie reçue malgré les connexions muettes", || seen.batteries() == vec![("tel-0".to_owned(), 61, false)]);
        silent[0].set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        assert_eq!(std::io::Read::read(&mut &silent[0], &mut [0u8; 1]).unwrap_or(0), 0, "la plus ancienne connexion muette a cédé sa place");
        let start = Instant::now();
        server.stop();
        assert!(start.elapsed() < Duration::from_secs(2), "l'arrêt n'attend pas les connexions muettes ({:?})", start.elapsed());
    }
}
