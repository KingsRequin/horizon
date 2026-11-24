//! Modules intégrés : fonctions qui restent dans l'exe (elles demandent plus que ce qu'un plugin
//! peut faire) mais ne tournent que si l'utilisateur les veut. Page *Plugins* des réglages,
//! rubrique « Modules intégrés » : Activer / Désactiver.
//!
//! Module éteint : son widget n'existe pas (ni sur les barres, ni dans le menu « + », ni dans les
//! réglages : `widgets::manifest` ne le connaît plus) et son service n'est pas démarré. Activé en
//! cours de route, le service démarre aussitôt ; désactivé, il cesse d'agir tout de suite et ses
//! fils ne sont plus lancés au démarrage suivant.

use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use crate::util::lock;
use horizon_core::layout::BarLayout;
use std::sync::Mutex;

/// Un module : son identifiant est celui de son widget (`ui/widgets/<id>/`) et de son `Backend`.
pub struct Module {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
}

pub const MODULES: &[Module] = &[
    Module {
        id: "claude_sessions",
        name: "Sessions Claude Code",
        description: "Sessions de Claude Code sur la barre, « Claude attend » dans l’îlot, consommation du jour, captures d’écran proposées à la session qui attend.",
        icon: "terminalWindow",
    },
    Module {
        id: "phone",
        name: "Téléphone",
        description: "Batterie du téléphone sur la barre, envoyée par l’appli Horizon Mobile, appairée par QR code. Horizon écoute alors sur le réseau local.",
        icon: "deviceMobile",
    },
];

/// Modules activés (posé par [`resolve`] au démarrage, puis par [`set`]).
static ENABLED: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn module(id: &str) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.id == id)
}

/// Le widget `id` est disponible : ce n'est pas un module, ou son module est activé.
pub fn enabled(id: &str) -> bool {
    module(id).is_none() || lock(&ENABLED).contains(&id)
}

/// Choix d'un module : celui de l'utilisateur s'il en a fait un, sinon activé seulement s'il
/// sert déjà (son widget est sur une barre, ou ce qui l'alimente est installé).
fn decide(chosen: Option<bool>, in_use: bool) -> bool {
    chosen.unwrap_or(in_use)
}

/// Choix enregistré de l'utilisateur pour le module `id` (`[features]` de settings.toml).
fn chosen(id: &str) -> Option<bool> {
    let f = crate::state::settings().features;
    match id {
        "claude_sessions" => f.claude_sessions,
        "phone" => f.phone,
        _ => None,
    }
}

/// Le module `id` sert déjà : un de ses widgets est sur une barre (tous profils), les hooks
/// d'Horizon sont dans les réglages de Claude Code, ou un téléphone est appairé.
fn in_use(app: &AppHandle, id: &str) -> bool {
    let on_a_bar = app.state::<AppState>().read(|l| {
        l.bars.iter().chain(l.profiles.iter().flat_map(|p| p.bars.iter())).flat_map(BarLayout::widgets).any(|w| w.kind.as_str() == id)
    });
    on_a_bar || (id == "claude_sessions" && claude_hooks_installed()) || (id == "phone" && crate::widgets::phone::paired_on_disk())
}

/// Les hooks d'Horizon figurent dans `settings.json` de Claude Code (posés par l'installateur).
fn claude_hooks_installed() -> bool {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(".claude")));
    dir.and_then(|d| std::fs::read_to_string(d.join("settings.json")).ok()).is_some_and(|text| text.contains("horizon-hook"))
}

/// Au démarrage, avant les barres et les services : quels modules sont activés.
pub fn resolve(app: &AppHandle) {
    let on: Vec<&'static str> = MODULES.iter().filter(|m| decide(chosen(m.id), in_use(app, m.id))).map(|m| m.id).collect();
    for m in MODULES {
        log::info!("module {} : {}", m.id, if on.contains(&m.id) { "activé" } else { "désactivé" });
    }
    *lock(&ENABLED) = on;
}

/// Active ou désactive le module `id` (fil de l'interface) : choix enregistré, service démarré
/// ou mis au repos, barres mises à jour.
pub fn set(app: &AppHandle, id: &str, on: bool) -> Result<(), String> {
    let m = module(id).ok_or_else(|| format!("module inconnu : {id}"))?;
    let mut s = crate::state::settings();
    match m.id {
        "claude_sessions" => s.features.claude_sessions = Some(on),
        "phone" => s.features.phone = Some(on),
        _ => return Err(format!("module sans réglage : {id}")),
    }
    crate::options::options_set(app, s)?;
    {
        let mut enabled = lock(&ENABLED);
        enabled.retain(|e| *e != m.id);
        if on {
            enabled.push(m.id);
        }
    }
    log::info!("module {} : {}", m.id, if on { "activé" } else { "désactivé" });
    if on {
        crate::widgets::start_one(app, m.id);
    } else if m.id == "claude_sessions" {
        crate::widgets::claude_sessions::clear(app);
    }
    if m.id == "phone" {
        // Le service ne démarre qu'une fois : c'est lui qui suit l'état du module ensuite
        // (serveur lancé ou arrêté, batteries retirées des barres).
        crate::widgets::phone::refresh(app);
    }
    crate::ui::bar::plugins_changed(app);
    crate::status::wake(app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_follows_the_choice_then_its_use() {
        assert!(decide(Some(true), false));
        assert!(!decide(Some(false), true), "désactivé à la main : même si son widget est resté sur une barre");
        assert!(decide(None, true), "jamais choisi : activé s'il sert déjà (barres existantes)");
        assert!(!decide(None, false), "installation neuve : éteint");
    }

    #[test]
    fn only_modules_can_be_off() {
        assert!(enabled("clock") && enabled("plugin:prisme"));
        assert!(module("claude_sessions").is_some() && module("phone").is_some());
        assert!(MODULES.iter().all(|m| crate::widgets::MANIFESTS.iter().any(|w| w.id == m.id)), "un module est un widget d'Horizon");
    }
}
