//! Sessions Claude Code : petit serveur HTTP local qui reçoit les hooks (`horizon-hook.exe`)
//! et diffuse la liste des sessions avec leur état ; alerte « Claude attend » (îlot dynamique,
//! signal Prisme), état git des dossiers, mise au premier plan du terminal d'une session,
//! consommation du jour lue dans les transcriptions (fil `claude-usage`).
//!
//! Repris de `app/src-tauri/src/widgets/claude_sessions/mod.rs` ; le serveur des hooks est le
//! même (adresse `horizon_hook::ADDR`, mêmes règles d'acceptation) : `horizon-hook.exe` parle
//! indifféremment à l'une ou l'autre version. Partie interface : `ui` (barre) et `panel`
//! (panneau) ; captures d'écran proposées à une session : `capture`.
//!
//! Sessions connues : comme dans la version Tauri, la liste vit en mémoire et se remplit au fil
//! des hooks reçus ; une session terminée avant le lancement n'y est pas (la version Tauri non
//! plus après un redémarrage). Pendant la migration, une seule des deux versions peut écouter :
//! celle qui tourne depuis l'ouverture de session a l'historique.

pub mod capture;
mod panel;
pub mod ui;

use crate::alerts::{self, Action, Severity};
use crate::util::{Published, Waker, lock, spawn_named, start_thread};
use horizon_core::claude_usage::{UsageBook, UsageView, guess_transcript};
use horizon_core::claude_sessions::{Alert, AlertOptions, HookEvent, SessionTracker, SessionView, pick_host_window};
use horizon_core::dnd::AlertKind;
use horizon_core::git_status::parse_porcelain_v2;
use horizon_core::time::now_ms as now;
use horizon_hook::ADDR;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use crate::host::{AppHandle, Manager};
use crate::widgets::Backend;

/// Sessions Claude Code.
pub const EV_CLAUDE: &str = "horizon://claude";
/// Consommation du jour des sessions Claude Code (transcriptions).
pub const EV_CLAUDE_USAGE: &str = "horizon://claude-usage";

pub const BACKEND: Backend = Backend {
    id: "claude_sessions",
    name: "sessions Claude Code",
    manage: |app| crate::widgets::manage(app, ClaudeSessions::default()),
    start: Some(|app| crate::widgets::start_each(app, &[("sessions", start), ("captures d'écran vers Claude", capture::start)])),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

/// Délai maximal d'un `git status` (gros dépôt, disque lent).
const GIT_TIMEOUT: Duration = Duration::from_secs(4);
/// Consommation : relecture au plus toutes les 5 s (hooks en rafale), et au moins chaque
/// minute (sous-agents, réponses sans hook).
const USAGE_MIN: Duration = Duration::from_secs(5);
const USAGE_MAX: Duration = Duration::from_secs(60);

pub struct ClaudeSessions {
    tracker: Mutex<SessionTracker>,
    /// Liste envoyée aux fenêtres : seuls ses changements partent (un hook d'outil sans effet
    /// visible, la relecture de chaque minute sans session oubliée, n'envoient rien).
    shown: Published<Vec<SessionView>>,
    /// Une relecture git de toutes les sessions est en cours.
    git_busy: AtomicBool,
    /// Heure (ms Unix) du dernier hook reçu ; 0 = aucun depuis le démarrage.
    last_hook_ms: AtomicU64,
    /// Heure de la dernière demande d'autorisation (`PermissionRequest`) ; 0 = aucune.
    last_permission_ms: AtomicU64,
    /// Transcriptions des sessions vues (fil `claude-usage` seulement, et les hooks).
    usage: Mutex<UsageBook>,
    usage_shown: Published<UsageView>,
    usage_wake: Waker,
}

impl ClaudeSessions {
    /// Heure du dernier hook reçu (page « État »).
    pub fn last_hook_ms(&self) -> Option<u64> {
        Some(self.last_hook_ms.load(Ordering::Relaxed)).filter(|&t| t > 0)
    }

    /// Heure de la dernière demande d'autorisation reçue (page « État »).
    pub fn last_permission_ms(&self) -> Option<u64> {
        Some(self.last_permission_ms.load(Ordering::Relaxed)).filter(|&t| t > 0)
    }
}

impl Default for ClaudeSessions {
    fn default() -> Self {
        ClaudeSessions {
            tracker: Mutex::default(),
            shown: Published::new(EV_CLAUDE, Vec::new()),
            git_busy: AtomicBool::default(),
            last_hook_ms: AtomicU64::default(),
            last_permission_ms: AtomicU64::default(),
            usage: Mutex::default(),
            usage_shown: Published::new(EV_CLAUDE_USAGE, UsageView::default()),
            usage_wake: Waker::default(),
        }
    }
}

fn tracker(app: &AppHandle) -> std::sync::MutexGuard<'_, SessionTracker> {
    lock(&app.state::<ClaudeSessions>().inner().tracker)
}

fn broadcast(app: &AppHandle) {
    let list = tracker(app).list(now());
    app.state::<ClaudeSessions>().shown.set(app, list);
}

/// Réglages de l'alerte « Claude attend » (section « Alertes » des réglages).
fn alert_options(app: &AppHandle) -> AlertOptions {
    let alerts = crate::state::settings().alerts;
    app.state::<crate::state::AppState>().read(|l| alerts.claude(l))
}

/// « Claude attend » (ou demande une autorisation) : dans l'îlot (clic = terminal de la
/// session) et signal lumineux, selon les réglages et « Ne pas déranger ».
fn alert(app: &AppHandle, a: Alert) {
    if !alerts::allowed(app, AlertKind::Claude) {
        return;
    }
    let opts = alert_options(app);
    if opts.island || opts.light {
        let island = alerts::Alert::app("Claude", "", Severity::Attention, a.name, a.message)
            .with_action(Action::Session { id: a.session_id });
        alerts::announce(app, AlertKind::Claude, island, opts.island, opts.light);
    }
}

/// Lit l'état git d'un dossier (bloquant, au plus `GIT_TIMEOUT`) et le reporte sur ses sessions.
fn read_git(app: &AppHandle, cwd: &str) {
    let git = horizon_sys::git::status(cwd, GIT_TIMEOUT).map(|out| parse_porcelain_v2(&out));
    tracker(app).set_git(cwd, git);
}

/// Relit l'état git d'un dossier (hors du fil appelant) puis rediffuse les sessions.
fn refresh_git(app: &AppHandle, cwd: String) {
    let app = app.clone();
    spawn_named("claude-git", move || {
        read_git(&app, &cwd);
        broadcast(&app);
    });
}

/// Le module vient d'être désactivé (`crate::modules`) : sessions et consommation oubliées, plus
/// rien sur les barres, dans la palette ni dans l'îlot. Les hooks qui arrivent encore sont
/// ignorés ([`on_hook`]) ; les fils ne sont plus lancés au démarrage suivant.
pub fn clear(app: &AppHandle) {
    let state = app.state::<ClaudeSessions>();
    *tracker(app) = SessionTracker::default();
    *lock(&state.usage) = UsageBook::default();
    broadcast(app);
    state.usage_shown.set(app, UsageView::default());
}

fn on_hook(app: &AppHandle, ev: HookEvent) {
    if !crate::modules::enabled(BACKEND.id) {
        return;
    }
    let state = app.state::<ClaudeSessions>();
    state.last_hook_ms.store(now(), Ordering::Relaxed);
    if ev.hook_event_name == "PermissionRequest" {
        state.last_permission_ms.store(now(), Ordering::Relaxed);
    }
    let stopped = matches!(ev.hook_event_name.as_str(), "Stop" | "StopFailure");
    let cwd = ev.cwd.clone();
    if ev.hook_event_name != "SessionEnd" {
        if let Some(path) = transcript(&ev.transcript_path, &cwd, &ev.session_id) {
            lock(&state.usage).track(&ev.session_id, &cwd, path);
        }
        state.usage_wake.wake();
    }
    let a = tracker(app).apply(ev, now());
    broadcast(app);
    if let Some(a) = a {
        alert(app, a);
    }
    if stopped && !cwd.is_empty() {
        refresh_git(app, cwd);
    }
}

/// Transcription d'une session : celle donnée par le hook, sinon devinée d'après le dossier.
fn transcript(given: &Option<String>, cwd: &str, id: &str) -> Option<PathBuf> {
    if let Some(p) = given.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var_os("USERPROFILE")?;
    (!cwd.is_empty() && !id.is_empty()).then(|| guess_transcript(std::path::Path::new(&home), cwd, id))
}

/// Relit les transcriptions et publie la consommation du jour.
fn refresh_usage(app: &AppHandle) {
    let state = app.state::<ClaudeSessions>();
    let active: Vec<String> = tracker(app).list(now()).into_iter().map(|s| s.id).collect();
    let now_ms = now();
    let view = {
        let mut book = lock(&state.usage);
        book.refresh(now_ms, &active);
        book.view(horizon_core::time::local_day_start_ms(now_ms))
    };
    state.usage_shown.set(app, view);
}

/// Fil de la consommation : réveillé par les hooks, au plus toutes les 5 s, au moins chaque
/// minute ; rien pendant une absence.
fn start_usage(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("claude-usage", move || {
        let state = app.state::<ClaudeSessions>();
        loop {
            horizon_sys::presence::wait_while_away();
            let t = Instant::now();
            refresh_usage(&app);
            state.usage_wake.wait(USAGE_MAX);
            if let Some(left) = USAGE_MIN.checked_sub(t.elapsed()) {
                std::thread::sleep(left);
            }
        }
    })
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    start_usage(app)?;
    let server =
        tiny_http::Server::http(ADDR).map_err(|e| anyhow::anyhow!("récepteur des hooks indisponible sur {ADDR} : {e}"))?;
    let a = app.clone();
    start_thread("claude-hooks", move || {
        for mut req in server.incoming_requests() {
            let ok = accepts(
                req.method() == &tiny_http::Method::Post,
                req.url(),
                header(&req, "Origin"),
                header(&req, "Content-Type"),
            );
            let mut body = String::new();
            let mut reader = std::io::Read::take(req.as_reader(), MAX_BODY);
            let read = ok && std::io::Read::read_to_string(&mut reader, &mut body).is_ok();
            // Réponse d'abord : le hook (et donc Claude Code) n'attend pas l'alerte.
            let _ = req.respond(tiny_http::Response::empty(if ok { 204 } else { 404 }));
            if read {
                match serde_json::from_str::<HookEvent>(&body) {
                    Ok(ev) => on_hook(&a, ev),
                    Err(e) => log::debug!("hook illisible : {e}"),
                }
            }
        }
    })?;
    // Les sessions muettes depuis longtemps disparaissent : on relit la liste chaque minute.
    let a = app.clone();
    start_thread("claude-oubli", move || {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            broadcast(&a);
        }
    })
}

/// Relit l'état git de toutes les sessions (ouverture du panneau) ; une seule relecture à la fois.
pub fn claude_refresh_git(app: AppHandle) {
    let state = app.state::<ClaudeSessions>();
    if state.git_busy.swap(true, Ordering::SeqCst) {
        return;
    }
    let cwds = tracker(&app).cwds();
    let app = app.clone();
    spawn_named("claude-git", move || {
        for cwd in cwds {
            read_git(&app, &cwd);
        }
        app.state::<ClaudeSessions>().git_busy.store(false, Ordering::SeqCst);
        broadcast(&app);
    });
}

/// Consommation du jour (panneau) ; une relecture part en tâche de fond.
pub fn claude_usage(app: AppHandle) -> UsageView {
    let state = app.state::<ClaudeSessions>();
    state.usage_wake.wake();
    state.usage_shown.get()
}

/// Oublie une session morte (« × » du panneau) ; elle revient si elle donne des nouvelles.
pub fn claude_forget(app: AppHandle, id: String) {
    if tracker(&app).forget(&id) {
        broadcast(&app);
    }
}

/// Sessions connues (palette : une entrée par session).
pub fn sessions(app: &AppHandle) -> Vec<SessionView> {
    tracker(app).list(now())
}

/// Met au premier plan la fenêtre du terminal qui héberge la session.
pub fn claude_focus(app: AppHandle, id: String) -> Result<(), String> {
    let (procs, hint) = tracker(&app).host(&id);
    let windows = horizon_sys::window_focus::main_windows();
    let w = pick_host_window(&procs, hint, horizon_sys::window_focus::exe_name, &windows)
        .ok_or("fenêtre du terminal introuvable")?;
    if horizon_sys::window_focus::bring_to_front(w) {
        Ok(())
    } else {
        Err("Windows a refusé la mise au premier plan".into())
    }
}

/// Seul le hook local est accepté : pas d'en-tête `Origin` (une page web en envoie toujours)
/// et un corps JSON déclaré (une page ne peut pas l'envoyer sans pré-vérification CORS).
fn accepts(method_is_post: bool, url: &str, origin: Option<&str>, content_type: Option<&str>) -> bool {
    method_is_post
        && url == "/hook"
        && origin.is_none()
        && content_type.is_some_and(|c| c.trim_start().to_ascii_lowercase().starts_with("application/json"))
}

/// Corps plus gros ignoré : comme le hook (`MAX_INPUT`), un événement d'outil peut porter une
/// longue sortie de commande.
const MAX_BODY: u64 = 1024 * 1024;

fn header<'a>(req: &'a tiny_http::Request, name: &str) -> Option<&'a str> {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

#[cfg(test)]
mod tests {
    use super::accepts;

    #[test]
    fn only_local_json_hook_is_accepted() {
        assert!(accepts(true, "/hook", None, Some("application/json")));
        assert!(accepts(true, "/hook", None, Some("application/json; charset=utf-8")));
        assert!(!accepts(true, "/hook", Some("https://exemple.com"), Some("application/json")));
        assert!(!accepts(true, "/hook", None, Some("text/plain")));
        assert!(!accepts(true, "/hook", None, None));
        assert!(!accepts(false, "/hook", None, Some("application/json")));
        assert!(!accepts(true, "/autre", None, Some("application/json")));
    }
}
