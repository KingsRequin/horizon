//! Entrées/sorties des scripts, chacune sur son fil : connexion WebSocket locale (reconnexion
//! automatique, arrêt quand le script ferme ou disparaît) et requête HTTPS (sans redirection :
//! l'adresse vérifiée est la seule jointe). Les résultats reviennent au fil des scripts
//! (`ToJs::Io`).

use super::{Io, ToJs};
use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::backoff::Backoff;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Taille maximale d'un message reçu, d'une réponse HTTP, d'un message envoyé.
pub const MAX_MESSAGE: usize = 1024 * 1024;
pub const MAX_RESPONSE: u64 = 2 * 1024 * 1024;
/// Délai d'une requête HTTP.
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Connexion WebSocket d'un script : envois, arrêt.
pub struct Conn {
    pub outbox: Sender<String>,
    stop: Arc<AtomicBool>,
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Ouvre (et rouvre au besoin) une connexion vers `url` ; ses événements arrivent au fil des
/// scripts. S'arrête quand la [`Conn`] est lâchée.
pub fn connect(id: &str, handle: u32, url: String, back: Sender<ToJs>) -> Conn {
    let (outbox, rx) = unbounded::<String>();
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    let id = id.to_owned();
    crate::util::spawn_named(&format!("plugin-ws-{id}"), move || {
        let send = |io: Io| back.send(ToJs::Io { id: id.clone(), handle, io }).is_ok();
        let mut backoff = Backoff::new();
        while !s.load(Ordering::SeqCst) {
            // Aucune redirection : l'adresse vérifiée par les permissions est la seule jointe.
            if let Ok((mut ws, _)) = tungstenite::client::connect_with_config(url.as_str(), None, 0) {
                let opened = Instant::now();
                if let MaybeTlsStream::Plain(t) = ws.get_ref() {
                    let _ = t.set_read_timeout(Some(Duration::from_millis(100)));
                }
                while rx.try_recv().is_ok() {}
                if !send(Io::WsOpen) {
                    return;
                }
                session(&mut ws, &rx, &s, &send);
                let _ = ws.close(None);
                if !send(Io::WsClosed) {
                    return;
                }
                backoff.connection_closed(opened.elapsed());
            }
            // Attente coupée en petits pas pour s'arrêter vite.
            let end = Instant::now() + backoff.next_delay();
            while Instant::now() < end && !s.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    });
    Conn { outbox, stop }
}

fn session(ws: &mut WebSocket<MaybeTlsStream<std::net::TcpStream>>, rx: &Receiver<String>, stop: &AtomicBool, send: &dyn Fn(Io) -> bool) {
    while !stop.load(Ordering::SeqCst) {
        match ws.read() {
            Ok(Message::Text(t)) if t.len() <= MAX_MESSAGE => {
                if !send(Io::WsText(t.to_string())) {
                    return;
                }
            }
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return,
        }
        while let Ok(m) = rx.try_recv() {
            if ws.send(Message::text(m)).is_err() {
                return;
            }
        }
    }
}

/// Requête HTTPS (`GET` sans corps, `POST` avec) ; la réponse revient au fil des scripts.
pub fn request(id: &str, handle: u32, url: String, body: Option<String>, headers: Vec<(String, String)>, back: Sender<ToJs>) {
    let id = id.to_owned();
    crate::util::spawn_named(&format!("plugin-http-{id}"), move || {
        let result = call(&url, body, &headers);
        let _ = back.send(ToJs::Io { id, handle, io: Io::Http(result) });
    });
}

fn call(url: &str, body: Option<String>, headers: &[(String, String)]) -> Result<(u16, String), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(HTTP_TIMEOUT))
        .build()
        .into();
    let ua = concat!("Horizon/", env!("CARGO_PKG_VERSION"));
    let response = match body {
        None => {
            let mut r = agent.get(url).header("User-Agent", ua);
            for (k, v) in headers {
                r = r.header(k.as_str(), v.as_str());
            }
            r.call()
        }
        Some(b) => {
            let mut r = agent.post(url).header("User-Agent", ua);
            for (k, v) in headers {
                r = r.header(k.as_str(), v.as_str());
            }
            r.send(b)
        }
    };
    let mut response = response.map_err(|_| "serveur injoignable".to_owned())?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_string()
        .map_err(|_| "réponse illisible ou trop grosse".to_owned())?;
    Ok((status, text))
}
