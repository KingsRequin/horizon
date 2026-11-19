//! Tuiles de la page « Plugins » des réglages (`ui/options/plugins.slint`) : une par plugin
//! installé, d'après le registre (`plugins::Plugins::list`), avec son état, son coût ou son
//! erreur et l'action à proposer. Les actions passent par `status::status_action`
//! (`plugin_accept:<id>`, `plugin_enable:<id>`, `plugin_disable:<id>`, `plugin_folder:<id>`).
//!
//! La bibliothèque (plugins publiés sur GitHub, `plugins::library`) s'y ajoutera.

use super::PluginTile;
use crate::host::{AppHandle, Manager};
use crate::plugins::{Disabled, PluginInfo, PluginState, Plugins};

/// Ce que montre la tuile d'un plugin installé (sans l'icône, lue sur le fil de l'interface).
#[derive(Debug, Clone, PartialEq)]
pub struct TileText {
    pub chip: &'static str,
    /// 0 : fonctionne, 1 : neutre, 2 : en échec, 3 : demande une action.
    pub level: i32,
    pub detail: String,
    /// (action, libellé du bouton).
    pub action: Option<(String, &'static str)>,
}

/// Texte de la tuile d'un plugin : `load` part d'un cœur (0 à 1), `crash` dernière erreur du
/// script.
pub fn tile_text(p: &PluginInfo, load: f32, crash: Option<String>) -> TileText {
    let id = &p.id;
    let t = |chip, level, detail: String, action: Option<(String, &'static str)>| TileText { chip, level, detail, action };
    match &p.state {
        PluginState::Ok => {
            let (level, detail) = match (&p.error, crash) {
                (Some(e), _) => (2, format!("Nouvelle version refusée (l’ancienne reste) : {e}")),
                (None, Some(c)) => (0, format!("Dernière erreur du script : {c}")),
                (None, None) => (0, format!("{:.1} % d’un cœur", load * 100.0).replace('.', ",")),
            };
            t("Actif", level, detail, Some((format!("plugin_disable:{id}"), "Désactiver")))
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

/// Tuiles des plugins installés, dans l'ordre de leur nom.
pub fn tiles(app: &AppHandle) -> Vec<PluginTile> {
    let plugins = app.state::<Plugins>();
    let mut list = plugins.list();
    list.sort_by_key(|p| p.manifest.as_ref().map_or_else(|| p.id.to_lowercase(), |m| m.name.to_lowercase()));
    list.iter()
        .map(|p| {
            let text = tile_text(p, plugins.load(&p.id), plugins.last_crash(&p.id));
            let m = p.manifest.as_deref();
            let meta = match m {
                Some(m) => match &m.author {
                    Some(a) => format!("{} · {a}", m.version),
                    None => m.version.clone(),
                },
                None => String::new(),
            };
            let (action_id, action_label) = text.action.unwrap_or_default();
            PluginTile {
                id: p.id.clone().into(),
                name: m.map_or_else(|| p.id.clone(), |m| m.name.clone()).into(),
                meta: meta.into(),
                description: m.and_then(|m| m.description.clone()).unwrap_or_default().into(),
                icon: icon(p),
                chip: text.chip.into(),
                level: text.level,
                detail: text.detail.into(),
                action_id: action_id.into(),
                action_label: action_label.into(),
                folder_id: format!("plugin_folder:{}", p.id).into(),
            }
        })
        .collect()
}

/// Icône du manifeste, sinon `icon.svg` du dossier, sinon celle des widgets.
fn icon(p: &PluginInfo) -> slint::Image {
    if let Some(name) = p.manifest.as_ref().and_then(|m| m.icon.as_deref()).and_then(super::icons::static_name) {
        return super::icons::image(name);
    }
    let svg = p.dir.join("icon.svg");
    if svg.is_file()
        && let Ok(image) = slint::Image::load_from_path(&svg)
    {
        return image;
    }
    super::icons::image("squaresFour")
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
        let t = tile_text(&plugin(PluginState::Ok, None), 0.0034, None);
        assert_eq!((t.chip, t.level), ("Actif", 0));
        assert_eq!(t.detail, "0,3 % d’un cœur");
        assert_eq!(t.action, Some(("plugin_disable:compteur".into(), "Désactiver")));
    }

    #[test]
    fn a_refused_update_is_an_error() {
        let t = tile_text(&plugin(PluginState::Ok, Some("widget.slint:3")), 0.0, Some("boum".into()));
        assert_eq!(t.level, 2);
        assert!(t.detail.contains("widget.slint:3"));
    }

    #[test]
    fn consent_lists_what_is_asked() {
        let asks = vec!["Lire le fichier a.txt".to_owned(), "Joindre example.org".to_owned()];
        let t = tile_text(&plugin(PluginState::NeedsConsent(asks), None), 0.0, None);
        assert_eq!((t.chip, t.level), ("À autoriser", 3));
        assert_eq!(t.detail, "Demande : Lire le fichier a.txt ; Joindre example.org");
        assert_eq!(t.action.map(|a| a.0), Some("plugin_accept:compteur".into()));
    }

    #[test]
    fn disabled_plugins_can_come_back() {
        for (d, label) in [(Disabled::User, "Activer"), (Disabled::TooHeavy, "Réactiver"), (Disabled::Crashing, "Réactiver")] {
            let t = tile_text(&plugin(PluginState::Disabled(d), None), 0.0, None);
            assert_eq!(t.action, Some(("plugin_enable:compteur".into(), label)));
        }
        assert_eq!(tile_text(&plugin(PluginState::Broken, Some("plugin.json, id : …")), 0.0, None).action, None);
    }
}
