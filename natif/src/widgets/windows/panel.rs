//! Panneau Fenêtres (global `WindowsPanelData` de `ui/widgets/windows/panel.slint`) : la liste
//! par écran est lue hors du fil de l'interface à l'ouverture, puis relue à chaque changement du
//! nombre de fenêtres (`horizon://windows`), seulement panneau montré ; l'écran « ici » est celui
//! de la barre d'où vient le panneau, ou de la barre de la tuile (`panel_hosts::screen`) : chaque
//! fenêtre a sa liste. Clic : la fenêtre est ramenée (ou mise au premier plan) et le panneau
//! surgissant se ferme (`popup_close` ; une tuile reste). Clic droit : menu de la fenêtre, pour
//! la même action ou l'envoyer vers un autre écran (`menu_entries`) ; le panneau surgissant
//! reste affiché le temps du menu. Fil de l'interface.

use super::{EV_WINDOWS, bring, count_label, groups, send};
use crate::host::{self, AppHandle, Manager};
use crate::popup::LABEL;
use crate::ui::menu::{self, Entry};
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{WindowsPanelData, WindowsPanelRow};
use crate::widgets::Panel;
use horizon_core::app_windows::{ScreenGroup, send_targets};
use slint::{ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel { shown: Some(shown), hidden: Some(hidden), ..Panel::new(bind) };

/// Liste montrée par une fenêtre.
struct Open {
    groups: Vec<ScreenGroup>,
    rows: Rc<VecModel<WindowsPanelRow>>,
    /// Numéro de la dernière lecture demandée : une réponse plus ancienne est ignorée.
    seq: u64,
}

thread_local! {
    /// Par fenêtre (étiquette).
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::default();
    static SEQ: Cell<u64> = const { Cell::new(0) };
}

/// Lignes de la liste (en-têtes d'écran et fenêtres) ; l'icône est prise dans le cache des
/// icônes d'exe quand elle y est déjà.
fn rows(groups: &[ScreenGroup], icon: impl Fn(&str) -> Option<slint::Image>) -> Vec<WindowsPanelRow> {
    let mut out = Vec::new();
    for g in groups {
        out.push(WindowsPanelRow {
            header: true,
            first: out.is_empty(),
            name: g.name.as_str().into(),
            here: g.here,
            ..Default::default()
        });
        for w in &g.windows {
            let image = (!w.icon.is_empty()).then(|| icon(&w.icon)).flatten();
            out.push(WindowsPanelRow {
                here: g.here,
                hwnd: w.hwnd.to_string().into(),
                title: w.title.as_str().into(),
                meta: if w.minimized { format!("{} · réduite", w.app) } else { w.app.clone() }.into(),
                minimized: w.minimized,
                tip: if g.here { format!("Afficher « {} »", w.title) } else { format!("Ramener « {} » ici", w.title) }.into(),
                has_icon: image.is_some(),
                icon: image.unwrap_or_default(),
                letter: w.app.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default().into(),
                ..Default::default()
            });
        }
    }
    out
}

fn show(label: &str) {
    let l = label.to_owned();
    let total = OPEN.with_borrow(|o| {
        let o = o.get(label)?;
        // Une icône qui arrive plus tard redessine la liste.
        let list = rows(&o.groups, |exe| {
            let l = l.clone();
            crate::widgets::shortcuts::exe_image(exe, move |_| show(&l))
        });
        crate::ui::model::sync(&o.rows, list);
        Some(o.groups.iter().map(|g| g.windows.len()).sum::<usize>())
    });
    if let (Some(total), Some(h)) = (total, panel_hosts::host(label)) {
        h.global::<WindowsPanelData>().set_count(count_label(total).into());
    }
}

/// Relit la liste de la fenêtre `label` (fil à part) ; la réponse n'est posée que si elle est la
/// dernière demandée et le panneau toujours montré.
fn load(app: &AppHandle, label: &str) {
    let seq = SEQ.get() + 1;
    SEQ.set(seq);
    if !OPEN.with_borrow_mut(|o| o.get_mut(label).map(|o| o.seq = seq).is_some()) {
        return;
    }
    let at = panel_hosts::screen(label);
    let (a, l) = (app.clone(), label.to_owned());
    crate::util::spawn_named("fenetres-liste", move || {
        let list = groups(&a, at);
        let _ = slint::invoke_from_event_loop(move || {
            let current = OPEN.with_borrow_mut(|o| match o.get_mut(&l) {
                Some(o) if o.seq == seq => {
                    o.groups = list;
                    true
                }
                _ => false,
            });
            if current {
                show(&l);
                if let Some(h) = panel_hosts::host(&l) {
                    h.global::<WindowsPanelData>().set_loaded(true);
                }
            }
        });
    });
}

/// Entrée « afficher ou ramener ici » du menu d'une fenêtre.
const SHOW: &str = "show";
/// Préfixe des entrées « envoyer vers cet écran » (suivi de l'identifiant de l'écran).
const SEND: &str = "to:";

/// Menu d'une fenêtre (`here` : elle est sur l'écran d'où l'on regarde) : la même action que le
/// clic, puis « Envoyer vers » chacun des écrans de `targets` (grisé s'il n'y en a pas d'autre).
fn menu_entries(here: bool, targets: &[(String, String)]) -> Vec<Entry> {
    let screens = targets.iter().map(|(id, name)| Entry::item(format!("{SEND}{id}"), name.as_str())).collect();
    vec![
        Entry::item(SHOW, if here { "Afficher" } else { "Ramener ici" }),
        Entry::sub("Envoyer vers", screens).enabled(!targets.is_empty()),
    ]
}

/// Clic droit sur la fenêtre `hwnd` de la liste de `label` : son menu, sous la souris.
fn context_menu(app: &AppHandle, label: &str, hwnd: i64, inline: bool) {
    let Some(host) = app.get_window(label).map(|w| w.hwnd()) else { return };
    let group = OPEN.with_borrow(|o| {
        let g = o.get(label)?.groups.iter().find(|g| g.windows.iter().any(|w| w.hwnd == hwnd))?;
        Some((g.monitor_id.clone(), g.here))
    });
    let Some((current, here)) = group else { return };
    let entries = menu_entries(here, &send_targets(&horizon_sys::monitors::list(), &current));
    // La barre d'une tuile ne garde le premier plan qu'en mode édition, le panneau surgissant
    // que s'il est épinglé.
    let keep_active = if inline { crate::ui::edit::editing(label) } else { crate::ui::popup::pinned() };
    if !inline {
        crate::ui::popup::hold_for_menu();
    }
    let (a, l) = (app.clone(), label.to_owned());
    menu::show_then(host, entries, keep_active, move |picked| {
        let acted = match picked.as_deref() {
            Some(SHOW) => {
                bring(&a, hwnd as isize, panel_hosts::screen(&l));
                true
            }
            Some(id) => id.strip_prefix(SEND).is_some_and(|monitor| {
                send(&a, hwnd as isize, monitor.to_owned());
                true
            }),
            None => false,
        };
        if !inline {
            crate::ui::popup::menu_closed();
            if acted {
                crate::ui::popup::close();
            }
        }
    });
}

fn bind(app: &AppHandle, win: &Host) {
    let a = app.clone();
    let (l, inline) = (win.label.clone(), win.inline());
    win.global::<WindowsPanelData>().on_bring(move |hwnd| {
        let Ok(h) = hwnd.parse::<i64>() else { return };
        bring(&a, h as isize, panel_hosts::screen(&l));
        if !inline {
            crate::ui::popup::close();
        }
    });
    let (a, l) = (app.clone(), win.label.clone());
    win.global::<WindowsPanelData>().on_menu(move |hwnd| {
        if let Ok(h) = hwnd.parse::<i64>() {
            context_menu(&a, &l, h, inline);
        }
    });
    // Une seule fois : chaque fenêtre qui montre le panneau relit sa liste.
    if win.is_popup() {
        let a = app.clone();
        host::listen(EV_WINDOWS, Some(LABEL), move |_| {
            let labels: Vec<String> = OPEN.with_borrow(|o| o.keys().cloned().collect());
            for l in labels {
                load(&a, &l);
            }
        });
    }
}

/// Une fenêtre montre le panneau : liste lue (« Chargement… » jusqu'à la première réponse).
fn shown(app: &AppHandle, win: &Host, _kind: &str) {
    let rows: Rc<VecModel<WindowsPanelRow>> = Rc::default();
    let g = win.global::<WindowsPanelData>();
    g.set_rows(ModelRc::from(rows.clone()));
    g.set_loaded(false);
    OPEN.with_borrow_mut(|o| o.insert(win.label.clone(), Open { groups: Vec::new(), rows, seq: 0 }));
    load(app, &win.label);
}

/// Fin de l'affichage : liste oubliée (relue au prochain, comme le montage du panneau).
fn hidden(_app: &AppHandle, win: &Host, _kind: &str) {
    OPEN.with_borrow_mut(|o| o.remove(&win.label));
    win.global::<WindowsPanelData>().set_loaded(false);
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::app_windows::WindowView;

    fn win(hwnd: i64, title: &str, app: &str, minimized: bool) -> WindowView {
        WindowView { hwnd, title: title.into(), app: app.into(), icon: String::new(), minimized, monitor: String::new() }
    }

    #[test]
    fn rows_follow_panel_svelte() {
        let groups = vec![
            ScreenGroup { monitor_id: "a".into(), name: "Écran du haut".into(), here: true, windows: vec![win(1, "Grafana", "vivaldi", false)] },
            ScreenGroup { monitor_id: "b".into(), name: "Écran principal".into(), here: false, windows: vec![win(2, "Téléchargements", "Explorer", true)] },
        ];
        let r = rows(&groups, |_| None);
        assert_eq!(r.len(), 4);
        assert!(r[0].header && r[0].first && r[0].here);
        assert_eq!((r[1].tip.as_str(), r[1].meta.as_str(), r[1].letter.as_str()), ("Afficher « Grafana »", "vivaldi", "V"));
        assert!(r[2].header && !r[2].first && !r[2].here);
        assert_eq!((r[3].tip.as_str(), r[3].meta.as_str(), r[3].hwnd.as_str()), ("Ramener « Téléchargements » ici", "Explorer · réduite", "2"));
    }

    #[test]
    fn menu_offers_the_click_action_then_the_other_screens() {
        let targets = vec![("a".to_owned(), "Écran principal".to_owned()), ("b".to_owned(), "Écran du haut".to_owned())];
        let m = menu_entries(false, &targets);
        assert_eq!(m[0], Entry::item("show", "Ramener ici"));
        assert_eq!(m[1], Entry::sub("Envoyer vers", vec![Entry::item("to:a", "Écran principal"), Entry::item("to:b", "Écran du haut")]));
        // Fenêtre de l'écran de la barre ; un seul écran : rien où l'envoyer.
        let m = menu_entries(true, &[]);
        assert_eq!(m[0].text(), "Afficher");
        assert_eq!(m[1], Entry::sub("Envoyer vers", Vec::new()).enabled(false));
    }
}
