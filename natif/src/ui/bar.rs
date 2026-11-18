//! Fenêtre d'une barre (`ui/bar.slint`) : zones et emplacements d'après `layout.json`, widgets
//! portés reliés à leurs services (`widgets::bind_all`), accent, affichage du volume, pulsation,
//! mode nuit, molette = volume (avec sa bulle), débordement (`overflow.rs`), réapparition, mode
//! édition (`ui::edit`), et sur une barre haute les tuiles (panneaux des widgets rangés en
//! colonnes par `tiles.rs`, reliés par `panel_hosts`). La disposition modifiée (mode édition,
//! profil, hauteur, style) arrive par `EV_LAYOUT` : emplacements mis à jour ligne par ligne,
//! widgets reliés au besoin (`widgets::rebind`), tuiles rangées de nouveau. Fil de l'interface.

use super::{BarWindow, Slot, TileBox, icons, panel_hosts};
use crate::comfort::NightState;
use crate::events::{EV_LAYOUT, EV_NIGHT};
use crate::host::{self, AppHandle, Manager};
use crate::overflow::{self, Fit, Zone};
use crate::tiles::{self, Item};
use crate::platform::{self, Frame, WindowSpec};
use horizon_core::geometry::MonitorInfo;
use horizon_core::layout::{BarLayout, BarRole, BarStyle, Interaction, WidgetInstance};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

struct BarUi {
    app: AppHandle,
    window: BarWindow,
    /// Écran de la barre (sa disposition, l'écran « ici » des panneaux en tuile).
    monitor: MonitorInfo,
    clickable: Cell<bool>,
    /// Disposition montrée.
    layout: RefCell<BarLayout>,
    /// Genres de widgets déjà reliés à la fenêtre (`widgets::rebind`).
    bound: RefCell<HashSet<&'static str>>,
    /// Emplacements de chaque zone (gauche, centre, droite), masqués compris.
    models: [Rc<VecModel<Slot>>; 3],
    /// Largeur naturelle de chaque emplacement (identifiant → px logiques), annoncée par
    /// l'interface, par état (normal, resserré).
    widths: Rc<RefCell<overflow::Widths>>,
    fit: RefCell<Fit>,
    fit_pending: Cell<bool>,
    /// Déjà affichée une fois (la première apparition n'est pas animée).
    shown_once: Cell<bool>,
    /// Tuiles d'une barre haute (une ligne par tuile, place posée par `run_tiles`).
    tiles: Rc<VecModel<TileBox>>,
    /// Hauteur naturelle annoncée par chaque tuile (identifiant du widget → px logiques).
    tile_heights: Rc<RefCell<HashMap<String, f32>>>,
    tiles_pending: Cell<bool>,
}

thread_local! {
    static BARS: RefCell<HashMap<String, Rc<BarUi>>> = RefCell::default();
}

fn get(label: &str) -> Option<Rc<BarUi>> {
    BARS.with_borrow(|b| b.get(label).cloned())
}

/// Fenêtre de la barre `label`.
pub fn window(label: &str) -> Option<BarWindow> {
    get(label).map(|b| b.window.clone_strong())
}

/// Étiquettes des barres ouvertes.
pub fn labels() -> Vec<String> {
    BARS.with_borrow(|b| b.keys().cloned().collect())
}

/// Genre du widget `id` de la barre `label`.
pub fn kind_of(label: &str, id: &str) -> Option<String> {
    let b = get(label)?;
    let layout = b.layout.borrow();
    layout.widgets().find(|w| w.id == id).map(|w| w.kind.as_str().to_owned())
}

/// Emplacements d'une liste de widgets de la barre ar ; un genre inconnu (dossier retiré)
/// n'est pas affiché.
fn slots(bar: &BarLayout, list: &[WidgetInstance]) -> Vec<Slot> {
    list.iter()
        .filter_map(|w| {
            let m = crate::widgets::manifest(w.kind.as_str())?;
            Some(Slot {
                id: w.id.as_str().into(),
                kind: m.id.into(),
                label: m.label.into(),
                icon: icons::image(m.icon),
                hidden: false,
                lead: 0.0,
                data: crate::widgets::data_index(bar, w),
                clickable: crate::widgets::clicks_on(bar.interaction, m.clickable_on_hover),
            })
        })
        .collect()
}

fn zone_index(z: Zone) -> usize {
    match z {
        Zone::Left => 0,
        Zone::Center => 1,
        Zone::Right => 2,
    }
}

/// Crée la barre `label` de l'écran `monitor`, cachée, au rectangle physique `rect`. Renvoie son
/// HWND.
pub fn open(app: &AppHandle, label: &str, monitor: &MonitorInfo, rect: (i32, i32, u32, u32)) -> anyhow::Result<isize> {
    let layout = app
        .state::<crate::state::AppState>()
        .bar(&monitor.id)
        .ok_or_else(|| anyhow::anyhow!("aucune barre pour l'écran {}", monitor.id))?;
    let window = BarWindow::new()?;
    let spec = WindowSpec {
        title: "Horizon".into(),
        frame: Frame::Layered,
        rect: Some(rect),
        focusable: false,
        topmost: true,
        taskbar: false,
        ..Default::default()
    };
    let hwnd = platform::configure(window.window(), spec)?;
    app.register_window(label, hwnd);
    let clickable = layout.interaction == Interaction::Click;
    apply_options(&window, &layout);
    // Info-bulles des widgets, sous la souris.
    window.global::<super::Tip>().on_show(move |text| super::tooltip::show(hwnd, &text));
    window.global::<super::Tip>().on_hide(super::tooltip::hide);
    let tiles = Rc::new(VecModel::default());
    window.set_tiles(ModelRc::from(tiles.clone()));
    let models =
        [slots(&layout, &layout.left), slots(&layout, &layout.center), slots(&layout, &layout.right)].map(|v| Rc::new(VecModel::from(v)));
    super::panels::bind(app, label, &window);
    super::island::bind(app, &window, label, &layout);
    if layout.role == BarRole::Main {
        window.set_main(true);
        super::bar_status::bind(app, &window, label);
    }
    let bound = crate::widgets::bind_all(app, &window, label, &layout);
    super::edit::bind(app, &window, label);
    // Accent, affichage du volume (barre principale), pulsation.
    super::accent::follow(&window);
    super::osd::bind(app, &window, label, layout.role == BarRole::Main);
    super::pulse::bind(&window, label);
    window.set_left(ModelRc::from(models[0].clone()));
    window.set_center(ModelRc::from(models[1].clone()));
    window.set_right(ModelRc::from(models[2].clone()));
    let ui = Rc::new(BarUi {
        app: app.clone(),
        window,
        monitor: monitor.clone(),
        clickable: Cell::new(clickable),
        layout: RefCell::new(layout.clone()),
        bound: RefCell::new(bound),
        models,
        widths: Rc::default(),
        fit: RefCell::default(),
        fit_pending: Cell::new(false),
        shown_once: Cell::new(false),
        tiles,
        tile_heights: Rc::default(),
        tiles_pending: Cell::new(false),
    });

    apply_night(&ui, &crate::comfort::night_state(app.clone()));
    let l = label.to_owned();
    host::listen(EV_NIGHT, Some(label), move |e| {
        if let (Some(n), Some(b)) = (e.payload::<NightState>(), get(&l)) {
            apply_night(&b, n);
        }
    });
    let (a, l) = (app.clone(), label.to_owned());
    host::listen(EV_LAYOUT, Some(label), move |e| {
        if let Some(bar) = e.payload::<BarLayout>() {
            relayout(&a, &l, bar);
        }
    });
    let l = label.to_owned();
    ui.window.on_wheel(move |notches, x| {
        if get(&l).is_some_and(|b| b.clickable.get() && !super::edit::editing(&l)) {
            // Volume par le son de Windows (COM multithread) : hors du fil de l'interface ; la
            // bulle (« 52 % » et sa jauge) revient sur ce fil.
            let l = l.clone();
            let _ = crate::util::start_thread("molette", move || match crate::comfort::volume_step(notches > 0.0) {
                Ok(v) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(b) = get(&l) {
                            let icon = if v == 0 { "speakerSlash" } else { "speakerHigh" };
                            super::hint::show(&b.window, &l, x, icon, &format!("{v} %"), Some(v as f32 / 100.0));
                        }
                    });
                }
                Err(e) => log::warn!("volume à la molette : {e}"),
            });
        }
    });
    let l = label.to_owned();
    ui.window.on_refit(move || schedule_fit(&l));
    let (l, widths, win) = (label.to_owned(), ui.widths.clone(), ui.window.as_weak());
    ui.window.on_measured(move |_, id, w| {
        // Annoncé pendant la mise en page : noté pour l'état où il a été mesuré, le calcul vient
        // au tour suivant.
        let compact = win.upgrade().is_some_and(|w| w.get_compact());
        let changed = widths.try_borrow_mut().is_ok_and(|mut m| m.insert(compact, &id, w));
        if changed {
            schedule_fit(&l);
        }
    });
    let l = label.to_owned();
    ui.window.on_tiles_resized(move || schedule_tiles(&l));
    let (l, heights) = (label.to_owned(), ui.tile_heights.clone());
    ui.window.on_tile_measured(move |id, h| {
        let changed = heights.try_borrow_mut().is_ok_and(|mut m| m.insert(id.to_string(), h) != Some(h));
        if changed {
            schedule_tiles(&l);
        }
    });
    // « Ajouter une tuile » : menu d'ajout du mode édition, zone des tuiles.
    let (a, l) = (app.clone(), label.to_owned());
    ui.window.on_add_tile(move || super::edit::add_menu(&a, &l, horizon_core::layout::Zone::Tiles));
    BARS.with_borrow_mut(|b| b.insert(label.to_owned(), ui));
    run_fit(label);
    apply_tiles(label, &layout);
    Ok(hwnd)
}

/// Tuiles de la barre `label` d'après sa disposition : zone affichée si la barre est plus haute
/// que sa bande ; une tuile par widget de la zone `tiles` qui a un panneau (`{#if def?.panel}`),
/// gardées mais pas montrées sur une barre revenue à la hauteur normale. Les panneaux montrés
/// sont annoncés à `panel_hosts` (données, crochets d'ouverture).
fn apply_tiles(label: &str, layout: &BarLayout) {
    let Some(b) = get(label) else { return };
    b.window.set_tall(layout.has_tiles_area());
    let shown: Vec<Slot> =
        slots(layout, layout.shown_tiles()).into_iter().filter(|s| crate::widgets::has_panel(s.kind.as_str())).collect();
    let (_, column_w) = tiles::columns(b.window.get_tiles_width());
    let old: Vec<TileBox> = b.tiles.iter().collect();
    let rows: Vec<TileBox> = shown
        .iter()
        .map(|s| match old.iter().find(|o| o.slot.id == s.id) {
            Some(o) => TileBox { slot: s.clone(), ..o.clone() },
            None => TileBox { slot: s.clone(), x: 0.0, y: 0.0, width: column_w, height: 0.0, placed: false },
        })
        .collect();
    super::model::sync(&b.tiles, rows);
    let kinds = shown.iter().map(|s| format!("{}:{}", s.kind, s.id)).collect();
    panel_hosts::set_tiles(&b.app, label, &b.window, kinds, b.monitor.rect);
    schedule_tiles(label);
}

/// Mode édition de la barre `label` : aspect de la zone des tuiles (cadre pointillé, « Ajouter
/// une tuile »), tuiles inertes qui se saisissent entières.
pub fn set_editing(label: &str, on: bool) {
    if let Some(b) = get(label)
        && b.window.get_editing() != on
    {
        b.window.set_editing(on);
        schedule_tiles(label);
    }
}

/// Mode édition (aspect) sur toutes les barres (essais : `HORIZON_TILES_ESSAI`).
pub fn set_editing_all(on: bool) {
    let labels: Vec<String> = BARS.with_borrow(|b| b.keys().cloned().collect());
    for l in labels {
        set_editing(&l, on);
    }
}

/// Rangement des tuiles à revoir au prochain tour (une seule fois par rafale de mesures).
fn schedule_tiles(label: &str) {
    let Some(b) = get(label) else { return };
    if b.tiles_pending.replace(true) {
        return;
    }
    let l = label.to_owned();
    slint::Timer::single_shot(Duration::ZERO, move || run_tiles(&l));
}

/// Range les tuiles en colonnes (`tiles::flow`) d'après les hauteurs annoncées ; une tuile pas
/// encore mesurée reste invisible et ne prend pas de place.
fn run_tiles(label: &str) {
    let Some(b) = get(label) else { return };
    b.tiles_pending.set(false);
    if !b.window.get_tall() {
        return;
    }
    let width = b.window.get_tiles_width();
    let zone_h = b.window.get_tiles_height();
    let editing = b.window.get_editing();
    let heights = b.tile_heights.borrow().clone();
    let rows: Vec<TileBox> = b.tiles.iter().collect();
    let measured: Vec<Option<f32>> = rows.iter().map(|r| heights.get(r.slot.id.as_str()).map(|h| tiles::bounded(*h, zone_h))).collect();
    let mut items: Vec<Item> = measured.iter().flatten().map(|h| Item::tile(*h)).collect();
    if editing {
        items.push(Item { height: 96.0, width: Some(140.0), margin: 0.0 });
    }
    let flow = tiles::flow(width, &items);
    let mut placed = flow.rects.iter();
    let next: Vec<TileBox> = rows
        .into_iter()
        .zip(&measured)
        .map(|(r, m)| match m {
            Some(_) => {
                let p = placed.next().copied().unwrap_or_default();
                TileBox { x: p.x, y: p.y, width: p.w, height: p.h, placed: true, ..r }
            }
            None => TileBox { width: flow.column_w, placed: false, ..r },
        })
        .collect();
    super::model::sync(&b.tiles, next);
    if let Some(add) = placed.next() {
        b.window.set_add_x(add.x);
        b.window.set_add_y(add.y);
    }
    b.window.set_tiles_content_h(flow.content_h);
}

/// Style et interaction de la barre.
fn apply_options(window: &BarWindow, bar: &BarLayout) {
    window.set_graphic(bar.style == BarStyle::Graphic);
    window.set_clickable(bar.interaction == Interaction::Click);
    window.set_interactive(bar.interaction != Interaction::Info);
}

/// Nouvelle disposition de la barre `label` (mode édition, profil, hauteur, style, réglages) :
/// emplacements mis à jour ligne par ligne, widgets qui arrivent reliés, ceux qui dépendent de
/// leurs exemplaires reliés de nouveau, débordement revu.
fn relayout(app: &AppHandle, label: &str, bar: &BarLayout) {
    let Some(b) = get(label) else { return };
    if *b.layout.borrow() == *bar {
        return;
    }
    let old = b.layout.replace(bar.clone());
    apply_options(&b.window, bar);
    let clickable = bar.interaction == Interaction::Click;
    if b.clickable.replace(clickable) != clickable {
        apply_night(&b, &crate::comfort::night_state(app.clone()));
    }
    crate::widgets::rebind(app, &b.window, label, Some(&old), bar, &mut b.bound.borrow_mut());
    super::island::relayout(&b.window, bar);
    for (model, list) in b.models.iter().zip([&bar.left, &bar.center, &bar.right]) {
        // Masquage et écarts gardés pour les emplacements qui restent (revus par le débordement).
        let rows: Vec<Slot> = slots(bar, list)
            .into_iter()
            .map(|mut s| {
                if let Some(old) = model.iter().find(|o| o.id == s.id) {
                    s.hidden = old.hidden;
                    s.lead = old.lead;
                }
                s
            })
            .collect();
        super::model::sync(model, rows);
    }
    apply_tiles(label, bar);
    log::info!("{label} : disposition mise à jour");
    schedule_fit(label);
}

/// Nuit : un voile sur la barre non cliquable (luminosité de `[night]`).
fn apply_night(b: &BarUi, n: &NightState) {
    let dim = if !b.clickable.get() && n.night { (1.0 - n.opacity).clamp(0.0, 1.0) } else { 0.0 };
    b.window.set_dim(dim);
}

/// Style graphique de la barre `label` (le panneau surgissant reprend son apparence).
pub fn graphic(label: &str) -> bool {
    get(label).is_some_and(|b| b.window.get_graphic())
}

/// Affiche ou cache la barre (sans l'activer). Une réapparition (après un plein écran) est
/// animée.
pub fn set_visible(label: &str, visible: bool) {
    let Some(b) = get(label) else { return };
    // Tuiles d'une barre cachée (plein écran) : leurs panneaux ne comptent plus (minuteurs,
    // suivi du mixeur arrêtés).
    panel_hosts::set_active(&b.app, label, visible);
    if !visible {
        let _ = b.window.hide();
        return;
    }
    if b.shown_once.replace(true) {
        b.window.set_arriving(true);
        let l = label.to_owned();
        slint::Timer::single_shot(Duration::from_millis(30), move || {
            if let Some(b) = get(&l) {
                b.window.set_arriving(false);
            }
        });
    }
    if let Err(e) = b.window.show() {
        log::error!("affichage de {label} : {e}");
    }
}

/// Ferme la barre (sa fenêtre native est détruite).
pub fn close(label: &str) {
    super::hint::forget(label);
    super::panels::forget(label);
    super::edit::forget(label);
    if let Some(b) = BARS.with_borrow_mut(|b| b.remove(label)) {
        panel_hosts::forget(&b.app, label);
        // Une fenêtre affichée est tenue par Slint : elle doit être cachée pour disparaître.
        let _ = b.window.hide();
    }
}

/// Débordement à revoir (mode édition ouvert ou fermé : rien n'y est masqué).
pub fn refit(label: &str) {
    schedule_fit(label);
}

/// Débordement à revoir au prochain tour (une seule fois par rafale de changements).
fn schedule_fit(label: &str) {
    let Some(b) = get(label) else { return };
    if b.fit_pending.replace(true) {
        return;
    }
    let l = label.to_owned();
    slint::Timer::single_shot(Duration::ZERO, move || run_fit(&l));
}

/// Largeur annoncée des témoins de la barre principale (au bout de la zone gauche).
const STATUS_ID: &str = "#status";

/// Largeur voulue par chaque zone si `hidden` y est masqué : les largeurs naturelles et un écart
/// avant chaque emplacement affiché sauf le premier. Un widget qui n'a rien à montrer (largeur
/// nulle : Sessions Claude sans session) ne compte pas, comme `.slot:not(:has(*))` ; les témoins
/// de la barre principale comptent dans la zone gauche.
fn needs(b: &BarUi, widths: &overflow::Widths, compact: bool, hidden: &[(Zone, &'static str)]) -> [f32; 3] {
    let gap = b.window.get_gap();
    Zone::ALL.map(|z| {
        let status = if z == Zone::Left { widths.get(compact, STATUS_ID) } else { None };
        let shown: Vec<f32> = b.models[zone_index(z)]
            .iter()
            .filter(|s| !hidden.contains(&(z, s.kind.as_str())))
            .map(|s| widths.get(compact, s.id.as_str()).unwrap_or(0.0))
            .chain(status)
            .filter(|w| *w > 0.0)
            .collect();
        shown.iter().sum::<f32>() + gap * shown.len().saturating_sub(1) as f32
    })
}

/// Masque et écarte les emplacements (ligne par ligne : rien n'est recréé). Un widget vide
/// (largeur naturelle nulle) ne prend pas d'écart et laisse au suivant la place de premier,
/// sauf en mode édition (`editing`), où il garde sa place, nommée.
fn show_slots(b: &BarUi, widths: &overflow::Widths, compact: bool, hidden: &[(Zone, &'static str)], editing: bool) {
    let gap = b.window.get_gap();
    for z in Zone::ALL {
        let model = &b.models[zone_index(z)];
        let mut first = true;
        for row in 0..model.row_count() {
            let Some(mut s) = model.row_data(row) else { continue };
            let hide = hidden.contains(&(z, s.kind.as_str()));
            let empty = !editing && widths.get(compact, s.id.as_str()).is_some_and(|w| w <= 0.0);
            let lead = if hide || empty || first { 0.0 } else { gap };
            first &= hide || empty;
            if s.hidden != hide || s.lead != lead {
                s.hidden = hide;
                s.lead = lead;
                model.set_row_data(row, s);
            }
        }
        // Témoins au bout de la zone gauche : un écart s'il y a un widget avant eux.
        if z == Zone::Left {
            let lead = if first { 0.0 } else { gap };
            if b.window.get_status_lead() != lead {
                b.window.set_status_lead(lead);
            }
        }
    }
    if b.window.get_compact() != compact {
        b.window.set_compact(compact);
    }
}

/// Applique la règle du débordement (`overflow.rs`), d'après les largeurs annoncées.
fn run_fit(label: &str) {
    let Some(b) = get(label) else { return };
    b.fit_pending.set(false);
    let widths = b.widths.borrow().clone();
    let inner = b.window.get_inner_width();
    let editing = super::edit::editing(label);
    let fit = overflow::fit(
        crate::widgets::SQUEEZE_ORDER,
        editing,
        |compact, hidden| overflow::overflowing(inner, needs(&b, &widths, compact, hidden)),
        |z, kind| b.models[zone_index(z)].iter().any(|s| s.kind == kind),
    );
    show_slots(&b, &widths, fit.compact, &fit.hidden, editing);
    if fit != *b.fit.borrow() {
        log::info!(
            "{label} : {}",
            if fit.compact { format!("trop chargée, resserrée ; masqués : {:?}", fit.hidden) } else { "tout tient".to_owned() }
        );
        *b.fit.borrow_mut() = fit;
    }
}
