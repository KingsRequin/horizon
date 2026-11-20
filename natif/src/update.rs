//! Mise à jour automatique des versions publiées (Releases GitHub de `REPO`). Seulement dans
//! une compilation publiée (`HORIZON_RELEASE`, posée par `scripts/package.ps1`) de l'édition
//! publique : une compilation depuis les sources se met à jour par git.
//!
//! Déroulé : une minute après le démarrage, puis toutes les 12 heures (ou à la demande, page
//! « État »), la dernière Release est lue ; une version plus récente est téléchargée dans
//! `<dossier d'installation>/update/`, vérifiée (SHA-256 de `SHA256SUMS`), annoncée dans l'îlot
//! et sur la page « État ». Elle s'installe au démarrage suivant (`apply_at_start`), ou tout de
//! suite au clic (`install_now`) : les exécutables sont échangés (`horizon_core::update::swap`),
//! la nouvelle version est lancée et attend la fin de celle-ci (`HORIZON_WAIT_PID`, voir
//! `wait_previous`). Si elle ne peut pas être lancée, l'ancienne est remise. La nouvelle version
//! corrige au démarrage celle qu'affichent les applications installées de Windows.

use crate::alerts::{Action, Alert, Severity};
use crate::host::{AppHandle, Manager};
use crate::util::{Waker, lock, start_thread};
use horizon_core::dnd::AlertKind;
use horizon_core::update::{self as core, Latest, Version};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// La mise à jour automatique existe dans cette compilation.
pub const ENABLED: bool = option_env!("HORIZON_RELEASE").is_some();
/// Dépôt GitHub des versions publiées.
pub const REPO: &str = "KingsRequin/horizon";

const FIRST: Duration = Duration::from_secs(60);
const PERIOD: Duration = Duration::from_secs(12 * 3600);
/// Taille maximale d'un exécutable téléchargé, et de la réponse de l'API.
const EXE_LIMIT: u64 = 64 * 1024 * 1024;
const API_LIMIT: u64 = 1024 * 1024;
/// Variable d'environnement de la version lancée par une mise à jour : processus à attendre.
const WAIT_PID: &str = "HORIZON_WAIT_PID";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// Pas encore cherché.
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(Version),
    /// Téléchargée et vérifiée : installée au démarrage suivant.
    Ready { version: Version, notes_url: String },
    Failed(String),
}

#[derive(Default)]
pub struct Updater {
    status: Mutex<Status>,
    /// Dernière recherche terminée (ms).
    checked_ms: Mutex<Option<u64>>,
    /// Recherche demandée à la main (même si la fonction est coupée).
    forced: AtomicBool,
    wake: Waker,
}

impl Updater {
    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    pub fn checked_ms(&self) -> Option<u64> {
        *lock(&self.checked_ms)
    }

    fn set(&self, s: Status) {
        *lock(&self.status) = s;
    }
}

/// Version qui tourne.
pub fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("version du paquet")
}

/// Dossier d'installation : celui de l'exécutable qui tourne.
fn install_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

/// Lancée par une mise à jour : attend (20 s au plus) que la version d'avant soit fermée, pour
/// prendre l'instance unique et la place en haut des écrans. À appeler avant `instance::acquire`.
pub fn wait_previous() {
    let Some(pid) = std::env::var(WAIT_PID).ok().and_then(|p| p.parse::<u32>().ok()) else { return };
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY : poignée ouverte ici et refermée juste après l'attente.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 20_000);
            let _ = CloseHandle(h);
        }
    }
}

/// Au démarrage (après l'instance unique) : efface les restes d'un échange précédent et
/// installe une version qui attend. Vrai si une nouvelle version a été lancée : celle-ci doit
/// s'arrêter tout de suite.
pub fn apply_at_start() -> bool {
    if !ENABLED {
        return false;
    }
    let Some(dir) = install_dir() else { return false };
    core::remove_old(&dir);
    match core::staged(&dir) {
        Some(v) if v > current() && crate::state::settings().features.auto_update => match relaunch(&dir) {
            Ok(()) => {
                log::info!("mise à jour : {} → {v}, nouvelle version lancée", current());
                true
            }
            Err(e) => {
                log::error!("mise à jour vers {v} impossible : {e}");
                false
            }
        },
        Some(v) if v <= current() => {
            core::stage_clear(&dir);
            false
        }
        _ => false,
    }
}

/// Échange les exécutables et lance le nouveau ; remet l'ancien s'il ne démarre pas.
fn relaunch(dir: &std::path::Path) -> Result<(), String> {
    core::swap(dir)?;
    let spawned = std::process::Command::new(dir.join(core::FILES[0])).env(WAIT_PID, std::process::id().to_string()).spawn();
    spawned.map(|_| ()).map_err(|e| {
        core::rollback(dir, &core::FILES);
        format!("nouvelle version impossible à lancer ({e}), ancienne remise")
    })
}

/// Une mise à jour remplace les exécutables sans repasser par l'installateur : la version
/// affichée dans les applications installées de Windows est remise à celle qui tourne.
fn show_installed_version() {
    let Some(dir) = install_dir() else { return };
    match horizon_sys::installed::sync_version(&dir, current()) {
        Ok(true) => log::info!("applications installées de Windows : version {} affichée", current()),
        Ok(false) => {}
        Err(e) => log::warn!("applications installées de Windows : version affichée inchangée : {e:#}"),
    }
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    if !ENABLED {
        return Ok(());
    }
    let app = app.clone();
    start_thread("mise-a-jour", move || {
        show_installed_version();
        let u = app.state::<Updater>();
        u.wake.wait(FIRST);
        loop {
            let forced = u.forced.swap(false, Ordering::Relaxed);
            if forced || crate::state::settings().features.auto_update {
                check(&app);
            }
            u.wake.wait(PERIOD);
        }
    })
}

/// Recherche tout de suite (page « État »), même si la fonction est coupée.
pub fn check_now(app: &AppHandle) {
    let u = app.state::<Updater>();
    u.forced.store(true, Ordering::Relaxed);
    u.wake.wake();
}

fn check(app: &AppHandle) {
    let u = app.state::<Updater>();
    if matches!(u.status(), Status::Ready { .. }) {
        // Déjà prête : rien à refaire avant le redémarrage.
        return;
    }
    u.set(Status::Checking);
    let result = find_and_stage(&u);
    *lock(&u.checked_ms) = Some(horizon_core::time::now_ms());
    match result {
        Ok(Some((version, notes_url))) => {
            log::info!("mise à jour : {version} prête");
            u.set(Status::Ready { version, notes_url });
            let alert = Alert::icon(
                "arrowsClockwise",
                Severity::Info,
                format!("Horizon {version} est prête"),
                "Installée au prochain démarrage · clic : redémarrer maintenant".into(),
            )
            .with_action(Action::Update);
            crate::alerts::show(app, AlertKind::Update, alert);
        }
        Ok(None) => u.set(Status::UpToDate),
        Err(e) => {
            log::warn!("mise à jour : {e}");
            u.set(Status::Failed(e));
        }
    }
    crate::status::wake(app);
}

/// Version plus récente téléchargée et vérifiée, ou `None` si rien de neuf.
fn find_and_stage(u: &Updater) -> Result<Option<(Version, String)>, String> {
    let dir = install_dir().ok_or("dossier d'installation introuvable")?;
    let json = crate::web::get(&core::latest_url(REPO), API_LIMIT).map_err(|e| format!("GitHub : {e}"))?;
    let offer = match core::read_latest(&json, REPO, current()) {
        Latest::Newer(o) => o,
        Latest::UpToDate(_) => {
            core::stage_clear(&dir);
            return Ok(None);
        }
        Latest::Invalid(e) => return Err(e),
    };
    if core::staged(&dir) == Some(offer.version) {
        return Ok(Some((offer.version, offer.notes_url)));
    }
    u.set(Status::Downloading(offer.version));
    let sums = crate::web::get(&offer.sums_url, 64 * 1024).map_err(|e| format!("{} : {e}", core::SUMS))?;
    let sums = core::parse_sums(&sums);
    core::stage_reset(&dir).map_err(|e| format!("dossier d'attente : {e}"))?;
    for (name, url) in &offer.files {
        let bytes = crate::web::get_bytes(url, EXE_LIMIT).map_err(|e| format!("{name} : {e}"))?;
        core::verify(&sums, name, &bytes)?;
        core::stage_file(&dir, name, &bytes).map_err(|e| format!("{name} : {e}"))?;
    }
    core::stage_ready(&dir, offer.version).map_err(|e| format!("dossier d'attente : {e}"))?;
    Ok(Some((offer.version, offer.notes_url)))
}

/// Installe tout de suite la version prête : échange, lancement, puis fin de celle-ci.
pub fn install_now(app: &AppHandle) -> Result<(), String> {
    let Status::Ready { version, .. } = app.state::<Updater>().status() else {
        return Err("aucune mise à jour prête".into());
    };
    let dir = install_dir().ok_or("dossier d'installation introuvable")?;
    relaunch(&dir)?;
    log::info!("mise à jour : {} → {version}, redémarrage", current());
    app.exit(0);
    Ok(())
}

/// Page de la Release prête (notes de version).
pub fn open_notes(app: &AppHandle) -> Result<(), String> {
    match app.state::<Updater>().status() {
        Status::Ready { notes_url, .. } => crate::web::url_open(&notes_url),
        _ => crate::web::url_open(&format!("https://github.com/{REPO}/releases")),
    }
}
