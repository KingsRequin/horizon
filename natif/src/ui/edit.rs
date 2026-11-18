//! Mode édition d'une barre, côté interface (`app/src/lib/editMode.svelte.ts` et la partie
//! « glisser » d'App.svelte) : entrée et sortie (menu, « Nouveautés » de l'îlot, Échap,
//! « Terminé », perte du premier plan), Ctrl+Z / Ctrl+Y, bande d'aide (`ui::popup::show_help`),
//! menus du clic droit et du « + », glisser-déposer des widgets avec le repère de dépôt, d'une
//! zone à l'autre, entre la bande et les tuiles d'une barre haute (seul un widget qui a un
//! panneau devient une tuile) et d'une barre (écran) à l'autre. Fil de l'interface.
//!
//! Les modifications passent par `crate::edit` (disposition, historique, diffusion) ; la règle
//! du dépôt par `crate::drag`. Les rectangles des zones et des widgets viennent de la barre
//! (`left-zone-x`…, `*-row-x`, largeur montrée de chaque emplacement, `slot-shown` ; tuiles :
//! modèle `TileBox` et `tiles-origin-*`, défilement compris), en pixels logiques de sa fenêtre.
//!
//! Écarts avec la version Tauri : pas d'oscillation des widgets (cadre fixe au survol, aucune
//! animation continue), pas d'animation « FLIP » au dépôt (les widgets prennent leur nouvelle
//! place d'un coup), pas de fondu au bord des zones qui défilent en mode édition.

use super::BarWindow;
use crate::drag::{self, Bounds, DropTarget, ZoneBox};
use crate::events::EV_EDIT;
use crate::host::{self, AppHandle, Manager};
use crate::ui::menu;
use horizon_core::layout::Zone;
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

/// Seuil du glisser (px logiques) : en deçà, l'appui n'est qu'un clic.
const DRAG_THRESHOLD: f32 = 4.0;
/// Hauteur de la bande du haut.
const BAND: f32 = 40.0;

/// Glisser en cours.
struct Drag {
    id: String,
    start: (f32, f32),
    moved: bool,
    /// Barre visée (étiquette) et cible du dépôt.
    target: Option<(String, DropTarget)>,
}

/// État du mode édition d'une barre.
#[derive(Default)]
struct EditUi {
    editing: bool,
    /// Emplacement sous la souris (menu du clic droit).
    hovered: Option<String>,
    /// Largeur montrée de chaque emplacement (sans l'écart avant lui).
    shown: HashMap<String, f32>,
    drag: Option<Drag>,
    /// Sortie différée après la perte du premier plan.
    blur: slint::Timer,
}

thread_local! {
    static EDITS: RefCell<HashMap<String, EditUi>> = RefCell::default();
}

fn with<R>(label: &str, f: impl FnOnce(&mut EditUi) -> R) -> R {
    EDITS.with_borrow_mut(|e| f(e.entry(label.to_owned()).or_default()))
}

/// La barre `label` est en mode édition.
pub fn editing(label: &str) -> bool {
    EDITS.with_borrow(|e| e.get(label).is_some_and(|x| x.editing))
}

fn hwnd_of(app: &AppHandle, label: &str) -> Option<isize> {
    app.get_window(label).map(|w| w.hwnd())
}

/// Relie le mode édition à la barre `label` (création de sa fenêtre).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str) {
    let (a, l) = (app.clone(), label.to_owned());
    host::listen(EV_EDIT, Some(label), move |e| {
        if let Some(on) = e.payload::<bool>() {
            set_editing(&a, &l, *on);
        }
    });
    let (a, l) = (app.clone(), label.to_owned());
    window.on_key(move |k| match k.as_str() {
        "escape" => set_editing(&a, &l, false),
        "undo" => report("annuler", crate::edit::edit_undo(&a).map(|_| ())),
        "redo" => report("rétablir", crate::edit::edit_redo(&a).map(|_| ())),
        _ => {}
    });
    let l = label.to_owned();
    window.on_slot_shown(move |_, id, w| {
        with(&l, |e| e.shown.insert(id.to_string(), w));
    });
    let l = label.to_owned();
    window.on_slot_hover(move |id, on| {
        with(&l, |e| {
            if on {
                e.hovered = Some(id.to_string());
            } else if e.hovered.as_deref() == Some(id.as_str()) {
                e.hovered = None;
            }
        })
    });
    let (a, l) = (app.clone(), label.to_owned());
    window.on_slot_drag(move |id, phase, x, y| on_drag(&a, &l, &id, phase, x, y));
    let (a, l) = (app.clone(), label.to_owned());
    window.on_add(move |zone| {
        let zone = match zone {
            0 => Zone::Left,
            1 => Zone::Center,
            _ => Zone::Right,
        };
        add_menu(&a, &l, zone);
    });
    let (a, l) = (app.clone(), label.to_owned());
    crate::platform::on_context(window.window(), move |_, _| context_menu(&a, &l));
    // Clic ailleurs (la barre perd le premier plan) : l'édition se termine, sauf pour un menu ou
    // la fenêtre de réglages (`horizon://settings-window` de la version Tauri).
    let (a, l) = (app.clone(), label.to_owned());
    crate::platform::on_active(window.window(), move |active| {
        if active || !editing(&l) {
            return;
        }
        let (a, l2) = (a.clone(), l.clone());
        with(&l, |e| {
            e.blur.start(slint::TimerMode::SingleShot, Duration::from_millis(300), move || {
                // Premier plan à cette barre, ou à une autre barre en mode édition (essai de
                // toutes les barres) : l'édition continue.
                // SAFETY : lecture de la fenêtre au premier plan.
                let front = unsafe { GetForegroundWindow() }.0 as isize;
                let foreground = super::bar::labels().iter().any(|x| editing(x) && hwnd_of(&a, x) == Some(front));
                if editing(&l2) && !foreground && !menu::is_open() && !super::settings_window::is_open() {
                    set_editing(&a, &l2, false);
                }
            })
        });
    });
}

fn report(what: &str, r: Result<(), String>) {
    if let Err(e) = r {
        log::warn!("mode édition, {what} : {e}");
    }
}

/// Barre fermée.
pub fn forget(label: &str) {
    let _ = EDITS.try_with(|e| e.borrow_mut().remove(label));
}

/// Entre en mode édition (`on`) ou en sort : la fenêtre devient activable (clavier), la bande
/// d'aide s'affiche sous le milieu de la barre.
pub fn set_editing(app: &AppHandle, label: &str, on: bool) {
    let Some(window) = super::bar::window(label) else { return };
    if with(label, |e| std::mem::replace(&mut e.editing, on)) == on {
        return;
    }
    let monitor = crate::edit::monitor_of(app, label).unwrap_or_default();
    log::info!("{label} : mode édition {}", if on { "ouvert" } else { "fermé" });
    super::bar::set_editing(label, on);
    crate::platform::set_focusable(window.window(), on);
    if on {
        if let Some(h) = hwnd_of(app, label) {
            horizon_sys::window_focus::bring_to_front(h);
        }
        window.invoke_focus_keys();
        super::popup::show_help(label, &monitor);
    } else {
        with(label, |e| {
            e.drag = None;
            e.blur.stop();
        });
        clear_drag_view(&window);
        hide_markers();
        super::popup::hide_help(&monitor);
    }
    super::bar::refit(label);
}

/// Menu « Ajouter un widget » de la zone `zone`.
pub(super) fn add_menu(app: &AppHandle, label: &str, zone: Zone) {
    let (Some(hwnd), Some(monitor)) = (hwnd_of(app, label), crate::edit::monitor_of(app, label)) else { return };
    let a = app.clone();
    menu::show(hwnd, crate::edit::menu_add(app, &monitor, zone), editing(label), move |id| crate::edit::on_menu(&a, &id));
}

/// Clic droit relâché dans la barre : menu du widget en mode édition, sinon menu de la barre
/// (avec en tête les réglages du widget sous la souris s'il en a). Rien si un widget a ouvert
/// son propre menu (suggestion).
fn context_menu(app: &AppHandle, label: &str) {
    if menu::requested() {
        return;
    }
    let (Some(hwnd), Some(monitor)) = (hwnd_of(app, label), crate::edit::monitor_of(app, label)) else { return };
    let (on, hovered) = with(label, |e| (e.editing, e.hovered.clone()));
    let kind = hovered.as_deref().and_then(|id| super::bar::kind_of(label, id));
    let has_settings = kind.as_deref().is_some_and(crate::widgets::has_settings);
    let entries = match (&hovered, on) {
        (Some(id), true) => crate::edit::menu_widget(app, id, has_settings),
        _ => crate::edit::menu_bar(app, &monitor, hovered.as_deref().filter(|_| has_settings)),
    };
    let a = app.clone();
    menu::show(hwnd, entries, on, move |id| crate::edit::on_menu(&a, &id));
}

/// Rectangles des zones de la barre `label` (et de leurs widgets, hors `exclude`), pixels
/// logiques de sa fenêtre : la bande, et la zone des tuiles d'une barre haute si `tiles` (seul un
/// widget qui a un panneau peut devenir une tuile), places des tuiles défilement compris.
fn zone_boxes(label: &str, exclude: &str, tiles: bool) -> Vec<ZoneBox> {
    let Some(window) = super::bar::window(label) else { return Vec::new() };
    let shown = EDITS.with_borrow(|e| e.get(label).map(|x| x.shown.clone()).unwrap_or_default());
    let zones = [
        (Zone::Left, window.get_left_zone_x(), window.get_left_zone_w(), window.get_left_row_x(), window.get_left()),
        (Zone::Center, window.get_center_zone_x(), window.get_center_zone_w(), window.get_center_row_x(), window.get_center()),
        (Zone::Right, window.get_right_zone_x(), window.get_right_zone_w(), window.get_right_row_x(), window.get_right()),
    ];
    // Bande entière : la zone droite s'arrête à la marge du bord (celle du début de la gauche).
    let band = Bounds::new(0.0, 0.0, window.get_right_zone_x() + window.get_right_zone_w() + window.get_left_zone_x(), BAND);
    zones
        .into_iter()
        .map(|(zone, zx, zw, row_x, model)| {
            let mut x = row_x;
            let mut items = Vec::new();
            for s in model.iter().filter(|s| !s.hidden) {
                x += s.lead;
                let w = shown.get(s.id.as_str()).copied().unwrap_or(0.0);
                if s.id != exclude {
                    items.push(Bounds::new(x, 2.0, x + w, BAND - 2.0));
                }
                x += w;
            }
            ZoneBox { zone, bounds: Bounds::new(zx, 2.0, zx + zw, BAND - 2.0), items, columns: false, band: Some(band) }
        })
        .chain((tiles && window.get_tall()).then(|| tile_box(&window, exclude)))
        .collect()
}

/// Zone des tuiles et place de chaque tuile (hors `exclude`), dans l'ordre (de haut en bas puis
/// de colonne en colonne).
fn tile_box(window: &BarWindow, exclude: &str) -> ZoneBox {
    let (ox, oy) = (window.get_tiles_origin_x(), window.get_tiles_origin_y());
    let (zx, zy) = (window.get_tiles_zone_x(), window.get_tiles_zone_y());
    let items = window
        .get_tiles()
        .iter()
        .filter(|b| b.slot.id != exclude)
        .map(|b| Bounds::new(ox + b.x, oy + b.y, ox + b.x + b.width, oy + b.y + b.height))
        .collect();
    ZoneBox {
        zone: Zone::Tiles,
        bounds: Bounds::new(zx, zy, zx + window.get_tiles_zone_w(), window.get_tiles_zone_bottom()),
        items,
        columns: true,
        band: None,
    }
}

/// Rectangle physique d'une fenêtre de barre et son échelle.
fn screen_rect(app: &AppHandle, label: &str) -> Option<(RECT, f32)> {
    let hwnd = hwnd_of(app, label)?;
    let scale = crate::platform::scale(super::bar::window(label)?.window()).unwrap_or(1.0);
    let mut r = RECT::default();
    // SAFETY : lecture du rectangle d'une fenêtre de barre.
    unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
    Some((r, scale))
}

/// Barre sous le point (`x`, `y` logiques de la barre `from`) : celle dont la fenêtre le
/// contient, sinon `from` ; avec le point dans ses coordonnées logiques.
fn bar_under(app: &AppHandle, from: &str, x: f32, y: f32) -> (String, f32, f32) {
    let Some((r, s)) = screen_rect(app, from) else { return (from.to_owned(), x, y) };
    let (sx, sy) = (r.left as f32 + x * s, r.top as f32 + y * s);
    for label in super::bar::labels() {
        if label == from {
            continue;
        }
        if let Some((o, os)) = screen_rect(app, &label)
            && sx >= o.left as f32
            && sx < o.right as f32
            && sy >= o.top as f32
            && sy < o.bottom as f32
        {
            return (label, (sx - o.left as f32) / os, (sy - o.top as f32) / os);
        }
    }
    (from.to_owned(), x, y)
}

/// Glisser d'un widget de la barre `label` : phase 0 appui, 1 déplacement, 2 relâché, 3 annulé ;
/// position dans la fenêtre.
fn on_drag(app: &AppHandle, label: &str, id: &str, phase: i32, x: f32, y: f32) {
    if !editing(label) {
        return;
    }
    let Some(window) = super::bar::window(label) else { return };
    match phase {
        0 => with(label, |e| e.drag = Some(Drag { id: id.to_owned(), start: (x, y), moved: false, target: None })),
        1 => {
            let Some((start, moved)) = with(label, |e| e.drag.as_ref().map(|d| (d.start, d.moved))) else { return };
            // Deux dimensions sur une barre haute (vers les tuiles), horizontal sinon.
            let (dx, dy) = (x - start.0, if window.get_tall() { y - start.1 } else { 0.0 });
            if !moved && dx.hypot(dy) <= DRAG_THRESHOLD {
                return;
            }
            window.set_drag_id(id.into());
            window.set_drag_zone(zone_of(&window, id).unwrap_or(-1));
            window.set_drag_dx(dx);
            window.set_drag_dy(dy);
            let (target_label, tx, ty) = bar_under(app, label, x, y);
            let panel = super::bar::kind_of(label, id).is_some_and(|k| crate::widgets::has_panel(&k));
            let zones = zone_boxes(&target_label, id, panel);
            let target = drag::drop_target(tx, ty, &zones);
            hide_markers();
            if let Some(t) = target
                && let Some(z) = zones.iter().find(|z| z.zone == t.zone)
                && let Some(w) = super::bar::window(&target_label)
            {
                let m = drag::marker_at(z, t);
                w.set_marker_x(m.x);
                w.set_marker_y(m.top);
                w.set_marker_w(m.width);
                w.set_marker_h(m.height);
                w.set_marker_shown(true);
            }
            with(label, |e| {
                if let Some(d) = e.drag.as_mut() {
                    d.moved = true;
                    d.target = target.map(|t| (target_label, t));
                }
            });
        }
        _ => {
            let drag = with(label, |e| e.drag.take());
            clear_drag_view(&window);
            hide_markers();
            let Some(Drag { id, moved: true, target: Some((to, t)), .. }) = drag.filter(|_| phase == 2) else { return };
            let Some(monitor) = crate::edit::monitor_of(app, &to) else { return };
            report("déplacer", crate::edit::edit_move(app, &id, &monitor, t.zone, t.index));
        }
    }
}

/// Rang de la zone (0, 1, 2 ; 3 = tuiles) qui contient le widget `id`.
fn zone_of(window: &BarWindow, id: &str) -> Option<i32> {
    if window.get_tiles().iter().any(|b| b.slot.id == id) {
        return Some(3);
    }
    [window.get_left(), window.get_center(), window.get_right()]
        .iter()
        .position(|m| m.iter().any(|s| s.id == id))
        .map(|i| i as i32)
}

fn clear_drag_view(window: &BarWindow) {
    window.set_drag_id("".into());
    window.set_drag_zone(-1);
    window.set_drag_dx(0.0);
    window.set_drag_dy(0.0);
}

/// Repère de dépôt retiré de toutes les barres.
fn hide_markers() {
    for label in super::bar::labels() {
        if let Some(w) = super::bar::window(&label) {
            w.set_marker_shown(false);
        }
    }
}

/// Essai sans clic (`horizon.exe --edition-essai <quoi>` pendant qu'Horizon tourne),
/// sur la barre principale : `on` / `off` (mode édition ; `toutes` / `aucune` : toutes les
/// barres), `menu-barre`, `menu-widget` (premier
/// widget de la zone gauche), `menu-ajout` (zone gauche), sous la souris.
pub fn essai(app: &AppHandle, what: &str) {
    let Some(monitor) = app.state::<crate::state::AppState>().main_monitor() else { return };
    let Some(label) = crate::edit::label_of(app, &monitor) else { return };
    log::info!("essai du mode édition : {what}");
    match what {
        "on" => set_editing(app, &label, true),
        "off" => set_editing(app, &label, false),
        // Toutes les barres (styles îlots et graphique, écran 4K).
        "toutes" | "aucune" => {
            for l in super::bar::labels() {
                set_editing(app, &l, what == "toutes");
            }
        }
        "menu-barre" | "menu-widget" => {
            let Some(hwnd) = hwnd_of(app, &label) else { return };
            let first = app.state::<crate::state::AppState>().bar(&monitor).and_then(|b| b.left.first().map(|w| (w.id.clone(), w.kind)));
            let entries = match (what, first) {
                ("menu-widget", Some((id, kind))) => crate::edit::menu_widget(app, &id, crate::widgets::has_settings(kind.as_str())),
                _ => crate::edit::menu_bar(app, &monitor, None),
            };
            let a = app.clone();
            menu::show(hwnd, entries, editing(&label), move |id| crate::edit::on_menu(&a, &id));
        }
        "menu-ajout" => add_menu(app, &label, Zone::Left),
        _ => log::warn!("essai du mode édition inconnu : {what}"),
    }
}
