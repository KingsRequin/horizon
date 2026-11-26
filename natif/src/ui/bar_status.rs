//! Témoins de la barre principale (`ui/bar-status.slint`, lib/BarStatus.svelte) : profil actif
//! autre que le premier (lu dans `layout.json`), « ! » tant qu'une intégration est en échec
//! (`status.rs`, `horizon://health`), clavier tant qu'un téléphone saisit (`horizon://phone-input`).
//! Clic : page des réglages (« État », profils) ; sur le clavier : couper.
//! Fil de l'interface.

use super::{BarStatusData, BarWindow};
use crate::events::{EV_HEALTH, EV_PROFILES};
use crate::host::{self, AppHandle, Manager};
use horizon_core::layout::Layout;
use slint::ComponentHandle;

/// Profil à signaler : l'actif s'il n'est pas le premier (celui de tous les jours), et s'il a
/// été activé par une appli.
fn custom_profile(l: &Layout) -> Option<(String, bool)> {
    let active = l.profiles.iter().find(|p| p.id == l.active_profile)?;
    (l.profiles.first().is_some_and(|first| first.id != active.id)).then(|| (active.name.clone(), l.auto_from.is_some()))
}

/// Relie les témoins de la barre principale `label`.
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str) {
    let g = window.global::<BarStatusData>();
    let (profile, auto) = app.state::<crate::state::AppState>().read(custom_profile).unwrap_or_default();
    g.set_profile(profile.into());
    g.set_profile_auto(auto);
    g.set_failing(crate::status::status_health(app).join(", ").into());
    g.set_input(crate::widgets::phone::witness::active_names(app).join(", ").into());
    let w = window.as_weak();
    host::listen(crate::widgets::phone::witness::EV_PHONE_INPUT, Some(label), move |e| {
        if let (Some(names), Some(w)) = (e.payload::<Vec<String>>(), w.upgrade()) {
            w.global::<BarStatusData>().set_input(names.join(", ").into());
        }
    });
    let a = app.clone();
    g.on_cut_input(move || crate::widgets::phone::witness::cut_all(&a));
    let w = window.as_weak();
    host::listen(EV_HEALTH, Some(label), move |e| {
        if let (Some(f), Some(w)) = (e.payload::<Vec<String>>(), w.upgrade()) {
            w.global::<BarStatusData>().set_failing(f.join(", ").into());
        }
    });
    // Profil changé (menu, appli au premier plan) : le témoin suit.
    let (a, w) = (app.clone(), window.as_weak());
    host::listen(EV_PROFILES, Some(label), move |_| {
        let Some(w) = w.upgrade() else { return };
        let (profile, auto) = a.state::<crate::state::AppState>().read(custom_profile).unwrap_or_default();
        let g = w.global::<BarStatusData>();
        g.set_profile(profile.into());
        g.set_profile_auto(auto);
    });
    let a = app.clone();
    g.on_open(move |page| {
        if let Err(e) = crate::settings_window::settings_open(&a, &page) {
            log::warn!("témoin de la barre : {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_profile_other_than_the_first_is_shown() {
        let mut l: Layout = serde_json::from_value(serde_json::json!({
            "version": 1,
            "bars": [],
            "profiles": [{ "id": "normal", "name": "Normal" }, { "id": "jeu", "name": "Jeu" }],
            "active_profile": "normal",
        }))
        .unwrap();
        assert_eq!(custom_profile(&l), None);
        l.active_profile = "jeu".into();
        assert_eq!(custom_profile(&l), Some(("Jeu".into(), false)));
        l.auto_from = Some("Steam".into());
        assert_eq!(custom_profile(&l), Some(("Jeu".into(), true)));
    }
}
