//! Module intégré « Téléphone » : liaison avec l'appli Horizon Mobile (`docs/PHONE.md`).
//! Logique pure : `horizon_core::phone`. Ici : l'identité TLS du PC, le serveur, et ce qui
//! les relie au reste d'Horizon (registre enregistré, batteries dans la barre, alertes).

//!
//! Rien n'existe tant que le module n'est pas activé : `start` (premier lancement activé) lit
//! `phones.json` et lance le fil `telephone-etat` ; le serveur n'écoute que si le module est
//! activé et que le pare-feu de Windows le permet ([`refresh`]).

pub mod identity;
pub mod server;
pub mod ui;

use crate::alerts::LowBattery;
use crate::host::{AppHandle, Manager};
use crate::util::{Published, Waker, lock, start_thread};
use crate::widgets::Backend;
use horizon_core::phone::battery::{BatteryView, Book};
use horizon_core::phone::firewall::{FirewallState, PORT};
use horizon_core::phone::pairing::{Pairing, qr_payload};
use horizon_core::phone::proto::{FEATURES, FromPhone, PROTOCOL, ToPhone};
use horizon_core::phone::registry::Registry;
use horizon_core::time::now_ms;
use identity::{Identity, random_token};
use serde::{Deserialize, Serialize};
use server::{Handler, ServerHandle};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Batteries reçues des téléphones (`Vec<BatteryView>`).
pub const EV_PHONE: &str = "horizon://phone";
/// État de la liaison, pour la page de réglages ([`LinkView`]).
pub const EV_PHONE_LINK: &str = "horizon://phone-link";

pub const BACKEND: Backend = Backend {
    id: "phone",
    name: "téléphone",
    manage: |app| crate::widgets::manage(app, PhoneState::default()),
    start: Some(start),
    bind: Some(ui::bind),
    ..Backend::NONE
};

/// Accent envoyé au téléphone tant qu'Horizon n'en a pas (ni pochette ni fond d'écran).
const DEFAULT_ACCENT: &str = "#f97316";
/// Longueur maximale de l'identifiant qu'un téléphone se donne.
const MAX_ID: usize = 64;
/// Clé privée du PC, dans le Gestionnaire d'identifiants de Windows.
const KEY_TARGET: &str = "Horizon/phone/key";
/// Registre et dernières batteries, dans le dossier d'Horizon.
const FILE: &str = "phones.json";
/// Un changement est écrit après ce délai sans autre changement.
const SAVE_DELAY: Duration = Duration::from_secs(2);
/// Relecture périodique tant que le module est activé (pare-feu en attente, mesures qui
/// vieillissent).
const RECHECK: Duration = Duration::from_secs(60);

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    registry: Registry,
    batteries: Book,
}

/// Un téléphone appairé, pour la page de réglages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PhoneView {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub last_seen_ms: u64,
}

/// État de la liaison, pour la page de réglages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LinkView {
    /// `allowed`, `missing`, `blocked` ou `unknown`.
    pub firewall: String,
    /// Port écouté ; 0 si le serveur est arrêté.
    pub port: u16,
    pub listening: bool,
    pub fingerprint: String,
    pub phones: Vec<PhoneView>,
    /// Millisecondes restantes de l'offre d'appairage ; 0 sans offre.
    pub pairing_ms: u64,
}

/// Ce que le module sait des téléphones, sans fenêtre ni réseau : c'est ce que les tests
/// exercent.
pub struct Core {
    pc_name: String,
    registry: Mutex<Registry>,
    book: Mutex<Book>,
    pairing: Mutex<Pairing>,
    accent: Mutex<String>,
}

impl Core {
    /// État relu dans le texte de `phones.json` (vide ou illisible : aucun téléphone).
    pub fn restore(text: &str, pc_name: &str) -> Core {
        let mut saved: Saved = serde_json::from_str(text).unwrap_or_default();
        if saved.registry.pc_id.is_empty() {
            saved.registry.pc_id = random_token(12);
        }
        Core {
            pc_name: pc_name.to_owned(),
            registry: Mutex::new(saved.registry),
            book: Mutex::new(saved.batteries),
            pairing: Mutex::default(),
            accent: Mutex::new(DEFAULT_ACCENT.to_owned()),
        }
    }

    #[cfg(test)]
    fn for_tests(pc_id: &str, pc_name: &str) -> Core {
        let core = Core::restore("", pc_name);
        lock(&core.registry).pc_id = pc_id.to_owned();
        core
    }

    /// Texte à écrire dans `phones.json`.
    pub fn snapshot(&self) -> String {
        let saved = Saved { registry: lock(&self.registry).clone(), batteries: lock(&self.book).clone() };
        serde_json::to_string_pretty(&saved).expect("registre sans clé non textuelle")
    }

    /// Ouvre une offre d'appairage pour `token`.
    pub fn pairing_offer(&self, token: String, now_ms: u64) {
        lock(&self.pairing).offer(token, now_ms);
    }

    /// Ouvre une offre d'appairage et rend le contenu de son QR code.
    pub fn offer(&self, addrs: &[Ipv4Addr], host: &str, port: u16, fingerprint: &str, token: String, now_ms: u64) -> Result<String, String> {
        if addrs.is_empty() {
            return Err("Aucun réseau local : branche le PC au réseau, puis réessaie.".to_owned());
        }
        let qr = qr_payload(addrs, host, port, fingerprint, &token);
        self.pairing_offer(token, now_ms);
        Ok(qr)
    }

    pub fn cancel_pairing(&self) {
        lock(&self.pairing).cancel();
    }

    /// Millisecondes restantes de l'offre d'appairage ; 0 sans offre.
    pub fn pending_ms(&self, now_ms: u64) -> u64 {
        lock(&self.pairing).pending(now_ms).unwrap_or(0)
    }

    /// Première trame d'un téléphone : son identifiant et la réponse, ou le refus.
    pub fn first(&self, msg: FromPhone, now_ms: u64) -> Result<(String, ToPhone), ToPhone> {
        match msg {
            FromPhone::PairRequest { token, device_id, device_name, .. } => {
                let refuse = |reason: &str| ToPhone::PairRejected { reason: reason.to_owned() };
                // Avant le jeton : un identifiant inutilisable ne doit pas le consommer.
                if device_id.is_empty() || device_id.chars().count() > MAX_ID {
                    return Err(refuse("unknown"));
                }
                lock(&self.pairing).redeem(&token, now_ms).map_err(|r| refuse(r.reason()))?;
                let device_token = random_token(32);
                let mut registry = lock(&self.registry);
                registry.add(&device_id, &device_name, &device_token, now_ms);
                Ok((device_id, ToPhone::PairAccepted { device_token, pc_id: registry.pc_id.clone(), pc_name: self.pc_name.clone() }))
            }
            FromPhone::Hello { device_id, device_token, protocol, .. } => {
                let refuse = |reason: &str| ToPhone::HelloRejected { reason: reason.to_owned() };
                if protocol != PROTOCOL {
                    return Err(refuse("protocol"));
                }
                let mut registry = lock(&self.registry);
                registry.check(&device_id, &device_token, now_ms).map_err(refuse)?;
                let ok = ToPhone::HelloOk {
                    pc_id: registry.pc_id.clone(),
                    pc_name: self.pc_name.clone(),
                    protocol: PROTOCOL,
                    features: FEATURES.iter().map(|f| (*f).to_owned()).collect(),
                    accent: lock(&self.accent).clone(),
                };
                Ok((device_id, ok))
            }
            _ => Err(ToPhone::PairRejected { reason: "unknown".to_owned() }),
        }
    }

    /// Message d'un téléphone dont la session est ouverte ; vrai si les batteries ont changé.
    pub fn message(&self, phone_id: &str, msg: FromPhone, now_ms: u64) -> bool {
        match msg {
            FromPhone::Battery { devices } => lock(&self.book).apply(phone_id, devices, now_ms),
            _ => false,
        }
    }

    /// Retire un téléphone : son jeton ne vaut plus rien, ses batteries sont oubliées.
    pub fn remove(&self, phone_id: &str) -> bool {
        let known = lock(&self.registry).remove(phone_id);
        lock(&self.book).forget(phone_id) || known
    }

    /// Batteries à montrer.
    pub fn view(&self, now_ms: u64) -> Vec<BatteryView> {
        lock(&self.book).view(now_ms)
    }

    /// Téléphones appairés ; `connected` : ceux dont la session est ouverte.
    pub fn phones(&self, connected: &[String], _now_ms: u64) -> Vec<PhoneView> {
        lock(&self.registry)
            .phones
            .iter()
            .map(|p| PhoneView { id: p.id.clone(), name: p.name.clone(), connected: connected.contains(&p.id), last_seen_ms: p.last_seen_ms })
            .collect()
    }

    /// Nouvel accent de la barre ; vrai s'il a changé.
    pub fn set_accent(&self, color: String) -> bool {
        let mut accent = lock(&self.accent);
        let changed = *accent != color;
        *accent = color;
        changed
    }
}

/// Ce qui vient d'arriver à une session, pour que le module mette l'interface à jour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    /// Un téléphone vient d'être appairé, ou a rouvert sa session.
    Opened,
    Batteries,
    Closed,
}

/// Relie le serveur à [`Core`] ; `notify` prévient le reste d'Horizon.
struct Link<F> {
    core: Arc<Core>,
    notify: F,
}

impl<F: Fn(Event) + Send + Sync + 'static> Handler for Link<F> {
    fn first(&self, msg: FromPhone, peer: IpAddr) -> Result<(String, ToPhone), ToPhone> {
        let result = self.core.first(msg, now_ms());
        match &result {
            Ok((id, _)) => {
                log::info!("téléphone : session ouverte pour {id} depuis {peer}");
                (self.notify)(Event::Opened);
            }
            Err(refusal) => log::warn!("téléphone : {peer} refusé ({})", refusal.to_json()),
        }
        result
    }

    fn message(&self, phone_id: &str, msg: FromPhone) {
        if self.core.message(phone_id, msg, now_ms()) {
            (self.notify)(Event::Batteries);
        }
    }

    fn closed(&self, phone_id: &str) {
        log::info!("téléphone : session de {phone_id} fermée");
        (self.notify)(Event::Closed);
    }
}

/// Le serveur, en marche ou non.
#[derive(Default)]
struct Runner {
    server: Mutex<Option<ServerHandle>>,
}

impl Runner {
    /// Met le serveur en marche (`make` le crée s'il ne l'est pas déjà) ou l'arrête. L'arrêt
    /// se fait verrou relâché : les sessions qui se ferment préviennent le module, qui relit
    /// cet état.
    fn set(&self, on: bool, make: impl FnOnce() -> anyhow::Result<ServerHandle>) -> anyhow::Result<()> {
        let stopped = {
            let mut server = lock(&self.server);
            match (on, server.is_some()) {
                (true, false) => {
                    *server = Some(make()?);
                    None
                }
                (false, true) => server.take(),
                _ => None,
            }
        };
        if let Some(server) = stopped {
            server.stop();
        }
        Ok(())
    }

    fn port(&self) -> Option<u16> {
        lock(&self.server).as_ref().map(ServerHandle::port)
    }

    fn connected(&self) -> Vec<String> {
        lock(&self.server).as_ref().map(ServerHandle::connected).unwrap_or_default()
    }

    fn send_all(&self, msg: &ToPhone) {
        if let Some(server) = lock(&self.server).as_ref() {
            server.send_all(msg);
        }
    }

    fn disconnect(&self, phone_id: &str) {
        if let Some(server) = lock(&self.server).as_ref() {
            server.disconnect(phone_id);
        }
    }
}

pub struct PhoneState {
    /// Posé par [`start`] : absent tant que le module n'a jamais été activé.
    core: OnceLock<Arc<Core>>,
    runner: Runner,
    identity: Mutex<Option<Arc<Identity>>>,
    firewall: Mutex<FirewallState>,
    shown: Published<Vec<BatteryView>>,
    link: Published<LinkView>,
    /// Une alerte « batterie faible » par appareil (clé de [`BatteryView`]).
    low: Mutex<HashMap<String, LowBattery>>,
    /// Le registre ou les batteries ont changé depuis la dernière écriture.
    dirty: AtomicBool,
    wake: Waker,
}

impl Default for PhoneState {
    fn default() -> Self {
        PhoneState {
            core: OnceLock::new(),
            runner: Runner::default(),
            identity: Mutex::default(),
            firewall: Mutex::new(FirewallState::Unknown),
            shown: Published::new(EV_PHONE, Vec::new()),
            link: Published::new(EV_PHONE_LINK, LinkView::default()),
            low: Mutex::default(),
            dirty: AtomicBool::new(false),
            wake: Waker::default(),
        }
    }
}

fn file() -> PathBuf {
    crate::state::app_dir().join(FILE)
}

fn exe() -> PathBuf {
    std::env::current_exe().unwrap_or_default()
}

fn core(app: &AppHandle) -> Option<Arc<Core>> {
    app.state::<PhoneState>().core.get().cloned()
}

/// Un téléphone est appairé d'après le fichier (avant tout démarrage : le module « sert déjà »).
pub fn paired_on_disk() -> bool {
    std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str::<Saved>(&t).ok()).is_some_and(|s| !s.registry.phones.is_empty())
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(file()).unwrap_or_default();
    let core = Arc::new(Core::restore(&text, &horizon_sys::local_net::host_name()));
    core.set_accent(crate::accent::accent_state(app).map_or_else(|| DEFAULT_ACCENT.to_owned(), crate::plugins::events::hex));
    let _ = app.state::<PhoneState>().core.set(core);
    let a = app.clone();
    start_thread("telephone-etat", move || watch(&a))?;
    refresh(app);
    Ok(())
}

/// Écrit ce qui attend, puis tient l'état à jour tant que le module est activé ; désactivé, le
/// fil dort jusqu'à ce qu'on le réveille.
fn watch(app: &AppHandle) {
    loop {
        horizon_sys::presence::wait_while_away();
        let on = crate::modules::enabled("phone");
        let state = app.state::<PhoneState>();
        if state.dirty.swap(false, Ordering::SeqCst) {
            save(app);
        }
        if on {
            // Le pare-feu n'est relu que tant qu'il nous manque : une fois la règle posée, seules
            // les mesures vieillissent.
            if *lock(&state.firewall) == FirewallState::Allowed { publish(app) } else { refresh(app) }
        }
        let woken = state.wake.wait(if on { RECHECK } else { Duration::from_secs(u32::MAX.into()) });
        if woken {
            // Plusieurs changements de suite (une rafale d'envois) : une seule écriture.
            std::thread::sleep(SAVE_DELAY);
        }
    }
}

fn save(app: &AppHandle) {
    let Some(core) = core(app) else { return };
    let (path, tmp) = (file(), file().with_extension("json.tmp"));
    let written = std::fs::create_dir_all(crate::state::app_dir()).and_then(|()| std::fs::write(&tmp, core.snapshot())).and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        log::error!("téléphone : {} non enregistré : {e}", path.display());
    }
}

/// Identité TLS du PC, créée au premier besoin et gardée par Windows.
fn identity(state: &PhoneState) -> anyhow::Result<Arc<Identity>> {
    let mut slot = lock(&state.identity);
    if let Some(identity) = slot.as_ref() {
        return Ok(identity.clone());
    }
    let identity = Arc::new(Identity::load_or_create(
        || horizon_sys::credentials::read(KEY_TARGET),
        |pem| {
            if let Err(e) = horizon_sys::credentials::write(KEY_TARGET, pem) {
                log::error!("téléphone : clé du PC non enregistrée, les téléphones devront être appairés de nouveau au prochain lancement : {e:#}");
            }
        },
    )?);
    *slot = Some(identity.clone());
    Ok(identity)
}

/// Met le serveur dans l'état voulu (module activé et pare-feu qui le permet : en marche ;
/// sinon arrêté) et l'interface à jour. À appeler quand le module change d'état.
pub fn refresh(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get().cloned() else { return };
    let on = crate::modules::enabled("phone");
    let firewall = if on { horizon_sys::firewall::state(&exe(), PORT) } else { FirewallState::Unknown };
    *lock(&state.firewall) = firewall;
    let want = on && matches!(firewall, FirewallState::Allowed | FirewallState::Unknown);
    let started = state.runner.set(want, || {
        let identity = identity(&state)?;
        let (core, a) = (core.clone(), app.clone());
        server::spawn(Ipv4Addr::UNSPECIFIED.into(), PORT, identity.tls_config(), Arc::new(Link { core, notify: move |event| on_event(&a, event) }))
    });
    match started {
        Ok(()) if want => log::info!("téléphone : écoute sur le port {}", state.runner.port().unwrap_or(0)),
        Ok(()) => {}
        Err(e) => log::error!("téléphone : serveur indisponible : {e:#}"),
    }
    if !on {
        core.cancel_pairing();
        lock(&state.low).clear();
    }
    publish(app);
}

/// Envoie aux fenêtres les batteries et l'état de la liaison (seuls les changements partent).
fn publish(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get() else { return };
    let (now, on) = (now_ms(), crate::modules::enabled("phone"));
    state.shown.set(app, if on { core.view(now) } else { Vec::new() });
    let port = state.runner.port();
    let firewall = match *lock(&state.firewall) {
        FirewallState::Allowed => "allowed",
        FirewallState::Missing => "missing",
        FirewallState::Blocked => "blocked",
        FirewallState::Unknown => "unknown",
    };
    let view = LinkView {
        firewall: firewall.to_owned(),
        port: port.unwrap_or(0),
        listening: port.is_some(),
        fingerprint: lock(&state.identity).as_ref().map(|i| i.fingerprint()).unwrap_or_default(),
        phones: core.phones(&state.runner.connected(), now),
        pairing_ms: core.pending_ms(now),
    };
    state.link.set(app, view);
}

/// Événement d'une session (fil de son téléphone).
fn on_event(app: &AppHandle, event: Event) {
    let state = app.state::<PhoneState>();
    if event == Event::Batteries {
        alert_low(app);
    }
    if event != Event::Closed {
        state.dirty.store(true, Ordering::SeqCst);
        state.wake.wake();
    }
    publish(app);
}

/// « Batterie faible » pour chaque appareil dont la mesure est récente.
fn alert_low(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get() else { return };
    let threshold = crate::state::settings().alerts.phone_threshold();
    let view = core.view(now_ms());
    let mut low = lock(&state.low);
    low.retain(|key, _| view.iter().any(|v| &v.key == key));
    for v in view.iter().filter(|v| !v.stale) {
        low.entry(v.key.clone()).or_insert_with(|| LowBattery::new(v.icon)).update_with(app, &v.name, v.percent, v.charging, threshold);
    }
}

/// Batteries montrées par les barres.
pub fn batteries(app: &AppHandle) -> Vec<BatteryView> {
    app.state::<PhoneState>().shown.get()
}

/// État de la liaison (page de réglages).
pub fn link(app: &AppHandle) -> LinkView {
    app.state::<PhoneState>().link.get()
}

/// Ouvre une offre d'appairage et rend le contenu du QR code à afficher.
pub fn offer_pairing(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<PhoneState>();
    let core = state.core.get().ok_or("Active d'abord le module Téléphone.")?;
    let port = state.runner.port().ok_or("Horizon n'écoute pas encore : autorise-le d'abord sur le réseau local.")?;
    let fingerprint = identity(&state).map_err(|e| format!("{e:#}"))?.fingerprint();
    let qr = core.offer(&horizon_sys::local_net::local_ipv4(), &horizon_sys::local_net::host_name(), port, &fingerprint, random_token(16), now_ms())?;
    publish(app);
    Ok(qr)
}

pub fn cancel_pairing(app: &AppHandle) {
    if let Some(core) = core(app) {
        core.cancel_pairing();
        publish(app);
    }
}

/// Retire un téléphone appairé et ferme sa session.
pub fn remove_phone(app: &AppHandle, id: &str) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get() else { return };
    if core.remove(id) {
        state.runner.disconnect(id);
        state.dirty.store(true, Ordering::SeqCst);
        state.wake.wake();
        publish(app);
    }
}

/// Pose la règle de pare-feu d'Horizon : Windows demande une autorisation d'administrateur.
/// Bloque jusqu'à la réponse : jamais sur le fil de l'interface.
pub fn allow_firewall(app: &AppHandle) -> Result<(), String> {
    let after = horizon_sys::firewall::allow_elevated(&exe()).map_err(|e| format!("{e:#}"))?;
    refresh(app);
    match after {
        FirewallState::Allowed | FirewallState::Unknown => Ok(()),
        _ => Err("Le pare-feu de Windows bloque toujours Horizon.".to_owned()),
    }
}

/// L'accent de la barre a changé : les téléphones connectés le reçoivent.
pub fn accent_changed(app: &AppHandle, color: Option<[u8; 3]>) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get() else { return };
    let color = color.map_or_else(|| DEFAULT_ACCENT.to_owned(), crate::plugins::events::hex);
    if core.set_accent(color.clone()) {
        state.runner.send_all(&ToPhone::Accent { color });
    }
}

/// Ce que la page État dit du module.
pub struct Health {
    /// Port écouté, si le serveur est en marche.
    pub port: Option<u16>,
    pub firewall: FirewallState,
    pub phones: usize,
    pub connected: usize,
    /// Dernière mesure reçue.
    pub last_ms: Option<u64>,
}

pub fn health(app: &AppHandle) -> Health {
    let state = app.state::<PhoneState>();
    let link = state.link.get();
    Health {
        port: state.runner.port(),
        firewall: *lock(&state.firewall),
        phones: link.phones.len(),
        connected: link.phones.iter().filter(|p| p.connected).count(),
        last_ms: state.shown.get().iter().map(|v| v.at_ms).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::phone::proto::{DeviceBattery, DeviceKind, FromPhone, ToPhone};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn pair_request(token: &str, id: &str, name: &str) -> FromPhone {
        FromPhone::PairRequest { token: token.into(), device_id: id.into(), device_name: name.into(), app_version: "0.1.0".into() }
    }

    fn hello(id: &str, token: &str) -> FromPhone {
        FromPhone::Hello { device_id: id.into(), device_token: token.into(), protocol: 1, app_version: "0.1.0".into() }
    }

    fn battery(percent: u8, charging: bool) -> FromPhone {
        let phone = DeviceBattery { id: "phone".into(), kind: DeviceKind::Phone, name: "Pixel".into(), percent, charging, at_ms: 10 };
        FromPhone::Battery { devices: vec![phone] }
    }

    /// Appaire `id` et rend son jeton permanent.
    fn pair(core: &Core, id: &str) -> String {
        core.pairing_offer("jeton".into(), 0);
        let Ok((_, ToPhone::PairAccepted { device_token, .. })) = core.first(pair_request("jeton", id, "Pixel"), 1) else { panic!("appairage refusé") };
        device_token
    }

    #[test]
    fn the_handler_pairs_recognises_and_refuses() {
        let core = Core::for_tests("pc-1", "Bureau");
        core.pairing_offer("jeton".into(), 0);
        let (id, reply) = core.first(pair_request("jeton", "tel-1", "Pixel"), 1).unwrap();
        let ToPhone::PairAccepted { device_token, pc_id, pc_name } = reply else { panic!() };
        assert_eq!((id.as_str(), pc_id.as_str(), pc_name.as_str()), ("tel-1", "pc-1", "Bureau"));
        assert_eq!(device_token.len(), 43, "32 octets aléatoires");
        let ok = core.first(hello("tel-1", &device_token), 2);
        assert!(matches!(&ok, Ok((id, ToPhone::HelloOk { protocol: 1, features, accent, .. })) if id == "tel-1" && features == &["battery"] && accent == "#f97316"), "{ok:?}");
        assert!(matches!(core.first(hello("tel-1", "faux"), 3), Err(ToPhone::HelloRejected { reason }) if reason == "bad_token"));
        let newer = FromPhone::Hello { device_id: "tel-1".into(), device_token, protocol: 2, app_version: "9".into() };
        assert!(matches!(core.first(newer, 3), Err(ToPhone::HelloRejected { reason }) if reason == "protocol"));
        assert!(matches!(core.first(pair_request("jeton", "tel-2", "x"), 4), Err(ToPhone::PairRejected { reason }) if reason == "used"));
        assert!(core.first(battery(50, false), 5).is_err(), "une batterie avant hello : refus");
    }

    #[test]
    fn a_phone_without_a_usable_identifier_is_refused_before_the_token_is_spent() {
        let core = Core::for_tests("pc-1", "Bureau");
        core.pairing_offer("jeton".into(), 0);
        assert!(matches!(core.first(pair_request("jeton", "", "x"), 1), Err(ToPhone::PairRejected { reason }) if reason == "unknown"));
        assert!(matches!(core.first(pair_request("jeton", &"i".repeat(65), "x"), 1), Err(ToPhone::PairRejected { reason }) if reason == "unknown"));
        assert!(core.first(pair_request("jeton", "tel-1", "x"), 1).is_ok(), "le jeton n'a pas été consommé par les refus");
    }

    #[test]
    fn no_local_address_means_no_qr() {
        let core = Core::for_tests("pc-1", "Bureau");
        assert_eq!(core.offer(&[], "BUREAU", 28680, "fp", "tok".into(), 0), Err("Aucun réseau local : branche le PC au réseau, puis réessaie.".to_owned()));
        let qr = core.offer(&["192.168.1.49".parse().unwrap()], "BUREAU", 28680, "fp", "tok".into(), 0).unwrap();
        assert_eq!(qr, "horizon://pair?v=1&h=192.168.1.49&n=BUREAU&p=28680&fp=fp&t=tok");
        assert!(core.first(pair_request("tok", "tel-1", "Pixel"), 1).is_ok(), "le jeton du QR est celui qui appaire");
    }

    #[test]
    fn removing_a_phone_forgets_its_batteries_and_its_token() {
        let core = Core::for_tests("pc-1", "Bureau");
        let token = pair(&core, "tel-1");
        assert!(core.message("tel-1", battery(82, true), 10));
        assert!(!core.message("tel-1", battery(82, true), 11), "même mesure : rien de neuf");
        assert_eq!(core.view(10).len(), 1);
        assert!(core.remove("tel-1"));
        assert!(core.view(10).is_empty());
        assert!(matches!(core.first(hello("tel-1", &token), 11), Err(ToPhone::HelloRejected { reason }) if reason == "unknown_device"));
    }

    #[test]
    fn what_is_saved_comes_back_without_any_token() {
        let core = Core::for_tests("pc-1", "Bureau");
        let token = pair(&core, "tel-1");
        core.message("tel-1", battery(64, false), 10);
        let saved = core.snapshot();
        assert!(!saved.contains(&token), "le jeton n'est jamais écrit");
        let back = Core::restore(&saved, "Bureau");
        assert_eq!(back.view(10), core.view(10));
        assert!(back.first(hello("tel-1", &token), 12).is_ok(), "le téléphone est reconnu après un redémarrage");
        assert_eq!(back.phones(&[], 12).len(), 1);
        assert!(Core::restore("{{{", "Bureau").view(0).is_empty());
        assert!(!Core::restore("", "Bureau").snapshot().contains("\"pc_id\": \"\""), "un identifiant de PC est créé");
    }

    #[test]
    fn turning_the_module_off_closes_phones_and_frees_the_port_then_it_comes_back() {
        let core = Arc::new(Core::for_tests("pc-1", "Bureau"));
        let identity = identity::Identity::load_or_create(|| None, |_| {}).unwrap();
        let runner = Runner::default();
        let make = |port: u16| {
            let (core, tls) = (core.clone(), identity.tls_config());
            move || server::spawn("127.0.0.1".parse().unwrap(), port, tls, Arc::new(Link { core, notify: |_| {} }))
        };
        runner.set(true, make(0)).unwrap();
        let port = runner.port().unwrap();
        core.pairing_offer("jeton".into(), horizon_core::time::now_ms());
        let link = format!("horizon://pair?v=1&h=127.0.0.1&n=Bureau&p={port}&fp={}&t=jeton", identity.fingerprint());
        let mut phone = faux_telephone::Client::pair(&link, "Pixel").unwrap();
        phone.send_battery(82, true).unwrap();
        let start = Instant::now();
        while core.view(horizon_core::time::now_ms()).is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5), "batterie jamais reçue");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(runner.connected(), vec![phone.device_id.clone()]);

        runner.set(false, make(port)).unwrap();
        assert!(phone.receive().is_err(), "session fermée par la désactivation");
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok(), "port rendu");
        assert_eq!(runner.port(), None);

        runner.set(true, make(port)).unwrap();
        runner.set(true, || panic!("déjà en marche : rien à relancer")).unwrap();
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], runner.port().unwrap()));
        assert!(faux_telephone::Client::hello(addr, &identity.fingerprint(), &phone.device_id, &phone.device_token).is_ok(), "il revient avec son jeton");
    }

    #[test]
    fn the_link_view_says_who_is_there() {
        let core = Core::for_tests("pc-1", "Bureau");
        pair(&core, "tel-1");
        let phones = core.phones(&["tel-1".to_owned()], 5);
        assert_eq!(phones, vec![PhoneView { id: "tel-1".into(), name: "Pixel".into(), connected: true, last_seen_ms: 1 }]);
        assert!(!core.phones(&[], 5)[0].connected);
    }
}
