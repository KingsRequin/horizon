//! Tuiles de la page « Plugins » des réglages (`ui/options/plugins.slint`) : une par plugin
//! installé, d'après le registre (`plugins::Plugins::list`), avec son état, son coût ou son
//! erreur et l'action à proposer, puis une par plugin de la bibliothèque (`plugins::library`)
//! pas encore installé. Les actions du registre passent par `status::status_action`
//! (`plugin_accept:<id>`, `plugin_enable:<id>`, `plugin_disable:<id>`, `plugin_folder:<id>`) ;
//! celles de la bibliothèque (`plugin_install:<id>`, `plugin_update:<id>`, `plugin_remove:<id>`)
//! téléchargent ou effacent : elles tournent hors du fil de l'interface ([`run`]).

use super::PluginTile;
use crate::host::{AppHandle, Manager};
use crate::plugins::library::{self, Tile, TileState};
use crate::plugins::{Disabled, PluginInfo, PluginState, Plugins};
use crate::util::lock;
use std::sync::Mutex;

/// Plugins en cours d'installation, de mise à jour ou de retrait.
static BUSY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Ce que montre la tuile d'un plugin (sans l'icône, lue sur le fil de l'interface).
#[derive(Debug, Clone, PartialEq)]
pub struct TileText {
    pub chip: &'static str,
    /// 0 : fonctionne, 1 : neutre, 2 : en échec, 3 : demande une action.
    pub level: i32,
    pub detail: String,
    /// (action, libellé du bouton).
    pub action: Option<(String, &'static str)>,
}

/// Texte de la tuile d'un plugin installé : `load` part d'un cœur (0 à 1), `crash` dernière
/// erreur du script, `update` version plus récente de la bibliothèque.
pub fn tile_text(p: &PluginInfo, load: f32, crash: Option<String>, update: Option<&str>) -> TileText {
    let id = &p.id;
    let t = |chip, level, detail: String, action: Option<(String, &'static str)>| TileText { chip, level, detail, action };
    match &p.state {
        PluginState::Ok => {
            let (level, detail) = match (&p.error, crash) {
                (Some(e), _) => (2, format!("Nouvelle version refusée (l’ancienne reste) : {e}")),
                (None, Some(c)) => (0, format!("Dernière erreur du script : {c}")),
                (None, None) => (0, format!("{:.1} % d’un cœur", load * 100.0).replace('.', ",")),
            };
            match update {
                Some(v) => t("Mise à jour", 3, format!("Version {v} disponible"), Some((format!("plugin_update:{id}"), "Mettre à jour"))),
                None => t("Actif", level, detail, Some((format!("plugin_disable:{id}"), "Désactiver"))),
            }
        }
        PluginState::NeedsConsent(asks) => t("À autoriser", 3, format!("Demande : {}", asks.join(" ; ")), Some((format!("plugin_accept:{id}"), "Autoriser"))),
        PluginState::Disabled(Disabled::User) => t("Désactivé", 1, "Coupé à la main".into(), Some((format!("plugin_enable:{id}"), "Activer"))),
        PluginState::Disabled(Disabled::TooHeavy) => {
            t("Coupé", 2, "Trop gourmand : plus de 2 % d’un cœur".into(), Some((format!("plugin_enable:{id}"), "Réactiver")))
        }
        PluginState::Disabled(Disabled::Crashing) => {
            let detail = match crash {
                Some(c) => format!("Erreurs répétées du script : {c}"),
                None => "Erreurs répétées du script".into(),
            };
            t("Coupé", 2, detail, Some((format!("plugin_enable:{id}"), "Réactiver")))
        }
        PluginState::Broken => t("Invalide", 2, p.error.clone().unwrap_or_else(|| "Plugin invalide".into()), None),
    }
}

/// Texte de la tuile d'un plugin de la bibliothèque pas encore installé : ce qu'il demande,
/// montré avant l'installation.
pub fn library_text(t: &Tile) -> TileText {
    let detail = if t.permissions.is_empty() { "Aucune permission demandée".to_owned() } else { format!("Demande : {}", t.permissions.join(" ; ")) };
    TileText { chip: "", level: 1, detail, action: Some((format!("plugin_install:{}", t.id), "Installer")) }
}

/// « 1.2.0 · KingsRequin · 14 Ko ».
fn meta(version: Option<&str>, author: Option<&str>, size: Option<u64>) -> String {
    let size = size.map(|b| if b < 1024 * 1024 { format!("{} Ko", b.div_ceil(1024)) } else { format!("{:.1} Mo", b as f64 / 1048576.0).replace('.', ",") });
    [version.map(str::to_owned), author.map(str::to_owned), size].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

/// Tuiles des plugins installés puis de ceux de la bibliothèque, chacune dans l'ordre des noms ;
/// `true` si l'index de la bibliothèque est connu.
pub fn tiles(app: &AppHandle) -> (Vec<PluginTile>, Vec<PluginTile>, bool) {
    let plugins = app.state::<Plugins>();
    let library = library::tiles(app);
    let known = library.iter().any(|t| t.available.is_some());
    let busy = lock(&BUSY).clone();
    let by_name = |a: &PluginTile, b: &PluginTile| a.name.to_lowercase().cmp(&b.name.to_lowercase());

    let mut installed: Vec<PluginTile> = plugins
        .list()
        .iter()
        .map(|p| {
            let lib = library.iter().find(|t| t.id == p.id);
            let update = lib.filter(|t| t.state == TileState::Update).and_then(|t| t.available.as_deref());
            let mut text = tile_text(p, plugins.load(&p.id), plugins.last_crash(&p.id), update);
            if busy.contains(&p.id) {
                text = TileText { chip: "En cours…", level: 1, detail: text.detail, action: None };
            }
            let m = p.manifest.as_deref();
            // Seul un plugin venu de la bibliothèque se retire d'ici : celui qu'on écrit soi-même
            // ne s'efface pas d'un clic.
            let removable = plugins.from_library(&p.id) && !busy.contains(&p.id);
            tile(
                &p.id,
                m.map_or_else(|| p.id.clone(), |m| m.name.clone()),
                meta(m.map(|m| m.version.as_str()), m.and_then(|m| m.author.as_deref()), None),
                m.and_then(|m| m.description.clone()),
                icon(p.manifest.as_ref().and_then(|m| m.icon.as_deref()), Some(p)),
                text,
                format!("plugin_folder:{}", p.id),
                if removable { format!("plugin_remove:{}", p.id) } else { String::new() },
            )
        })
        .collect();
    installed.sort_by(by_name);

    let mut available: Vec<PluginTile> = library
        .iter()
        .filter(|t| t.state == TileState::Available)
        .map(|t| {
            let mut text = library_text(t);
            if busy.contains(&t.id) {
                text = TileText { chip: "Installation…", level: 1, detail: text.detail, action: None };
            }
            tile(
                &t.id,
                t.name.clone(),
                meta(t.available.as_deref(), t.author.as_deref(), t.size),
                t.description.clone(),
                icon(Some(&t.icon), None),
                text,
                String::new(),
                String::new(),
            )
        })
        .collect();
    available.sort_by(by_name);
    (installed, available, known)
}

#[allow(clippy::too_many_arguments)]
fn tile(id: &str, name: String, meta: String, description: Option<String>, icon: slint::Image, text: TileText, folder: String, remove: String) -> PluginTile {
    let (action_id, action_label) = text.action.unwrap_or_default();
    PluginTile {
        id: id.into(),
        name: name.into(),
        meta: meta.into(),
        description: description.unwrap_or_default().into(),
        icon,
        chip: text.chip.into(),
        level: text.level,
        detail: text.detail.into(),
        action_id: action_id.into(),
        action_label: action_label.into(),
        folder_id: folder.into(),
        remove_id: remove.into(),
    }
}

/// Icône nommée (catalogue), sinon `icon.svg` du dossier du plugin, sinon celle des widgets.
fn icon(name: Option<&str>, p: Option<&PluginInfo>) -> slint::Image {
    if let Some(name) = name.and_then(super::icons::static_name) {
        return super::icons::image(name);
    }
    if let Some(p) = p {
        let svg = p.dir.join("icon.svg");
        if svg.is_file()
            && let Ok(image) = slint::Image::load_from_path(&svg)
        {
            return image;
        }
    }
    super::icons::image("squaresFour")
}

/// Action de la bibliothèque (`plugin_install:<id>`, `plugin_update:<id>`, `plugin_remove:<id>`) :
/// lancée hors du fil de l'interface, `done` y est rappelée avec le résultat. `false` : ce n'en
/// est pas une.
pub fn run(app: &AppHandle, action: &str, done: impl FnOnce(Result<(), String>) + Send + 'static) -> bool {
    let job: fn(&AppHandle, &str) -> Result<(), String> = match action.split_once(':') {
        Some(("plugin_install", _)) => library::install,
        Some(("plugin_update", _)) => library::update,
        Some(("plugin_remove", _)) => library::remove,
        _ => return false,
    };
    let id = action.split_once(':').map(|(_, id)| id.to_owned()).unwrap_or_default();
    {
        let mut busy = lock(&BUSY);
        if busy.contains(&id) {
            return true;
        }
        busy.push(id.clone());
    }
    let a = app.clone();
    let started = crate::util::start_thread("plugins-action", move || {
        let result = job(&a, &id);
        lock(&BUSY).retain(|b| *b != id);
        let _ = slint::invoke_from_event_loop(move || done(result));
    });
    if let Err(e) = started {
        log::warn!("{e:#}");
        lock(&BUSY).clear();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plugin(state: PluginState, error: Option<&str>) -> PluginInfo {
        PluginInfo { id: "compteur".into(), manifest: None, dir: PathBuf::new(), state, error: error.map(str::to_owned) }
    }

    #[test]
    fn a_running_plugin_shows_its_cost_and_can_be_turned_off() {
        let t = tile_text(&plugin(PluginState::Ok, None), 0.0034, None, None);
        assert_eq!((t.chip, t.level), ("Actif", 0));
        assert_eq!(t.detail, "0,3 % d’un cœur");
        assert_eq!(t.action, Some(("plugin_disable:compteur".into(), "Désactiver")));
    }

    #[test]
    fn a_newer_version_is_offered() {
        let t = tile_text(&plugin(PluginState::Ok, None), 0.0, None, Some("1.3.0"));
        assert_eq!((t.chip, t.level), ("Mise à jour", 3));
        assert_eq!(t.detail, "Version 1.3.0 disponible");
        assert_eq!(t.action.map(|a| a.0), Some("plugin_update:compteur".into()));
    }

    #[test]
    fn a_refused_update_is_an_error() {
        let t = tile_text(&plugin(PluginState::Ok, Some("widget.slint:3")), 0.0, Some("boum".into()), None);
        assert_eq!(t.level, 2);
        assert!(t.detail.contains("widget.slint:3"));
    }

    #[test]
    fn consent_lists_what_is_asked() {
        let asks = vec!["Lire le fichier a.txt".to_owned(), "Joindre example.org".to_owned()];
        let t = tile_text(&plugin(PluginState::NeedsConsent(asks), None), 0.0, None, None);
        assert_eq!((t.chip, t.level), ("À autoriser", 3));
        assert_eq!(t.detail, "Demande : Lire le fichier a.txt ; Joindre example.org");
        assert_eq!(t.action.map(|a| a.0), Some("plugin_accept:compteur".into()));
    }

    #[test]
    fn disabled_plugins_can_come_back() {
        for (d, label) in [(Disabled::User, "Activer"), (Disabled::TooHeavy, "Réactiver"), (Disabled::Crashing, "Réactiver")] {
            let t = tile_text(&plugin(PluginState::Disabled(d), None), 0.0, None, None);
            assert_eq!(t.action, Some(("plugin_enable:compteur".into(), label)));
        }
        assert_eq!(tile_text(&plugin(PluginState::Broken, Some("plugin.json, id : …")), 0.0, None, None).action, None);
    }

    #[test]
    fn a_library_plugin_shows_what_it_asks_before_install() {
        let mut t = Tile {
            id: "meteo".into(),
            name: "Météo".into(),
            author: None,
            description: None,
            installed: None,
            available: Some("1.0.0".into()),
            icon: "cloud".into(),
            permissions: vec![],
            state: TileState::Available,
            size: Some(2048),
            error: None,
            load: 0.0,
        };
        assert_eq!(library_text(&t).detail, "Aucune permission demandée");
        t.permissions = vec!["Joindre api.open-meteo.com".into()];
        let text = library_text(&t);
        assert_eq!(text.detail, "Demande : Joindre api.open-meteo.com");
        assert_eq!(text.action, Some(("plugin_install:meteo".into(), "Installer")));
        assert_eq!(meta(Some("1.0.0"), Some("KR"), Some(2048)), "1.0.0 · KR · 2 Ko");
        assert_eq!(meta(Some("1.0.0"), None, Some(3 * 1048576 / 2)), "1.0.0 · 1,5 Mo");
    }
}
