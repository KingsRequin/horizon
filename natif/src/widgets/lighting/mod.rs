//! Widget Éclairage (Prisme) (`ui/widgets/lighting/widget.slint`) : sans service propre, il
//! montre l'état que relaie le lien avec Prisme (`crate::prisme`, au cœur) : scène en cours,
//! éteint, Prisme absent (`LightingData`).

mod panel;

use crate::events::EV_LIGHTING;
use crate::host::{self, AppHandle, Manager};
use crate::prisme::{Lighting, Prisme};
use crate::ui::{BarWindow, LightingData};
use crate::widgets::Backend;
use horizon_core::layout::BarLayout;
use slint::ComponentHandle;

/// Sans état ni fil : Prisme est au cœur (`crate::prisme`).
pub const BACKEND: Backend =
    Backend { id: "lighting", name: "éclairage", bind: Some(bind), panel: Some(panel::PANEL), ..Backend::NONE };

/// Nom affiché d'une scène (`sceneName` de lib/lighting.ts).
pub fn scene_name(l: &Lighting, key: &str) -> String {
    l.names.get(key).cloned().unwrap_or_else(|| key.to_owned())
}

fn show(w: &BarWindow, l: &Lighting) {
    let d = w.global::<LightingData>();
    d.set_connected(l.connected);
    d.set_off(l.override_scene.as_deref() == Some("off"));
    d.set_scene(scene_name(l, &l.scene).into());
}

fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    if let Some(p) = app.try_state::<Prisme>() {
        show(window, &p.lighting());
    }
    let w = window.as_weak();
    host::listen(EV_LIGHTING, Some(label), move |e| {
        if let (Some(l), Some(w)) = (e.payload::<Lighting>(), w.upgrade()) {
            show(&w, l);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_names_fall_back_to_the_key() {
        let mut l = Lighting::default();
        l.names.insert("music".into(), "Musique".into());
        assert_eq!(scene_name(&l, "music"), "Musique");
        assert_eq!(scene_name(&l, "video"), "video");
    }
}
