//! Panneau des Raccourcis (global `ShortcutsPanelData` de `ui/widgets/shortcuts/panel.slint`) :
//! les raccourcis de l'exemplaire, avec le modèle même de sa rangée dans la barre (icônes,
//! pastilles « ouvert »). Clic : l'appli est ouverte ou ramenée, et le panneau surgissant se
//! ferme (une tuile reste). Fil de l'interface.

use crate::host::AppHandle;
use crate::ui::ShortcutsPanelData;
use crate::ui::panel_hosts::Host;
use crate::widgets::Panel;
use slint::ModelRc;

pub(super) const PANEL: Panel = Panel::new(bind);

fn bind(_app: &AppHandle, win: &Host) {
    let g = win.global::<ShortcutsPanelData>();
    g.on_items(|instance| super::bar_of(&instance).map(|b| ModelRc::from(b.model(&instance))).unwrap_or_default());
    let inline = win.inline();
    g.on_open(move |instance, index| {
        if let Some(b) = super::bar_of(&instance) {
            b.open(&instance, index as usize, false);
        }
        if !inline {
            crate::ui::popup::close();
        }
    });
}
