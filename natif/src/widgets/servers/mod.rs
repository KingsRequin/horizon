//! Widget « Serveurs » (homelab) : hôtes de `~/.ssh/config` complétés par les réglages des
//! widgets, sondés toutes les 60 s hors du fil principal (tout de suite à la sortie de veille et
//! à l'ouverture du panneau), en parallèle. Chute et retour d'un hôte annoncés dans l'îlot
//! (deux échecs consécutifs avant de le dire hors ligne). Terminal SSH et interface web.
//!
//! Repris de `app/src-tauri/src/widgets/servers.rs`. Écart : `servers_test` est synchrone (à
//! appeler hors du fil de l'interface). Partie interface : `ui` (barres) et `panel` (panneau
//! surgissant).

mod panel;
pub mod ui;

use crate::alerts::{Alert, Severity};
use crate::state::AppState;
use horizon_core::dnd::AlertKind;
use horizon_core::homelab::{
    Change, Health, HostSetting, Probe, ServersSettings, SshHost, Status, Target, parse_ssh_config, safe_ssh_target, targets,
};
use horizon_core::layout::{Layout, WidgetKind};
use serde::Serialize;
use crate::util::{MtimeCache, Waker, lock, start_thread};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use crate::host::{AppHandle, Emitter, Manager};
use crate::widgets::Backend;

/// L'état des serveurs a été relu : les widgets redemandent le leur.
pub const EV_SERVERS: &str = "horizon://servers";

pub const BACKEND: Backend = Backend {
    id: "servers",
    name: "serveurs",
    manage: |app| crate::widgets::manage(app, Homelab::default()),
    start: Some(start),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};


const PERIOD: Duration = Duration::from_secs(60);
/// Deux sondages rapprochés (panneau rouvert) ne relancent pas les connexions.
const MIN_GAP: Duration = Duration::from_secs(5);
const TCP_TIMEOUT: Duration = Duration::from_millis(1500);
const HTTP_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Default)]
pub struct Homelab {
    health: Mutex<HashMap<Probe, Health>>,
    /// `~/.ssh/config` lu (relu seulement s'il change).
    ssh: MtimeCache<Vec<SshHost>>,
    wake: Waker,
    /// Verdicts du dernier sondage annoncé aux fenêtres.
    last_verdicts: Mutex<Option<Verdicts>>,
}

fn ssh_config_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh").join("config"))
}

impl Homelab {
    /// Hôtes de `~/.ssh/config` (vide s'il n'existe pas).
    fn ssh_hosts(&self) -> Vec<SshHost> {
        let Some(path) = ssh_config_path() else { return Vec::new() };
        let read = || {
            let hosts = match std::fs::read_to_string(&path) {
                Ok(text) => parse_ssh_config(&text),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => {
                    log::warn!("lecture de la configuration ssh : {e}");
                    Vec::new()
                }
            };
            log::info!("configuration ssh : {} hôte(s)", hosts.len());
            Some(hosts)
        };
        self.ssh.get(&path, read).unwrap_or_default()
    }

    /// Demande un sondage tout de suite.
    pub fn refresh(&self) {
        self.wake.wake();
    }
}

/// Réglages des widgets Serveurs de la disposition active.
fn widget_settings(l: &Layout) -> Vec<(String, ServersSettings)> {
    l.widgets_of(WidgetKind::SERVERS).map(|w| (w.id.clone(), ServersSettings::from_value(&w.settings))).collect()
}

/// Réglages des widgets Serveurs, lus dans l'état de l'appli.
fn all_settings(app: &AppHandle) -> Vec<(String, ServersSettings)> {
    app.state::<AppState>().read(widget_settings)
}

/// Hôtes visibles d'un ensemble de réglages.
fn visible(ssh: &[SshHost], s: &ServersSettings) -> Vec<Target> {
    targets(ssh, s).into_iter().filter(|t| !t.hidden).collect()
}

/// Hôtes visibles de tous les widgets Serveurs (sans doublon), ou ceux de `~/.ssh/config`
/// s'il n'y a pas de widget (palette de commandes).
pub fn known_hosts(app: &AppHandle) -> Vec<Target> {
    let ssh = app.state::<Homelab>().ssh_hosts();
    let settings = all_settings(app);
    if settings.is_empty() {
        return visible(&ssh, &ServersSettings::default());
    }
    let mut out: Vec<Target> = Vec::new();
    for (_, s) in &settings {
        for t in visible(&ssh, s) {
            if !out.iter().any(|o| o.id == t.id) {
                out.push(t);
            }
        }
    }
    out
}

/// Résultat d'une sonde : latence en ms, ou `None` si l'hôte ne répond pas.
fn probe(p: &Probe) -> Option<u32> {
    use std::net::{TcpStream, ToSocketAddrs};
    let start = Instant::now();
    let ms = |s: Instant| u32::try_from(s.elapsed().as_millis()).unwrap_or(u32::MAX);
    match p {
        Probe::Tcp { host, port } => {
            let addrs = (host.as_str(), *port).to_socket_addrs().ok()?;
            for a in addrs {
                let t = Instant::now();
                if TcpStream::connect_timeout(&a, TCP_TIMEOUT).is_ok() {
                    return Some(ms(t));
                }
            }
            None
        }
        Probe::Http { url } => {
            // Un serveur du homelab a souvent un certificat auto-signé : on ne vérifie que
            // qu'il répond (rien n'est envoyé ni lu au-delà des en-têtes). Toute réponse HTTP,
            // même 401 ou 404, veut dire « en ligne ».
            let agent = ureq::Agent::config_builder()
                .timeout_global(Some(HTTP_TIMEOUT))
                .http_status_as_error(false)
                .tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build())
                .build()
                .new_agent();
            agent.head(url).header("User-Agent", "Horizon").call().ok().map(|_| ms(start))
        }
    }
}

/// Sonde tous les hôtes voulus en parallèle, met à jour leur état et annonce les changements.
fn probe_all(app: &AppHandle) {
    let ssh = app.state::<Homelab>().ssh_hosts();
    let settings = all_settings(app);
    // Une sonde par façon de joindre un hôte, avec le nom à annoncer.
    let mut wanted: Vec<(Probe, String)> = Vec::new();
    for (_, s) in &settings {
        for t in visible(&ssh, s) {
            if !wanted.iter().any(|(p, _)| *p == t.probe) {
                wanted.push((t.probe, t.label));
            }
        }
    }
    let results: Vec<Option<u32>> = std::thread::scope(|scope| {
        let handles: Vec<_> = wanted.iter().map(|(p, _)| scope.spawn(move || probe(p))).collect();
        handles.into_iter().map(|h| h.join().ok().flatten()).collect()
    });
    let now = horizon_core::time::now_ms() as i64;
    // Changements relevés sous le verrou, annoncés après l'avoir relâché.
    let lab = app.state::<Homelab>();
    let (changes, verdicts): (Vec<(String, Option<u32>, Change)>, Verdicts) = {
        let mut health = lock(&lab.health);
        health.retain(|p, _| wanted.iter().any(|(w, _)| w == p));
        let changes = wanted
            .iter()
            .zip(results)
            .filter_map(|((p, label), r)| health.entry(p.clone()).or_default().observe(r, now).map(|c| (label.clone(), r, c)))
            .collect();
        let verdicts = wanted
            .iter()
            .map(|(p, _)| health.get(p).map(|h| (h.status(), h.since_ms())).unwrap_or((Status::Unknown, None)))
            .collect();
        (changes, verdicts)
    };
    for (label, r, change) in changes {
        match change {
            Change::WentDown => {
                log::info!("serveur {label} hors ligne");
                let title = format!("{label} est hors ligne");
                let alert = Alert::icon("hardDrives", Severity::Alert, title, "Injoignable deux fois de suite".into());
                crate::alerts::show(app, AlertKind::Server, alert);
            }
            Change::CameBack => {
                log::info!("serveur {label} de nouveau en ligne");
                let body = r.map(|ms| format!("Répond en {ms} ms")).unwrap_or_default();
                let title = format!("{label} est de nouveau en ligne");
                crate::alerts::show(app, AlertKind::Server, Alert::icon("hardDrives", Severity::Info, title, body));
            }
        }
    }
    // Les widgets ne relisent leur état que s'il a changé (verdict, depuis quand) ; le panneau
    // ouvert, lui, montre aussi les latences.
    let changed = *lock(&lab.last_verdicts) != Some(verdicts.clone());
    if changed || app.state::<crate::popup::PopupState>().is_shown("servers") {
        *lock(&lab.last_verdicts) = Some(verdicts);
        let _ = app.emit(EV_SERVERS, ());
    }
}

/// État affiché de chaque sonde voulue, dans l'ordre des sondes.
type Verdicts = Vec<(Status, Option<i64>)>;

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    // Sortie de veille : sondage immédiat (le réseau est déjà revenu, voir resume.rs).
    let a = app.clone();
    start_thread("servers-resume", move || {
        loop {
            if crate::resume::sleep(Duration::from_secs(24 * 3600)) {
                a.state::<Homelab>().refresh();
            }
        }
    })?;
    let app = app.clone();
    start_thread("servers", move || {
        let lab = app.state::<Homelab>();
        loop {
            // Session verrouillée ou écran éteint : pas de sondage ; au retour, tout de suite.
            horizon_sys::presence::wait_while_away();
            let last = Instant::now();
            probe_all(&app);
            // Un réveil trop proche du dernier sondage le laisse valoir.
            while lab.wake.wait(PERIOD.saturating_sub(last.elapsed())) && last.elapsed() < MIN_GAP {}
        }
    })
}


/// Hôte tel qu'affiché par le widget et son panneau.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostView {
    pub id: String,
    pub label: String,
    pub address: String,
    /// Un terminal SSH peut être ouvert.
    pub ssh: bool,
    pub url: Option<String>,
    /// Test utilisé (« TCP 22 », « HTTPS »).
    pub check: String,
    pub status: Status,
    pub latency_ms: Option<u32>,
    /// Dernier changement d'état (ms depuis l'époque Unix).
    pub since_ms: Option<i64>,
}

/// Hôtes visibles du widget `widget_id`, avec leur dernier état connu.
pub fn servers_state(app: AppHandle, widget_id: String) -> Vec<HostView> {
    let s = app.state::<AppState>().read(|l| l.widget(&widget_id).map(|w| ServersSettings::from_value(&w.settings)));
    let lab = app.state::<Homelab>();
    let ssh = lab.ssh_hosts();
    let health = lock(&lab.health);
    visible(&ssh, &s.unwrap_or_default())
        .into_iter()
        .map(|t| {
            let h = health.get(&t.probe).cloned().unwrap_or_default();
            HostView {
                check: t.probe.label(),
                status: h.status(),
                latency_ms: h.latency_ms(),
                since_ms: h.since_ms(),
                ssh: t.ssh.is_some(),
                id: t.id,
                label: t.label,
                address: t.address,
                url: t.url,
            }
        })
        .collect()
}

/// Panneau ouvert, réglages modifiés : sondage tout de suite.
pub fn servers_refresh(app: AppHandle) {
    app.state::<Homelab>().refresh();
}

/// Hôte de `~/.ssh/config` pour la fenêtre de réglages (sans utilisateur ni clé).
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SshHostView {
    pub alias: String,
    pub address: String,
    pub port: Option<u16>,
}

pub fn servers_ssh_hosts(app: AppHandle) -> Vec<SshHostView> {
    app.state::<Homelab>()
        .ssh_hosts()
        .into_iter()
        .map(|h| SshHostView { address: h.hostname.unwrap_or_else(|| h.alias.clone()), alias: h.alias, port: h.port })
        .collect()
}

/// Ouvre un terminal SSH vers `target` : Windows Terminal s'il est installé, sinon `ssh` dans
/// une nouvelle console.
pub fn open_ssh(target: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !safe_ssh_target(target) {
        return Err("nom d’hôte refusé".into());
    }
    let wt = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\WindowsApps\wt.exe"));
    if let Some(wt) = wt.filter(|p| p.exists()) {
        return std::process::Command::new(wt).args(["ssh", target]).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    std::process::Command::new("ssh")
        .arg(target)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("ssh introuvable ({e})"))
}

/// Terminal SSH vers un hôte connu (jamais une commande arbitraire venue de la page).
pub fn servers_ssh(app: AppHandle, host_id: String) -> Result<(), String> {
    let ssh = app.state::<Homelab>().ssh_hosts();
    let settings = all_settings(&app);
    let target = settings
        .iter()
        .flat_map(|(_, s)| targets(&ssh, s))
        .chain(targets(&ssh, &ServersSettings::default()))
        .find(|t| t.id == host_id)
        .and_then(|t| t.ssh)
        .ok_or("hôte inconnu")?;
    open_ssh(&target)
}

/// Teste tout de suite un hôte tel qu'il est réglé (bouton « Tester » des réglages) : latence
/// en ms, ou la raison de l'échec.
/// Bloquant (jusqu'à quelques secondes) : hors du fil de l'interface.
pub fn servers_test(app: AppHandle, host: HostSetting) -> Result<u32, String> {
    let ssh = app.state::<Homelab>().ssh_hosts();
    let id = host.id.clone();
    let settings = ServersSettings { hosts: vec![host] };
    let target = targets(&ssh, &settings).into_iter().find(|t| t.id == id).ok_or("adresse manquante")?;
    probe(&target.probe).ok_or_else(|| format!("aucune réponse ({})", target.probe.label()))
}
