//! Page « État » des réglages : l'état réel de chaque intégration (hooks Claude, relais
//! Discord, Prisme, notifications Windows, batteries de SharkApp, quota, raccourci de la
//! palette, fichiers de réglages, fils système relancés, démarrages ratés, mises à jour), les derniers
//! avertissements du journal et quelques actions. Un fil relit ces états (sans le journal)
//! toutes les 15 s : tant qu'une intégration est en échec, un témoin s'affiche sur l'icône de
//! notification et sur la barre principale (`horizon://health`, « ! » de `ui/bar-status.slint`).
//!
//! Repris de `app/src-tauri/src/status.rs`. Différences : les commandes sont des fonctions
//! (`status_get` est appelé hors du fil de l'interface : il relit le journal) ; le journal est
//! celui d'Horizon (`journal::path`) ; les démarrages ratés du cœur ont chacun leur ligne.

use crate::events::EV_HEALTH;
use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use crate::util::{Published, Waker, lock, start_thread};
use horizon_core::layout::WidgetKind;
use horizon_core::settings::{Settings, SettingsFile};
use std::sync::Mutex;
use std::time::Duration;

const PERIOD: Duration = Duration::from_secs(15);
/// Mesure de batterie plus ancienne : SharkApp ne met plus le fichier à jour.
const BATTERY_STALE_MS: i64 = 2 * 3_600_000;
/// Avertissements du journal montrés, et taille de la fin du journal relue.
const WARNINGS: usize = 3;
const LOG_TAIL: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Fonctionne.
    Ok,
    /// À l'arrêt sans que ce soit une panne (Prisme fermé, Discord fermé…).
    Off,
    /// En échec : témoin sur l'icône de notification et la barre principale.
    Error,
}

/// État d'une intégration.
#[derive(Debug, Clone)]
pub struct Check {
    pub id: String,
    pub label: String,
    pub level: Level,
    pub detail: String,
    /// Fait daté (dernier hook reçu, dernière mesure…), affiché « il y a … ».
    pub at: Option<Dated>,
    /// Action proposée : (identifiant pour `status_action`, libellé du bouton).
    pub action: Option<(String, &'static str)>,
}

#[derive(Debug, Clone)]
pub struct Dated {
    pub label: &'static str,
    pub ms: u64,
}

impl Check {
    fn new(id: &str, label: &str, level: Level, detail: impl Into<String>) -> Check {
        Check { id: id.into(), label: label.into(), level, detail: detail.into(), at: None, action: None }
    }

    fn at(mut self, label: &'static str, ms: Option<u64>) -> Check {
        self.at = ms.map(|ms| Dated { label, ms });
        self
    }

    fn action(mut self, id: impl Into<String>, label: &'static str) -> Check {
        self.action = Some((id.into(), label));
        self
    }
}

#[derive(Debug, Clone)]
pub struct StatusView {
    pub checks: Vec<Check>,
    /// Derniers avertissements et erreurs du journal, le plus récent d'abord.
    pub warnings: Vec<String>,
    pub log_path: String,
}

/// Démarrages ratés et témoin envoyé aux fenêtres.
pub struct Health {
    /// Fonctions qui n'ont pas pu démarrer : (nom, erreur).
    starts: Mutex<Vec<(String, String)>>,
    /// Intégrations en échec (libellés).
    failing: Published<Vec<String>>,
    wake: Waker,
}

impl Default for Health {
    fn default() -> Self {
        Health { starts: Mutex::default(), failing: Published::new(EV_HEALTH, Vec::new()), wake: Waker::default() }
    }
}

impl Health {
    /// Démarrage raté d'une fonction (table de démarrage).
    pub fn start_failed(&self, name: &str, error: String) {
        lock(&self.starts).push((name.to_owned(), error));
    }

    fn start_error(&self, name: &str) -> Option<String> {
        lock(&self.starts).iter().find(|(n, _)| n == name).map(|(_, e)| e.clone())
    }
}

/// Démarrages couverts par une ligne à eux (pas répétés dans la liste générale).
const COVERED: [&str; 2] = ["sessions Claude Code", "notifications"];

fn checks(app: &AppHandle) -> Vec<Check> {
    let health = app.state::<Health>();
    let now = horizon_core::time::now_ms();
    let mut out = Vec::new();

    // Hooks de Claude Code.
    let addr = horizon_hook::ADDR;
    out.push(match health.start_error("sessions Claude Code") {
        Some(e) => Check::new("hooks", "Hooks Claude Code", Level::Error, format!("récepteur indisponible sur {addr} : {e}")),
        None => {
            let last = app.state::<crate::widgets::claude_sessions::ClaudeSessions>().last_hook_ms();
            let detail = if last.is_some() { format!("écoute sur {addr}") } else { format!("écoute sur {addr} · aucun reçu depuis le démarrage") };
            Check::new("hooks", "Hooks Claude Code", Level::Ok, detail).at("dernier reçu", last)
        }
    });
    // Autorisations (hook PermissionRequest, ajouté en v4 : un ancien réglage ne l'a pas).
    let claude = app.state::<crate::widgets::claude_sessions::ClaudeSessions>();
    out.push(match claude.last_permission_ms() {
        Some(t) => Check::new("permissions", "Autorisations Claude", Level::Ok, "demandes suivies").at("dernière demande", Some(t)),
        None => Check::new(
            "permissions",
            "Autorisations Claude",
            Level::Off,
            "aucune demande reçue depuis le démarrage (hook PermissionRequest : relancer install-claude-hooks.ps1 s’il manque)",
        ),
    });

    // Mode jeu (plein écran sur l'écran principal).
    let game = crate::state::settings().features.game_mode;
    out.push(match crate::game::since_ms() {
        Some(t) => Check::new("game", "Mode jeu", Level::Ok, "actif : spectre coupé, mesures espacées").at("depuis", Some(t)),
        None if game => Check::new("game", "Mode jeu", Level::Ok, "prêt (plein écran sur l’écran principal)"),
        None => Check::new("game", "Mode jeu", Level::Off, "désactivé (Réglages d’Horizon › Fonctions)"),
    });

    // Discord vocal par le RPC local (StreamKit ou ta propre application).
    let d = app.state::<crate::discord::Discord>();
    let ds = d.status();
    out.push(if d.active() {
        Check::new("discord_rpc", "Discord vocal", Level::Ok, ds)
    } else {
        Check::new("discord_rpc", "Discord vocal", Level::Off, if ds.is_empty() { "démarrage…".to_owned() } else { ds })
    });

    // Applis personnelles (absentes de l’édition partagée).
    if crate::PERSONAL {
        // Relais Discord (plugin KrispBridge).
        let v = crate::widgets::discord_voice::voice_state(app.clone());
        let version = v.rest.get("version").and_then(|x| x.as_str()).map(|s| format!(" · KrispBridge {s}")).unwrap_or_default();
        out.push(if !v.available {
            Check::new("discord", "Relais Discord", Level::Off, "Discord fermé, ou relais absent (port 28642)")
        } else if !v.bridged {
            Check::new("discord", "Relais Discord", Level::Off, format!("connecté, mais le relais ne transmet pas les commandes{version}"))
        } else {
            Check::new("discord", "Relais Discord", Level::Ok, format!("connecté, commandes transmises{version}"))
        });

        // Prisme.
        let l = app.state::<crate::prisme::Prisme>().lighting();
        out.push(if l.connected {
            Check::new("prisme", "Prisme", Level::Ok, "connecté")
        } else {
            Check::new("prisme", "Prisme", Level::Off, "ne tourne pas (éclairage, signaux)").action("prisme", "Lancer Prisme")
        });
    }

    // Notifications Windows.
    out.push(match (health.start_error("notifications"), crate::widgets::notifications::health(app)) {
        (Some(e), _) => Check::new("notifications", "Notifications Windows", Level::Error, format!("démarrage impossible : {e}")),
        (None, (true, _)) => Check::new("notifications", "Notifications Windows", Level::Error, "accès refusé par Windows")
            .action("notifications", "Ouvrir les paramètres"),
        (None, (_, true)) => Check::new("notifications", "Notifications Windows", Level::Error, "lecture impossible, nouvel essai automatique"),
        _ => Check::new("notifications", "Notifications Windows", Level::Ok, "lues"),
    });

    // SharkApp (absent de l’édition partagée).
    if crate::PERSONAL {
        // Batteries de SharkApp.
        let b = crate::widgets::batteries::batteries_state(app);
        let latest = [&b.phone, &b.watch].into_iter().flatten().map(|x| x.at_ms).max();
        let file = crate::widgets::batteries::path().is_some_and(|p| p.exists());
        out.push(match latest {
            _ if !file => Check::new("batteries", "Batteries (SharkApp)", Level::Off, "aucun fichier de SharkApp"),
            Some(t) if now as i64 - t > BATTERY_STALE_MS => {
                Check::new("batteries", "Batteries (SharkApp)", Level::Off, "mesures anciennes : SharkApp ne les met plus à jour")
            }
            Some(_) => Check::new("batteries", "Batteries (SharkApp)", Level::Ok, "à jour"),
            None => Check::new("batteries", "Batteries (SharkApp)", Level::Off, "aucune mesure dans le fichier"),
        }
        .at("dernière mesure", latest.map(|t| t as u64)));

        // Appels du téléphone (SharkApp) : fichier facultatif.
        let phone_file = crate::phone::path().is_some_and(|p| p.exists());
        let updated = crate::phone::last(app).map(|f| f.updated_ms).filter(|&t| t > 0);
        out.push(match updated {
            _ if !phone_file => Check::new("phone", "Appels du téléphone (SharkApp)", Level::Off, "aucun fichier phone.json de SharkApp"),
            Some(t) if now.saturating_sub(t) > horizon_core::phone::STALE_MS => {
                Check::new("phone", "Appels du téléphone (SharkApp)", Level::Ok, "au repos")
            }
            Some(_) => Check::new("phone", "Appels du téléphone (SharkApp)", Level::Ok, "à jour"),
            None => Check::new("phone", "Appels du téléphone (SharkApp)", Level::Off, "fichier illisible ou vide"),
        }
        .at("dernière mise à jour", updated));
    }

    // Quota Claude : une erreur ne compte comme panne que si le widget est affiché.
    let (error, fetched) = crate::widgets::claude_quota::health(app);
    let shown = app.state::<AppState>().read(|l| l.first_of(WidgetKind::CLAUDE_QUOTA).is_some());
    out.push(match error {
        Some(e) => Check::new("quota", "Quota Claude", if shown { Level::Error } else { Level::Off }, e),
        None if fetched.is_some() => Check::new("quota", "Quota Claude", Level::Ok, "lu"),
        None => Check::new("quota", "Quota Claude", Level::Off, "pas encore lu"),
    }
    .at("dernière lecture", fetched));

    // Raccourci de la palette.
    let (hotkey, error) = crate::palette::hotkey_status(app);
    out.push(match error {
        Some(e) => Check::new("hotkey", "Raccourci de la palette", Level::Error, e).action("hotkey", "Changer"),
        None if hotkey.is_empty() => Check::new("hotkey", "Raccourci de la palette", Level::Off, "aucun raccourci"),
        None => Check::new("hotkey", "Raccourci de la palette", Level::Ok, hotkey),
    });

    // Fichiers de réglages.
    let settings = crate::state::app_dir().join("settings.toml");
    out.push(match Settings::inspect(&settings) {
        Ok(SettingsFile::Invalid(why)) => {
            Check::new("settings", "settings.toml", Level::Error, format!("illisible, réglages par défaut : {why}"))
                .action("settings_file", "Montrer le fichier")
        }
        Ok(SettingsFile::Missing) => Check::new("settings", "settings.toml", Level::Ok, "absent : réglages par défaut"),
        Ok(SettingsFile::Valid(_)) => Check::new("settings", "settings.toml", Level::Ok, "valide"),
        Err(e) => Check::new("settings", "settings.toml", Level::Error, format!("lecture impossible : {e}")),
    });
    out.push(if app.state::<AppState>().writable() {
        Check::new("layout", "layout.json", Level::Ok, "lu et enregistré")
    } else {
        Check::new("layout", "layout.json", Level::Error, "illisible au démarrage : jamais écrasé ; corrige-le puis relance Horizon")
            .action("layout_file", "Montrer le fichier")
    });

    // Fils système relancés après une panne (lecture en cours, panneau Audio, micro…).
    for f in horizon_sys::restart::faults() {
        let id = format!("fault:{}", f.what);
        out.push(if f.down {
            Check::new(&id, &capitalized(&f.what), Level::Error, format!("indisponible ({}), nouvel essai automatique", f.error))
        } else {
            let times = if f.count > 1 { format!("{} fois", f.count) } else { "une fois".into() };
            Check::new(&id, &capitalized(&f.what), Level::Ok, format!("relancé après une panne ({times})"))
        }
        .at("dernière panne", Some(f.at_ms)));
    }

    // Démarrages ratés des autres fonctions.
    for (name, e) in lock(&health.starts).iter().filter(|(n, _)| !COVERED.contains(&n.as_str())) {
        out.push(Check::new(&format!("start:{name}"), &capitalized(name), Level::Error, format!("démarrage impossible : {e}")));
    }

    // Mise à jour automatique (versions publiées seulement).
    out.push(update_check(app));

    // Plugins.
    out.extend(plugin_checks(app));

    // Présence (pause pendant une absence).
    out.push(if horizon_sys::presence::away() {
        Check::new("presence", "Présence", Level::Off, "absent (session verrouillée ou écran éteint) : en pause")
    } else {
        Check::new("presence", "Présence", Level::Ok, "présent")
    });
    out
}

/// Une ligne par plugin (état, coût, accord à donner), et le dossier des plugins.
fn plugin_checks(app: &AppHandle) -> Vec<Check> {
    use crate::plugins::{Disabled, PluginState, Plugins};
    let plugins = app.state::<Plugins>();
    let mut out = Vec::new();
    for p in plugins.list() {
        let name = p.manifest.as_ref().map_or_else(|| p.id.clone(), |m| m.name.clone());
        let label = format!("Plugin : {name}");
        let version = p.manifest.as_ref().map(|m| format!("{} · ", m.version)).unwrap_or_default();
        let id = format!("plugin:{}", p.id);
        let c = match &p.state {
            PluginState::Ok => {
                let load = plugins.load(&p.id) * 100.0;
                let mut detail = format!("{version}actif · {load:.1} % d’un cœur");
                if let Some(e) = &p.error {
                    detail.push_str(&format!(" · nouvelle version refusée (l’ancienne reste) : {e}"));
                } else if let Some(crash) = plugins.last_crash(&p.id) {
                    detail.push_str(&format!(" · dernière erreur du script : {crash}"));
                }
                let level = if p.error.is_some() { Level::Error } else { Level::Ok };
                Check::new(&id, &label, level, detail).action(format!("plugin_disable:{}", p.id), "Désactiver")
            }
            PluginState::NeedsConsent(asks) => Check::new(&id, &label, Level::Off, format!("{version}attend ton accord : {}", asks.join(" ; ")))
                .action(format!("plugin_accept:{}", p.id), "Autoriser"),
            PluginState::Disabled(Disabled::User) => {
                Check::new(&id, &label, Level::Off, format!("{version}désactivé")).action(format!("plugin_enable:{}", p.id), "Activer")
            }
            PluginState::Disabled(Disabled::TooHeavy) => Check::new(&id, &label, Level::Error, format!("{version}désactivé : trop gourmand (plus de 2 % d’un cœur)"))
                .action(format!("plugin_enable:{}", p.id), "Réactiver"),
            PluginState::Disabled(Disabled::Crashing) => {
                let crash = plugins.last_crash(&p.id).unwrap_or_default();
                Check::new(&id, &label, Level::Error, format!("{version}désactivé : erreurs répétées du script · {crash}"))
                    .action(format!("plugin_enable:{}", p.id), "Réactiver")
            }
            PluginState::Broken => Check::new(&id, &label, Level::Error, p.error.clone().unwrap_or_else(|| "invalide".into()))
                .action(format!("plugin_folder:{}", p.id), "Ouvrir le dossier"),
        };
        out.push(c);
    }
    let dir = crate::plugins::plugins_dir();
    let count = out.len();
    let detail = if count == 0 { format!("aucun plugin dans {}", dir.display()) } else { format!("{count} dans {}", dir.display()) };
    out.push(Check::new("plugins", "Plugins", if count == 0 { Level::Off } else { Level::Ok }, detail).action("plugins_dir", "Ouvrir le dossier"));
    out.push(
        Check::new("plugin_new", "Nouveau plugin", Level::Ok, "un modèle à modifier, avec son guide (PLUGINS.md), posé sur la barre principale")
            .action("plugin_new", "Créer"),
    );
    out
}

fn update_check(app: &AppHandle) -> Check {
    use crate::update::Status;
    const L: &str = "Mises à jour";
    let version = crate::update::current();
    if !crate::update::ENABLED {
        return Check::new("update", L, Level::Off, format!("{version}, compilée depuis les sources : mise à jour par git"));
    }
    let u = app.state::<crate::update::Updater>();
    let auto = crate::state::settings().features.auto_update;
    let c = match u.status() {
        Status::Ready { version: v, .. } => {
            return Check::new("update", L, Level::Ok, format!("{v} prête, installée au prochain démarrage"))
                .action("update_now", "Installer et redémarrer");
        }
        Status::Downloading(v) => Check::new("update", L, Level::Ok, format!("téléchargement de {v}…")),
        Status::Checking => Check::new("update", L, Level::Ok, "recherche en cours…"),
        Status::Failed(e) => Check::new("update", L, Level::Off, format!("{version} · dernière recherche ratée : {e}")),
        Status::UpToDate => Check::new("update", L, Level::Ok, format!("{version}, à jour")),
        Status::Idle if auto => Check::new("update", L, Level::Ok, format!("{version} · recherche une minute après le démarrage")),
        Status::Idle => Check::new("update", L, Level::Off, format!("{version} · automatique coupée (Réglages d’Horizon › Fonctions)")),
    };
    c.at("dernière recherche", u.checked_ms()).action("update_check", "Rechercher")
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Derniers avertissements et erreurs de `log` (fin du journal), le plus récent d'abord.
fn last_warnings(log: &str, n: usize) -> Vec<String> {
    log.lines().rev().filter(|l| l.contains(" [WARN] ") || l.contains(" [ERROR] ")).take(n).map(str::to_owned).collect()
}

fn log_path() -> std::path::PathBuf {
    crate::journal::path()
}

/// Fin du journal (au plus `LOG_TAIL` octets).
fn log_tail() -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log_path()) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(LOG_TAIL)));
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Libellés des intégrations en échec.
fn failing(checks: &[Check]) -> Vec<String> {
    checks.iter().filter(|c| c.level == Level::Error).map(|c| c.label.clone()).collect()
}

/// Relit les états et met le témoin à jour.
fn refresh(app: &AppHandle) {
    publish(app, &checks(app));
}

/// Met le témoin à jour d'après `checks`.
fn publish(app: &AppHandle, checks: &[Check]) {
    let failing = failing(checks);
    let health = app.state::<Health>();
    if health.failing.set(app, failing.clone()) {
        if !failing.is_empty() {
            log::warn!("intégrations en échec : {}", failing.join(", "));
        }
        crate::tray::set_health(app, &failing);
    }
}

/// Relecture tout de suite (réglages enregistrés, raccourci changé).
pub fn wake(app: &AppHandle) {
    app.state::<Health>().wake.wake();
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("etat", move || {
        loop {
            horizon_sys::presence::wait_while_away();
            refresh(&app);
            app.state::<Health>().wake.wait(PERIOD);
        }
    })
}

/// Hors du fil de l'interface : le journal est relu.
pub fn status_get(app: &AppHandle) -> StatusView {
    let checks = checks(app);
    publish(app, &checks);
    StatusView {
        checks,
        warnings: last_warnings(&log_tail(), WARNINGS),
        log_path: log_path().display().to_string(),
    }
}

/// Intégrations en échec (témoin de la barre principale).
pub fn status_health(app: &AppHandle) -> Vec<String> {
    app.state::<Health>().failing.get()
}

/// Ouvre le journal dans l'éditeur de texte par défaut.
pub fn open_log() {
    horizon_sys::launch::launch(&log_path().display().to_string());
}

/// Action d'une ligne de la page « État ».
pub fn status_action(app: &AppHandle, id: &str) -> Result<(), String> {
    match id {
        "prisme" => {
            let target = horizon_sys::launch::find_app("Prisme").ok_or("Prisme introuvable dans le menu Démarrer")?;
            horizon_sys::launch::launch(&target);
        }
        "notifications" => horizon_sys::launch::launch("ms-settings:privacy-notifications"),
        "settings_file" => horizon_sys::launch::reveal(&crate::state::app_dir().join("settings.toml").display().to_string()),
        "layout_file" => horizon_sys::launch::reveal(&app.state::<AppState>().layout_path.display().to_string()),
        "update_check" => crate::update::check_now(app),
        "update_now" => crate::update::install_now(app)?,
        "plugins_dir" => horizon_sys::launch::launch(&crate::plugins::plugins_dir().display().to_string()),
        "plugin_new" => crate::plugins::template::new_plugin(app)?,
        other => {
            let plugins = app.state::<crate::plugins::Plugins>();
            match other.split_once(':') {
                Some(("plugin_accept", p)) => plugins.accept(app, p),
                Some(("plugin_enable", p)) => plugins.set_disabled(app, p, None),
                Some(("plugin_disable", p)) => plugins.set_disabled(app, p, Some(crate::plugins::Disabled::User)),
                Some(("plugin_folder", p)) => {
                    let dir = plugins.list().into_iter().find(|i| i.id == p).map(|i| i.dir).ok_or("plugin introuvable")?;
                    horizon_sys::launch::launch(&dir.display().to_string());
                }
                _ => return Err(format!("action inconnue : {id}")),
            }
        }
    }
    wake(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_errors_light_the_witness() {
        let c = |label: &str, level| Check::new(label, label, level, "");
        let list = [c("Prisme", Level::Off), c("Quota Claude", Level::Error), c("settings.toml", Level::Ok), c("Lecture", Level::Error)];
        assert_eq!(failing(&list), ["Quota Claude", "Lecture"]);
        assert_eq!(capitalized("lecture en cours"), "Lecture en cours");
    }

    #[test]
    fn warnings_are_the_last_ones_first() {
        let log = "10:00:00 [INFO] a\n10:00:01 [WARN] b\n10:00:02 [ERROR] c\n10:00:03 [INFO] d\n10:00:04 [WARN] e\n10:00:05 [WARN] f\n";
        assert_eq!(last_warnings(log, 3), vec!["10:00:05 [WARN] f", "10:00:04 [WARN] e", "10:00:02 [ERROR] c"]);
        assert!(last_warnings("", 3).is_empty());
        assert_eq!(capitalized("lecture en cours"), "Lecture en cours");
    }
}
