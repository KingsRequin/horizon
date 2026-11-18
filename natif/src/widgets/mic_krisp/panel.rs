//! Panneau Audio (global `MicKrispPanelData` de `ui/widgets/mic_krisp/panel.slint`) : sorties et
//! micros (favoris en tête ; avec des favoris, les autres derrière « N autres »), volume général,
//! volume de chaque appli (icône de son exe). Le mixeur partagé (`crate::mixer`) n'est suivi que
//! panneau ouvert (`useMixer` de la version Tauri). Fil de l'interface.

use crate::events::EV_MIXER;
use crate::host::{self, AppHandle};
use crate::mixer::{self, MixerView};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{MicKrispPanelApp, MicKrispPanelData, MicKrispPanelDevice, MicKrispPanelDevices, model};
use crate::widgets::Panel;
use horizon_core::mixer::{AppAudio, Device, Flow};
use slint::{ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

thread_local! {
    static OUTPUTS: Rc<VecModel<MicKrispPanelDevice>> = Rc::new(VecModel::default());
    static INPUTS: Rc<VecModel<MicKrispPanelDevice>> = Rc::new(VecModel::default());
    static APPS: Rc<VecModel<MicKrispPanelApp>> = Rc::new(VecModel::default());
    /// Tous les périphériques montrés (« Favoris seulement » pour revenir) : sorties, micros.
    /// Oublié à la fermeture, comme l'état local du `Panel.svelte`.
    static EXPANDED: Cell<(bool, bool)> = const { Cell::new((false, false)) };
    /// Dernier état reçu (redessiné quand une icône d'appli arrive ou qu'une liste s'ouvre).
    static LAST: RefCell<MixerView> = RefCell::default();
}

fn with_data(mut f: impl FnMut(MicKrispPanelData<'_>)) {
    panel_hosts::each("mic_krisp", |h| f(h.global::<MicKrispPanelData>()));
}

fn bind(app: &AppHandle, win: &Host) {
    win.global::<MicKrispPanelData>().set_apps(ModelRc::from(APPS.with(Rc::clone)));
    // Dernier état connu (une tuile reliée pendant que le panneau est déjà montré ailleurs).
    show();
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_MIXER, Some(LABEL), |e| {
            if let Some(v) = e.payload::<MixerView>() {
                LAST.set(v.clone());
                show();
            }
        });
    }
    let g = win.global::<MicKrispPanelData>();
    let a = app.clone();
    g.on_set_default(move |id| report(mixer::set_default(&a, id.to_string())));
    let a = app.clone();
    g.on_pin(move |id, pinned| report(mixer::pin(&a, id.to_string(), pinned)));
    g.on_toggle_expanded(|output| {
        let (o, i) = EXPANDED.get();
        EXPANDED.set(if output { (!o, i) } else { (o, !i) });
        show();
    });
    let a = app.clone();
    g.on_set_master(move |v| report(mixer::set_master(&a, v)));
    let a = app.clone();
    g.on_set_master_mute(move |m| report(mixer::set_master_mute(&a, m)));
    let a = app.clone();
    g.on_set_app_volume(move |key, v| report(mixer::set_app_volume(&a, key.to_string(), v)));
    let a = app.clone();
    g.on_set_app_mute(move |key, m| report(mixer::set_app_mute(&a, key.to_string(), m)));
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::warn!("panneau Audio : {e}");
    }
}

/// Panneau ouvert : le mixeur est suivi (relu tant qu'il est ouvert), l'état affiché aussitôt.
fn opened(app: &AppHandle, _kind: &str) {
    mixer::watch(app, true);
    LAST.set(mixer::view(app));
    show();
}

fn closed(app: &AppHandle, _kind: &str) {
    mixer::watch(app, false);
    EXPANDED.set((false, false));
}

/// Périphériques montrés (`visibleDevices` de lib/mixer.ts) : tous s'il n'y a aucun favori ;
/// sinon les favoris et le défaut, les autres derrière « N autres » (`expanded` les montre).
/// L'ordre reçu (favoris en tête) est gardé. Renvoie aussi le nombre de cachés.
fn visible_devices<'a>(devices: &'a [Device], default: Option<&str>, expanded: bool) -> (Vec<&'a Device>, usize) {
    if expanded || !devices.iter().any(|d| d.pinned) {
        return (devices.iter().collect(), 0);
    }
    let shown: Vec<&Device> = devices.iter().filter(|d| d.pinned || Some(d.id.as_str()) == default).collect();
    let hidden = devices.len() - shown.len();
    (shown, hidden)
}

/// « Haut-parleurs (Realtek(R) Audio) » → nom et précision entre parenthèses (`splitName`).
fn split_name(name: &str) -> (&str, &str) {
    let t = name.trim_end();
    if let Some(body) = t.strip_suffix(')') {
        // Parenthèse ouvrante qui équilibre la dernière fermante (« Realtek(R) Audio » garde la
        // sienne), comme la regex non gourmande `^(.*?)\s*\((.+)\)\s*$` : la première « ( » dont
        // le reste se termine par « ) ».
        if let Some(open) = body.find('(') {
            let main = body[..open].trim_end();
            let detail = &body[open + 1..];
            if !main.is_empty() && !detail.is_empty() {
                return (main, detail);
            }
        }
    }
    (name, "")
}

/// Casques reconnus à leur nom (`HEADPHONES` de lib/mixer.ts).
const HEADPHONES: [&str; 9] = ["casque", "headset", "headphone", "écouteur", "earbud", "pro x", "arctis", "buds", "airpods"];

/// Icône d'un périphérique (`deviceIcon`) : 0 haut-parleur, 1 casque, 2 micro.
fn device_icon(d: &Device) -> i32 {
    if d.flow == Flow::Input {
        return 2;
    }
    let n = d.name.to_lowercase();
    if HEADPHONES.iter().any(|h| n.contains(h)) { 1 } else { 0 }
}

fn devices(list: &[Device], default: Option<&str>, expanded: bool, model: &Rc<VecModel<MicKrispPanelDevice>>) -> MicKrispPanelDevices {
    let (shown, hidden) = visible_devices(list, default, expanded);
    let rows = shown
        .into_iter()
        .map(|d| {
            let (main, detail) = split_name(&d.name);
            MicKrispPanelDevice {
                id: d.id.as_str().into(),
                name: d.name.as_str().into(),
                main: main.into(),
                detail: detail.into(),
                icon: device_icon(d),
                on: Some(d.id.as_str()) == default,
                pinned: d.pinned,
            }
        })
        .collect();
    model::sync(model, rows);
    // Le même modèle est reposé : la structure ne change que par ses autres champs.
    MicKrispPanelDevices { shown: ModelRc::from(Rc::clone(model)), empty: list.is_empty(), hidden: hidden as i32, expanded }
}

fn app_row(a: &AppAudio) -> MicKrispPanelApp {
    let icon = if a.exe.is_empty() { None } else { super::super::shortcuts::exe_image(&a.exe, |_| show()) };
    MicKrispPanelApp {
        key: a.key.as_str().into(),
        name: a.name.as_str().into(),
        initial: a.name.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default().into(),
        has_icon: icon.is_some(),
        icon: icon.unwrap_or_default(),
        active: a.active,
        volume: a.level.volume,
        muted: a.level.muted,
    }
}

/// Affiche le dernier état reçu.
fn show() {
    let (eo, ei) = EXPANDED.get();
    LAST.with_borrow(|v| {
        let s = &v.snapshot;
        let outputs = OUTPUTS.with(|m| devices(&s.outputs, s.default_output.as_deref(), eo, m));
        let inputs = INPUTS.with(|m| devices(&s.inputs, s.default_input.as_deref(), ei, m));
        let apps: Vec<MicKrispPanelApp> = s.apps.iter().map(app_row).collect();
        APPS.with(|m| model::sync(m, apps));
        with_data(|g| {
            g.set_outputs(outputs.clone());
            g.set_inputs(inputs.clone());
            g.set_has_master(s.master.is_some());
            if let Some(m) = &s.master {
                g.set_master_volume(m.volume);
                g.set_master_muted(m.muted);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, flow: Flow, pinned: bool) -> Device {
        Device { id: id.into(), name: name.into(), flow, pinned }
    }

    #[test]
    fn names_are_split_at_the_first_parenthesis() {
        assert_eq!(split_name("Haut-parleurs (Realtek(R) Audio)"), ("Haut-parleurs", "Realtek(R) Audio"));
        assert_eq!(split_name("CABLE Input (VB-Audio Virtual Cable) "), ("CABLE Input", "VB-Audio Virtual Cable"));
        assert_eq!(split_name("Casque"), ("Casque", ""));
        assert_eq!(split_name("(seulement)"), ("(seulement)", ""));
        assert_eq!(split_name("Nom ()"), ("Nom ()", ""));
    }

    #[test]
    fn device_icons_follow_the_name() {
        assert_eq!(device_icon(&dev("a", "Casque (PRO X 2 LIGHTSPEED)", Flow::Output, false)), 1);
        assert_eq!(device_icon(&dev("a", "Écouteurs (AirPods)", Flow::Output, false)), 1);
        assert_eq!(device_icon(&dev("a", "Haut-parleurs (Realtek(R) Audio)", Flow::Output, false)), 0);
        assert_eq!(device_icon(&dev("a", "Micro (PRO X 2)", Flow::Input, false)), 2);
    }

    #[test]
    fn favourites_hide_the_others_unless_expanded() {
        let list = [
            dev("p", "Casque", Flow::Output, true),
            dev("d", "Haut-parleurs", Flow::Output, false),
            dev("x", "CABLE Input", Flow::Output, false),
            dev("y", "LG", Flow::Output, false),
        ];
        let ids = |v: Vec<&Device>| v.iter().map(|d| d.id.clone()).collect::<Vec<_>>();
        let (shown, hidden) = visible_devices(&list, Some("d"), false);
        assert_eq!((ids(shown), hidden), (vec!["p".to_owned(), "d".to_owned()], 2));
        let (shown, hidden) = visible_devices(&list, Some("d"), true);
        assert_eq!((shown.len(), hidden), (4, 0));
        // Aucun favori : tout est montré.
        let (shown, hidden) = visible_devices(&list[1..], Some("d"), false);
        assert_eq!((shown.len(), hidden), (3, 0));
    }
}
