//! Panneau Éclairage (global `LightingPanelData` de `ui/widgets/lighting/panel.slint`) : scènes
//! de Prisme en grille (« Automatique » d'abord, sans « off »), extinction, luminosité, ouverture
//! de l'interface de Prisme. L'état vient du lien avec Prisme (`crate::prisme`, événement
//! `EV_LIGHTING`). Fil de l'interface.

use super::scene_name;
use crate::events::EV_LIGHTING;
use crate::host::{self, AppHandle, Manager};
use crate::popup::LABEL;
use crate::prisme::{self, Lighting, Prisme};
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{LightingPanelData, LightingPanelPair, LightingPanelScene, model};
use crate::widgets::Panel;
use slint::{ModelRc, VecModel};
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel::new(bind);

thread_local! {
    static SCENES: Rc<VecModel<LightingPanelPair>> = Rc::new(VecModel::default());
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<LightingPanelData>();
    g.set_scenes(ModelRc::from(SCENES.with(Rc::clone)));
    if let Some(p) = app.try_state::<Prisme>() {
        show(&p.lighting());
    }
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_LIGHTING, Some(LABEL), |e| {
            if let Some(l) = e.payload::<Lighting>() {
                show(l);
            }
        });
    }
    let a = app.clone();
    g.on_set_scene(move |key| {
        let scene = (!key.is_empty()).then(|| key.to_string());
        report(prisme::lighting_set_scene(a.clone(), scene));
    });
    let a = app.clone();
    g.on_toggle(move || report(prisme::lighting_toggle(a.clone())));
    let a = app.clone();
    g.on_set_brightness(move |v| report(prisme::lighting_set_brightness(a.clone(), v)));
    let a = app.clone();
    g.on_open_prisme(move || crate::prisme_window::open(&a));
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::info!("panneau Éclairage : {e}");
    }
}

/// Boutons de scène : « Automatique » (avec la scène appliquée quand il est choisi), puis chaque
/// scène sauf « off », par rangées de deux.
fn pairs(l: &Lighting) -> Vec<LightingPanelPair> {
    let auto = l.override_scene.is_none();
    let mut buttons = vec![LightingPanelScene {
        key: "".into(),
        name: "Automatique".into(),
        sub: if auto { scene_name(l, &l.scene).into() } else { "".into() },
        on: auto,
    }];
    buttons.extend(l.scenes.iter().filter(|s| *s != "off").map(|s| LightingPanelScene {
        key: s.as_str().into(),
        name: scene_name(l, s).into(),
        sub: "".into(),
        on: l.override_scene.as_deref() == Some(s.as_str()),
    }));
    buttons
        .chunks(2)
        .map(|c| LightingPanelPair { left: c[0].clone(), has_right: c.len() > 1, right: c.get(1).cloned().unwrap_or_default() })
        .collect()
}

fn show(l: &Lighting) {
    panel_hosts::each("lighting", |h| {
        let g = h.global::<LightingPanelData>();
        g.set_off(l.override_scene.as_deref() == Some("off"));
        g.set_brightness(l.brightness);
    });
    SCENES.with(|m| model::sync(m, pairs(l)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_first_then_scenes_by_two_without_off() {
        let mut l = Lighting { scene: "music".into(), scenes: vec!["idle".into(), "music".into(), "off".into()], ..Lighting::default() };
        l.names.insert("music".into(), "Musique".into());
        let p = pairs(&l);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].left.name.as_str(), p[0].left.sub.as_str(), p[0].left.on), ("Automatique", "Musique", true));
        assert_eq!((p[0].right.key.as_str(), p[0].has_right), ("idle", true));
        assert_eq!((p[1].left.name.as_str(), p[1].has_right), ("Musique", false));
        l.override_scene = Some("idle".into());
        let p = pairs(&l);
        assert!(!p[0].left.on && p[0].left.sub.is_empty());
        assert!(p[0].right.on);
    }
}
