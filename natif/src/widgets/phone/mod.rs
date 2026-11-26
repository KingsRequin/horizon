//! Module intégré « Téléphone » : liaison avec l'appli Horizon Mobile (`docs/PHONE.md`).
//! Logique pure : `horizon_core::phone`. Ici : l'identité TLS du PC, le serveur, et ce qui
//! les relie au reste d'Horizon (registre enregistré, batteries dans la barre, alertes).

//!
//! Rien n'existe tant que le module n'est pas activé : `start` (premier lancement activé) lit
//! `phones.json` et lance le fil `telephone-etat` ; le serveur n'écoute que si le module est
//! activé et que le pare-feu de Windows le permet ([`refresh`]).

pub mod identity;
pub mod remote;
pub mod server;
pub mod ui;

use crate::alerts::LowBattery;
use crate::host::{AppHandle, Emitter, Manager};
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
    /// Ce qu'il a le droit de faire (`battery`, `media`, `sound`, `input`).
    pub granted: Vec<String>,
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
    /// Appelé dès que la liste des téléphones change (voir [`Core::on_registry_change`]).
    registry_changed: OnceLock<RegistryHook>,
}

/// Ce qui est fait quand la liste des téléphones change (écriture de `phones.json`).
type RegistryHook = Box<dyn Fn(&Core) + Send + Sync>;

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
            registry_changed: OnceLock::new(),
        }
    }

    /// `f` est appelé, sur le fil qui la provoque, à chaque arrivée ou retrait d'un téléphone :
    /// c'est là que le registre s'écrit sur le disque. Un retrait qui attendrait l'écriture
    /// groupée des mesures pourrait ne jamais être écrit (session verrouillée, puis arrêt), et
    /// le téléphone retiré serait de nouveau accepté au lancement suivant.
    pub fn on_registry_change(&self, f: impl Fn(&Core) + Send + Sync + 'static) {
        let _ = self.registry_changed.set(Box::new(f));
    }

    fn registry_did_change(&self) {
        if let Some(f) = self.registry_changed.get() {
            f(self);
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
    // Le refus a la taille de la réponse (`hello.ok`) et ne sert qu'une fois par connexion :
    // l'emballer n'apporterait rien.
    #[allow(clippy::result_large_err)]
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
                let pc_id = {
                    let mut registry = lock(&self.registry);
                    registry.add(&device_id, &device_name, &device_token, now_ms);
                    registry.pc_id.clone()
                };
                self.registry_did_change();
                Ok((device_id, ToPhone::PairAccepted { device_token, pc_id, pc_name: self.pc_name.clone() }))
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
                    granted: registry.granted(&device_id),
                    accent: lock(&self.accent).clone(),
                };
                Ok((device_id, ok))
            }
            _ => Err(ToPhone::PairRejected { reason: "unknown".to_owned() }),
        }
    }

    /// Message d'un téléphone dont la session est ouverte ; vrai si les batteries ont changé.
    pub fn message(&self, phone_id: &str, msg: FromPhone, now_ms: u64) -> bool {
        // Un téléphone retiré dont la session vit encore un instant ne recrée rien.
        if !lock(&self.registry).phones.iter().any(|p| p.id == phone_id) {
            return false;
        }
        match msg {
            FromPhone::Battery { devices } => lock(&self.book).apply(phone_id, devices, now_ms),
            _ => false,
        }
    }

    /// Droits du téléphone `phone_id` (vide s'il n'est pas connu).
    pub fn granted(&self, phone_id: &str) -> Vec<String> {
        lock(&self.registry).granted(phone_id)
    }

    /// Accorde ou retire un droit ; la nouvelle liste s'il a changé (elle est aussitôt écrite).
    pub fn set_right(&self, phone_id: &str, right: &str, on: bool) -> Option<Vec<String>> {
        let granted = {
            let mut registry = lock(&self.registry);
            registry.set_granted(phone_id, right, on).then(|| registry.granted(phone_id))
        }?;
        self.registry_did_change();
        Some(granted)
    }

    /// Retire un téléphone : son jeton ne vaut plus rien, ses batteries sont oubliées.
    pub fn remove(&self, phone_id: &str) -> bool {
        let known = lock(&self.registry).remove(phone_id);
        let forgotten = lock(&self.book).forget(phone_id);
        if known {
            self.registry_did_change();
        }
        forgotten || known
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
            .map(|p| PhoneView { id: p.id.clone(), name: p.name.clone(), connected: connected.contains(&p.id), last_seen_ms: p.last_seen_ms, granted: p.rights() })
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

/// Relie le serveur à [`Core`] et à la télécommande ; `notify` prévient le reste d'Horizon.
struct Link<F> {
    core: Arc<Core>,
    remote: Arc<remote::Remote>,
    notify: F,
}

impl<F: Fn(Event) + Send + Sync + 'static> Handler for Link<F> {
    fn first(&self, msg: FromPhone, peer: IpAddr) -> Result<(String, ToPhone), ToPhone> {
        let result = self.core.first(msg, now_ms());
        match &result {
            Ok((id, _)) => {
                log::info!("téléphone : session ouverte pour {id} depuis {peer}");
                // Session neuve : le téléphone redemande ce qu'il regarde et en reçoit l'état, même
                // si sa session morte n'a pas encore été vue.
                self.remote.closed(id);
                (self.notify)(Event::Opened);
            }
            // Pas un avertissement : n'importe quel appareil du réseau peut en provoquer à volonté.
            Err(refusal) => log::debug!("téléphone : {peer} refusé ({})", refusal.to_json()),
        }
        result
    }

    fn message(&self, phone_id: &str, msg: FromPhone) {
        let now = now_ms();
        if self.remote.message(phone_id, &msg, now) {
            return;
        }
        if self.core.message(phone_id, msg, now) {
            (self.notify)(Event::Batteries);
        }
    }

    fn poll(&self, phone_id: &str) -> Duration {
        if self.remote.watching(phone_id) { server::PROMPT } else { server::POLL }
    }

    fn closed(&self, phone_id: &str) {
        log::info!("téléphone : session de {phone_id} fermée");
        self.remote.closed(phone_id);
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

    fn send_to(&self, phone_id: &str, msg: &ToPhone) {
        if let Some(server) = lock(&self.server).as_ref() {
            server.send_to(phone_id, msg);
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
    /// Télécommande des téléphones, posée par [`start`].
    remote: OnceLock<Arc<remote::Remote>>,
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
            remote: OnceLock::new(),
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
    core.on_registry_change(write);
    let registry = core.clone();
    let _ = app.state::<PhoneState>().core.set(core);
    let a = app.clone();
    let send = move |id: &str, msg: &ToPhone| a.state::<PhoneState>().runner.send_to(id, msg);
    let rights = move |id: &str| registry.granted(id);
    let _ = app.state::<PhoneState>().remote.set(Arc::new(remote::Remote::new(Box::new(AppControls(app.clone())), Box::new(send), Box::new(rights))));
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
    if let Some(core) = core(app) {
        write(&core);
    }
}

/// Écrit `phones.json`. Une écriture à la fois, l'état lu sous le verrou : la dernière écrite
/// est toujours la plus récente (fil de l'état, fil d'une session, fil de l'interface).
fn write(core: &Core) {
    static WRITING: Mutex<()> = Mutex::new(());
    let _one_at_a_time = lock(&WRITING);
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
        let remote = state.remote.get().cloned().ok_or_else(|| anyhow::anyhow!("télécommande pas encore prête"))?;
        server::spawn(Ipv4Addr::UNSPECIFIED.into(), PORT, identity.tls_config(), Arc::new(Link { core, remote, notify: move |event| on_event(&a, event) }))
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

/// Millisecondes restantes de l'offre d'appairage (0 : aucune), à l'instant.
pub fn pairing_left(app: &AppHandle) -> u64 {
    core(app).map_or(0, |core| core.pending_ms(now_ms()))
}

/// Le seuil d'alerte a changé : les barres recolorent leurs batteries (les mesures, elles,
/// n'ont pas changé : `publish` ne renverrait rien).
pub fn threshold_changed(app: &AppHandle) {
    let _ = app.emit(EV_PHONE, batteries(app));
}

pub fn cancel_pairing(app: &AppHandle) {
    if let Some(core) = core(app) {
        core.cancel_pairing();
        publish(app);
    }
}

/// Accorde ou retire un droit à un téléphone (page Téléphone) : enregistré tout de suite, et
/// le téléphone l'apprend (`rights`) s'il est relié.
pub fn set_right(app: &AppHandle, id: &str, right: &str, on: bool) {
    let state = app.state::<PhoneState>();
    let (Some(core), Some(remote)) = (state.core.get(), state.remote.get()) else { return };
    if let Some(granted) = core.set_right(id, right, on) {
        log::info!("téléphone : droit {right} {} pour {id}", if on { "accordé" } else { "retiré" });
        remote.rights_changed(id, &granted, now_ms());
        publish(app);
    }
}

/// La lecture en cours a changé (fil du média) ; `art` : ce que la relecture dit de la
/// pochette. Ne fait rien tant que le module n'a pas démarré ou que personne ne regarde.
pub fn media_changed(app: &AppHandle, art: remote::ArtChange) {
    if let Some(remote) = app.state::<PhoneState>().remote.get() {
        remote.media_changed(&AppControls(app.clone()).media_now(), art, now_ms());
    }
}

/// Le mixeur a publié un état (volume général, applis, sorties) : il part vers les téléphones
/// qui le regardent. Rien n'est calculé si personne ne regarde.
pub fn mixer_changed(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    let Some(remote) = state.remote.get() else { return };
    if !remote.wants_volume_or_mixer() {
        return;
    }
    let snapshot = crate::mixer::view(app).snapshot;
    remote.volume_changed(snapshot.master.map(|l| (l.volume, l.muted)));
    remote.mixer_changed(Some(&snapshot));
}

/// Ne pas déranger, le minuteur ou le casque ont peut-être changé : l'état des actions part
/// vers les téléphones qui le regardent (seulement s'il a changé).
pub fn actions_changed(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    let Some(remote) = state.remote.get() else { return };
    if remote.wants_actions() {
        remote.actions_changed(&AppControls(app.clone()).actions_now(), now_ms());
    }
}

/// Volume général et muet de la sortie par défaut.
fn master(app: &AppHandle) -> Option<(f32, bool)> {
    crate::mixer::view(app).snapshot.master.map(|l| (l.volume, l.muted))
}

/// Les lectures du média (identifiant, titre en cours) nommées d'après leur appli, comme dans
/// le panneau du média : c'est le nom du lecteur que le téléphone affiche.
fn named(sessions: Vec<(String, String)>) -> Vec<(String, String)> {
    sessions.into_iter().map(|(id, _)| {
        let name = crate::ui::format::app_name(&id);
        (id, name)
    }).collect()
}

/// La télécommande branchée sur Horizon : lecture en cours, mixeur, actions rapides.
struct AppControls(AppHandle);

impl AppControls {
    fn media_now(&self) -> remote::MediaNow {
        let state = crate::widgets::media::media_state(&self.0);
        remote::MediaNow { info: state.payload.info, sessions: named(state.payload.sessions) }
    }

    fn actions_now(&self) -> remote::ActionsNow {
        let timer = crate::timer::timer_state(&self.0);
        let headset = crate::widgets::headset::current(&self.0).map(|h| horizon_core::phone::proto::HeadsetView { name: h.name, level: h.level, charging: h.charging });
        remote::ActionsNow { dnd: crate::dnd::dnd_state(&self.0).active, timer: timer.timer, presets: timer.presets, headset }
    }

    fn report(what: &str, result: Result<(), String>) {
        if let Err(e) = result {
            log::warn!("téléphone : {what} : {e}");
        }
    }
}

impl remote::Controls for AppControls {
    fn play_pause(&self) {
        Self::report("lecture", crate::widgets::media::media_play_pause(&self.0));
    }
    fn next(&self) {
        Self::report("suivant", crate::widgets::media::media_next(&self.0));
    }
    fn previous(&self) {
        Self::report("précédent", crate::widgets::media::media_prev(&self.0));
    }
    fn seek_relative(&self, ms: i64) {
        Self::report("position", crate::widgets::media::media_seek_relative(&self.0, ms));
    }
    fn select(&self, app_id: &str) {
        Self::report("choix du lecteur", crate::widgets::media::media_select_session(&self.0, app_id.to_owned()));
    }
    fn set_volume(&self, volume: f32) {
        Self::report("volume", crate::mixer::set_master(&self.0, volume));
    }
    fn set_mute(&self, muted: bool) {
        Self::report("muet", crate::mixer::set_master_mute(&self.0, muted));
    }
    fn media(&self) -> remote::MediaNow {
        self.media_now()
    }
    fn volume(&self) -> Option<(f32, bool)> {
        master(&self.0)
    }
    fn watch_mixer(&self, on: bool) {
        crate::mixer::watch(&self.0, on);
    }
    fn mixer(&self) -> Option<horizon_core::mixer::MixerSnapshot> {
        Some(crate::mixer::view(&self.0).snapshot)
    }
    fn actions(&self) -> remote::ActionsNow {
        self.actions_now()
    }
    fn set_app_volume(&self, key: &str, volume: f32) {
        Self::report("volume d'une appli", crate::mixer::set_app_volume(&self.0, key.to_owned(), volume));
    }
    fn set_app_mute(&self, key: &str, muted: bool) {
        Self::report("muet d'une appli", crate::mixer::set_app_mute(&self.0, key.to_owned(), muted));
    }
    fn set_output(&self, id: &str) {
        Self::report("sortie audio", crate::mixer::set_default(&self.0, id.to_owned()));
    }
    fn lock_pc(&self) {
        if let Err(e) = horizon_sys::power::lock_workstation() {
            log::warn!("téléphone : {e:#}");
        }
    }
    fn set_dnd(&self, on: bool) {
        if crate::dnd::dnd_state(&self.0).active != on {
            crate::dnd::dnd_toggle(&self.0);
        }
    }
    fn timer_start(&self, minutes: u32) {
        crate::timer::timer_start(&self.0, minutes);
    }
    fn timer_pause(&self, paused: bool) {
        crate::timer::timer_pause(&self.0, paused);
    }
    fn timer_stop(&self) {
        crate::timer::timer_stop(&self.0);
    }
}

/// Retire un téléphone appairé et ferme sa session.
pub fn remove_phone(app: &AppHandle, id: &str) {
    let state = app.state::<PhoneState>();
    let Some(core) = state.core.get() else { return };
    if core.remove(id) {
        state.runner.disconnect(id);
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
        assert!(matches!(&ok, Ok((id, ToPhone::HelloOk { protocol: 1, features, accent, .. })) if id == "tel-1" && features == &["battery", "media", "mixer", "actions"] && accent == "#f97316"), "{ok:?}");
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
    fn a_removed_phone_cannot_leave_batteries_behind() {
        let core = Core::for_tests("pc-1", "Bureau");
        pair(&core, "tel-1");
        assert!(core.remove("tel-1"));
        // Sa session vit encore un instant : une mesure qui arrive là ne recrée rien.
        assert!(!core.message("tel-1", battery(82, true), 10));
        assert!(core.view(10).is_empty());
        assert!(!core.message("jamais-vu", battery(50, false), 10));
        assert!(core.view(10).is_empty());
    }

    #[test]
    fn pairing_and_removal_are_saved_at_once() {
        let core = Core::for_tests("pc-1", "Bureau");
        let saved = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = saved.clone();
        core.on_registry_change(move |c| lock(&s).push(c.snapshot()));
        pair(&core, "tel-1");
        assert_eq!(lock(&saved).len(), 1, "un appairage est écrit sans attendre");
        assert!(lock(&saved)[0].contains("tel-1"));
        assert!(core.message("tel-1", battery(82, true), 10));
        assert_eq!(lock(&saved).len(), 1, "une mesure attend l'écriture groupée");
        assert!(core.remove("tel-1"));
        assert_eq!(lock(&saved).len(), 2, "un retrait est écrit sans attendre");
        assert!(!lock(&saved)[1].contains("tel-1"), "le téléphone retiré n'est plus sur le disque");
        assert!(!core.remove("tel-1"));
        assert_eq!(lock(&saved).len(), 2, "rien à écrire quand rien n'a changé");
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
            move || server::spawn("127.0.0.1".parse().unwrap(), port, tls, Arc::new(Link { core, remote: quiet_remote(), notify: |_| {} }))
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

    /// Contrôle à la main avec l'appli dans un émulateur Android, qui joint le PC par 10.0.2.2
    /// (boucle locale : ni pare-feu ni Wi-Fi en jeu, tout le reste est le vrai module) :
    /// `cargo test -p horizon-natif serve_an_emulated_phone_by_hand -- --ignored --nocapture`.
    /// Écrit le lien d'appairage à ouvrir dans l'émulateur, puis ce qui arrive, pendant
    /// `HORIZON_ESSAI_S` secondes (600). Les lignes ajoutées au fichier `HORIZON_ESSAI_CMD`
    /// sont exécutées : `accent #a06cd5`, `retirer`, `offre` (nouveau code), `fin`.
    #[test]
    #[ignore = "sert le téléphone d'un émulateur pendant quelques minutes"]
    fn serve_an_emulated_phone_by_hand() {
        let core = Arc::new(Core::for_tests("pc-essai", "PC essai"));
        let identity = identity::Identity::load_or_create(|| None, |_| {}).unwrap();
        let notify = |event| println!("événement : {event:?}");
        let server = server::spawn("127.0.0.1".parse().unwrap(), 0, identity.tls_config(), Arc::new(Link { core: core.clone(), remote: quiet_remote(), notify })).unwrap();
        let offer = |token: &str| {
            core.pairing_offer(token.to_owned(), horizon_core::time::now_ms());
            println!("LIEN horizon://pair?v=1&h=10.0.2.2&n=PC%20essai&p={}&fp={}&t={token}", server.port(), identity.fingerprint());
        };
        offer("jeton-essai-1");
        let commands = std::env::var("HORIZON_ESSAI_CMD").unwrap_or_default();
        let seconds = std::env::var("HORIZON_ESSAI_S").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
        let (start, mut done, mut shown) = (Instant::now(), 0, String::new());
        'run: while start.elapsed() < Duration::from_secs(seconds) {
            let lines: Vec<String> = std::fs::read_to_string(&commands).unwrap_or_default().lines().map(str::to_owned).collect();
            for line in lines.iter().skip(done) {
                println!("commande : {line}");
                match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                    ["accent", color] => {
                        core.set_accent((*color).to_owned());
                        server.send_all(&ToPhone::Accent { color: (*color).to_owned() });
                    }
                    ["retirer"] => {
                        for phone in core.phones(&[], 0) {
                            core.remove(&phone.id);
                            server.disconnect(&phone.id);
                        }
                    }
                    ["offre"] => offer(&format!("jeton-essai-{}", done + 2)),
                    ["fin"] => break 'run,
                    _ => println!("commande inconnue"),
                }
            }
            done = lines.len();
            let now = horizon_core::time::now_ms();
            let state = format!("connectés : {:?} ; batteries : {:?}", server.connected(), core.view(now).iter().map(|v| (v.name.clone(), v.percent, v.charging)).collect::<Vec<_>>());
            if state != shown {
                println!("{state}");
                shown = state;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        println!("fin de l'essai");
    }

    /// Commandes reçues par la télécommande, et lecture servie (essais par le réseau).
    #[derive(Default)]
    struct Recorded {
        calls: Mutex<Vec<String>>,
    }

    impl remote::Controls for Arc<Recorded> {
        fn play_pause(&self) {
            lock(&self.calls).push("play_pause".into());
        }
        fn next(&self) {
            lock(&self.calls).push("next".into());
        }
        fn previous(&self) {}
        fn seek_relative(&self, _ms: i64) {}
        fn select(&self, _app_id: &str) {}
        fn set_volume(&self, _volume: f32) {}
        fn set_mute(&self, _muted: bool) {}
        fn media(&self) -> remote::MediaNow {
            let info = horizon_core::media::MediaInfo { app_id: "Spotify.exe".into(), title: "FROGZ".into(), artist: "SPLATA".into(), playing: true, position_ms: 0, duration_ms: 233_000, timeline_at_ms: horizon_core::time::now_ms() };
            remote::MediaNow { info: Some(info), sessions: vec![("Spotify.exe".into(), "Spotify".into())] }
        }
        fn volume(&self) -> Option<(f32, bool)> {
            None
        }
        fn watch_mixer(&self, _on: bool) {}
        fn mixer(&self) -> Option<horizon_core::mixer::MixerSnapshot> {
            None
        }
        fn actions(&self) -> remote::ActionsNow {
            remote::ActionsNow::default()
        }
        fn set_app_volume(&self, _key: &str, _volume: f32) {}
        fn set_app_mute(&self, _key: &str, _muted: bool) {}
        fn set_output(&self, _id: &str) {}
        fn lock_pc(&self) {}
        fn set_dnd(&self, _on: bool) {}
        fn timer_start(&self, _minutes: u32) {}
        fn timer_pause(&self, _paused: bool) {}
        fn timer_stop(&self) {}
    }

    /// Une télécommande qui ne commande rien ni n'envoie rien.
    fn quiet_remote() -> Arc<remote::Remote> {
        Arc::new(remote::Remote::new(Box::new(Arc::new(Recorded::default())), Box::new(|_: &str, _: &ToPhone| {}), Box::new(|_: &str| Vec::new())))
    }

    #[test]
    fn hello_carries_the_rights_of_the_phone() {
        let core = Core::for_tests("pc-1", "Bureau");
        let token = pair(&core, "tel-1");
        assert_eq!(core.set_right("tel-1", "media", false), Some(vec!["battery".to_owned(), "sound".to_owned()]));
        assert_eq!(core.set_right("tel-1", "media", false), None, "rien de changé");
        assert_eq!(core.set_right("inconnu", "media", true), None);
        let Ok((_, ToPhone::HelloOk { features, granted, .. })) = core.first(hello("tel-1", &token), 2) else { panic!() };
        assert_eq!(features, vec!["battery", "media", "mixer", "actions"]);
        assert_eq!(granted, vec!["battery", "sound"]);
        assert_eq!(core.phones(&[], 0)[0].granted, vec!["battery", "sound"]);
    }

    #[test]
    fn a_right_change_is_saved_at_once() {
        let core = Core::for_tests("pc-1", "Bureau");
        pair(&core, "tel-1");
        let saved = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = saved.clone();
        core.on_registry_change(move |c| lock(&s).push(c.snapshot()));
        core.set_right("tel-1", "input", true);
        assert_eq!(lock(&saved).len(), 1);
        assert!(lock(&saved)[0].contains("\"input\""));
        core.set_right("tel-1", "input", true);
        assert_eq!(lock(&saved).len(), 1, "rien à écrire");
    }

    /// Le vrai serveur, le vrai `Core` et une télécommande branchée sur de fausses commandes ;
    /// un téléphone appairé prêt à ouvrir des sessions.
    struct Bench {
        recorded: Arc<Recorded>,
        remote: Arc<remote::Remote>,
        slot: Arc<Mutex<Option<ServerHandle>>>,
        addr: std::net::SocketAddr,
        fingerprint: String,
        paired: faux_telephone::Client,
    }

    impl Bench {
        fn new() -> Bench {
            let core = Arc::new(Core::for_tests("pc-1", "Bureau"));
            let identity = identity::Identity::load_or_create(|| None, |_| {}).unwrap();
            let recorded = Arc::new(Recorded::default());
            let slot: Arc<Mutex<Option<ServerHandle>>> = Arc::default();
            let to = slot.clone();
            let c = core.clone();
            let remote = Arc::new(remote::Remote::new(
                Box::new(recorded.clone()),
                Box::new(move |id: &str, msg: &ToPhone| {
                    if let Some(server) = lock(&to).as_ref() {
                        server.send_to(id, msg);
                    }
                }),
                Box::new(move |id: &str| c.granted(id)),
            ));
            let link = Link { core: core.clone(), remote: remote.clone(), notify: |_| {} };
            let server = server::spawn("127.0.0.1".parse().unwrap(), 0, identity.tls_config(), Arc::new(link)).unwrap();
            let port = server.port();
            *lock(&slot) = Some(server);
            core.pairing_offer("jeton".into(), horizon_core::time::now_ms());
            let link = format!("horizon://pair?v=1&h=127.0.0.1&n=Bureau&p={port}&fp={}&t=jeton", identity.fingerprint());
            let paired = faux_telephone::Client::pair(&link, "Pixel").unwrap();
            Bench { recorded, remote, slot, addr: std::net::SocketAddr::from(([127, 0, 0, 1], port)), fingerprint: identity.fingerprint(), paired }
        }

        /// Une session de ce téléphone, abonnée à la lecture ; son premier état est lu.
        fn watching(&self) -> faux_telephone::Client {
            let mut phone = faux_telephone::Client::hello(self.addr, &self.fingerprint, &self.paired.device_id, &self.paired.device_token).unwrap();
            phone.send_raw(r#"{"type":"watch","topics":["media"]}"#).unwrap();
            let state = phone.receive().unwrap();
            assert!(state.contains("\"media.state\"") && state.contains("FROGZ"), "{state}");
            phone
        }

        /// Le PC change de morceau.
        fn new_track(&self, title: &str) {
            let mut now = remote::Controls::media(&self.recorded);
            if let Some(info) = now.info.as_mut() {
                info.title = title.to_owned();
            }
            self.remote.media_changed(&now, remote::ArtChange::Unchanged, horizon_core::time::now_ms());
        }
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            if let Some(server) = lock(&self.slot).take() {
                server.stop();
            }
        }
    }

    #[test]
    fn a_phone_watching_media_gets_its_state_over_the_network() {
        let bench = Bench::new();
        let mut phone = bench.watching();
        phone.send_raw(r#"{"type":"media.command","action":"next"}"#).unwrap();
        let start = Instant::now();
        while lock(&bench.recorded.calls).is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5), "commande jamais reçue");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(*lock(&bench.recorded.calls), vec!["next"]);
    }

    #[test]
    fn a_reconnected_phone_keeps_its_watch() {
        // Le téléphone revient avant que le PC ait vu la mort de sa session : la fin de
        // l'ancienne ne doit pas emporter l'abonnement de la nouvelle.
        let bench = Bench::new();
        let _old = bench.watching();
        let mut phone = bench.watching();
        std::thread::sleep(Duration::from_millis(1_500));
        bench.new_track("AUTRE");
        let state = phone.receive().unwrap();
        assert!(state.contains("AUTRE"), "{state}");
    }

    #[test]
    fn the_state_leaves_at_once_for_a_watching_phone() {
        let bench = Bench::new();
        let mut phone = bench.watching();
        for title in ["UN", "DEUX", "TROIS"] {
            let start = Instant::now();
            bench.new_track(title);
            let state = phone.receive().unwrap();
            assert!(state.contains(title), "{state}");
            assert!(start.elapsed() < Duration::from_millis(300), "{title} : {:?}", start.elapsed());
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    #[test]
    fn media_sessions_are_named_after_their_app() {
        // La liste du média donne (identifiant, titre en cours) : le téléphone veut le nom de l'appli.
        let sessions = vec![("Spotify.exe".to_owned(), "FROGZ".to_owned()), ("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic".to_owned(), String::new())];
        assert_eq!(named(sessions), vec![("Spotify.exe".to_owned(), "Spotify".to_owned()), ("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic".to_owned(), "ZuneMusic".to_owned())]);
    }

    #[test]
    fn the_link_view_says_who_is_there() {
        let core = Core::for_tests("pc-1", "Bureau");
        pair(&core, "tel-1");
        let phones = core.phones(&["tel-1".to_owned()], 5);
        assert_eq!(phones, vec![PhoneView { id: "tel-1".into(), name: "Pixel".into(), connected: true, last_seen_ms: 1, granted: horizon_core::phone::registry::default_granted() }]);
        assert!(!core.phones(&[], 5)[0].connected);
    }
}
