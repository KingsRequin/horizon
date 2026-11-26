//! Fenêtre d'une barre (`ui/bar.slint`) : zones et emplacements d'après `layout.json`, widgets
//! portés reliés à leurs services (`widgets::bind_all`), accent, affichage du volume, pulsation,
//! mode nuit, molette = volume (avec sa bulle), débordement (`overflow.rs`), réapparition, mode
//! édition (`ui::edit`), et sur une barre haute les tuiles (panneaux des widgets rangés en
//! colonnes par `tiles.rs`, reliés par `panel_hosts`), sous la bande ou dans un volet (voir
//! « Volet des tuiles » plus bas). La disposition modifiée (mode édition,
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
use horizon_core::layout::{
    BAND_HEIGHT, BarLayout, BarRole, BarStyle, DRAWER_MARGIN, Interaction, TilesMode, WidgetInstance, drawer_height,
};
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
    /// Volet des tuiles ouvert ; épinglé (ouvert au clic : la sortie de la souris ne le ferme
    /// pas) ; hauteur de la fenêtre volet ouvert (px logiques).
    drawer_open: Cell<bool>,
    drawer_pinned: Cell<bool>,
    drawer_h: Cell<f32>,
    /// Souris sur le bouton du volet ; minuteur d'ouverture au survol ; suivi de la souris d'un
    /// volet ouvert au survol (fermeture quand elle s'en va).
    drawer_hovered: Cell<bool>,
    drawer_hover_timer: slint::Timer,
    drawer_leave_timer: slint::Timer,
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
                limit: 0.0,
                plugin: horizon_core::plugin::id_of_kind(m.id).is_some(),
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
    crate::plugins::view::bind(app, label, &window.global::<super::PluginFrames>());
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
    super::island::bind(app, &window, &layout);
    let main = layout.role == BarRole::Main;
    window.set_main(main);
    super::bar_status::bind(app, &window, label, main);
    let bound = crate::widgets::bind_all(app, &window, label, &layout);
    super::edit::bind(app, &window, label);
    // Accent, affichage du volume (barre principale), pulsation.
    super::accent::follow(&window);
    super::osd::bind(app, &window, label, layout.role == BarRole::Main);
    super::pulse::bind(&window, label, &layout);
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
        drawer_open: Cell::new(false),
        drawer_pinned: Cell::new(false),
        drawer_h: Cell::new(BAND_HEIGHT as f32),
        drawer_hovered: Cell::new(false),
        drawer_hover_timer: slint::Timer::default(),
        drawer_leave_timer: slint::Timer::default(),
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
    let (l, widths, win) = (label.to_owned(), ui.widths.clone(), ui.window.as_weak());
    ui.window.on_full_measured(move |id, w| {
        // Largeur de tous les éléments d'un widget extensible, même limité.
        let compact = win.upgrade().is_some_and(|w| w.get_compact());
        let changed = widths.try_borrow_mut().is_ok_and(|mut m| m.insert_full(compact, &id, w));
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
    let l = label.to_owned();
    ui.window.on_drawer_toggle(move || drawer_toggle(&l));
    let l = label.to_owned();
    ui.window.on_drawer_hover(move |on| drawer_hover(&l, on));
    BARS.with_borrow_mut(|b| b.insert(label.to_owned(), ui));
    run_fit(label);
    apply_tiles(label, &layout);
    sync_plugins(label, &layout);
    Ok(hwnd)
}

/// Tuiles de la barre `label` d'après sa disposition : zone affichée si la barre est plus haute
/// que sa bande (sous la barre) ou si le volet est ouvert ; une tuile par widget de la zone
/// `tiles` qui a un panneau (`{#if def?.panel}`), gardées mais pas montrées sinon. Les panneaux
/// montrés sont annoncés à `panel_hosts` (données, crochets d'ouverture) : volet fermé, aucun
/// (rien ne tourne pour lui).
fn apply_tiles(label: &str, layout: &BarLayout) {
    let Some(b) = get(label) else { return };
    let drawer = layout.tiles_mode.is_drawer();
    let tall = if drawer { b.drawer_open.get() } else { layout.has_tiles_area() };
    b.window.set_tall(tall);
    let count = layout.tiles.iter().filter(|w| crate::widgets::has_panel(w.kind.as_str())).count();
    b.window.set_drawer(drawer);
    b.window.set_drawer_button(drawer && (count > 0 || b.window.get_editing()));
    b.window.set_drawer_count(count as i32);
    b.window.set_drawer_open(b.drawer_open.get());
    let shown: Vec<Slot> = slots(layout, layout.tiles_shown(b.drawer_open.get())).into_iter().filter(|s| crate::widgets::has_panel(s.kind.as_str())).collect();
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
    let Some(b) = get(label) else { return };
    if b.window.get_editing() == on {
        return;
    }
    b.window.set_editing(on);
    // Volet : ouvert pendant l'édition (ranger les tuiles, « Ajouter une tuile »), refermé après.
    if b.layout.borrow().tiles_mode.is_drawer() {
        if on {
            open_drawer(label, true);
        } else {
            close_drawer(label);
        }
        let layout = b.layout.borrow().clone();
        apply_tiles(label, &layout);
    }
    schedule_tiles(label);
}

// --- Volet des tuiles -----------------------------------------------------------------------
//
// En mode volet (`TilesMode::Click`, `Hover`), seule la bande est réservée (`bars.rs`) ; ouvert,
// la fenêtre de la barre grandit vers le bas, par-dessus les fenêtres, à la hauteur de ses
// tuiles (bornée à l'écran), puis reprend la hauteur de la bande à la fermeture. Il ne prend
// jamais le focus : il se ferme au clic ailleurs (crochet `outside_click`, posé tant qu'un volet
// est ouvert), au bouton, au plein écran, à la fin du mode édition ; au survol, aussi quand la
// souris quitte la barre et les fenêtres d'Horizon (panneaux, menus) plus de 250 ms.

/// Écrans dont le volet est ouvert : lu hors du fil de l'interface (services des widgets en
/// tuile : `BarLayout::tiles_shown`).
static OPEN_DRAWERS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Le volet de la barre de l'écran `monitor_id` est ouvert (tout fil).
pub fn drawer_open_on(monitor_id: &str) -> bool {
    crate::util::lock(&OPEN_DRAWERS).iter().any(|m| m == monitor_id)
}

fn set_open_on(monitor_id: &str, open: bool) {
    let mut list = crate::util::lock(&OPEN_DRAWERS);
    list.retain(|m| m != monitor_id);
    if open {
        list.push(monitor_id.to_owned());
    }
}

/// Délai d'ouverture au survol du bouton (celui des panneaux).
const DRAWER_HOVER_DELAY: Duration = crate::popup::HOVER_DELAY;
/// Délai de fermeture après la sortie de la souris (celui des panneaux).
const DRAWER_LEAVE_DELAY: Duration = crate::popup::HIDE_DELAY;

/// Ouvre le volet (`pinned` : au clic, il ne se ferme pas quand la souris s'en va).
fn open_drawer(label: &str, pinned: bool) {
    let Some(b) = get(label) else { return };
    let mode = b.layout.borrow().tiles_mode;
    if !mode.is_drawer() {
        return;
    }
    if b.drawer_open.replace(true) {
        if pinned && !b.drawer_pinned.replace(true) {
            b.drawer_leave_timer.stop();
        }
        return;
    }
    b.drawer_pinned.set(pinned);
    set_open_on(&b.monitor.id, true);
    log::info!("{label} : volet ouvert{}", if pinned { "" } else { " (survol)" });
    let layout = b.layout.borrow().clone();
    apply_tiles(label, &layout);
    watch_clicks(true);
    if !pinned && mode == TilesMode::Hover {
        let l = label.to_owned();
        b.drawer_leave_timer.start(slint::TimerMode::Repeated, DRAWER_LEAVE_DELAY, move || leave_check(&l));
    }
}

/// Ferme le volet : la fenêtre reprend la hauteur de la bande.
fn close_drawer(label: &str) {
    let Some(b) = get(label) else { return };
    if !b.drawer_open.replace(false) {
        return;
    }
    b.drawer_pinned.set(false);
    b.drawer_hover_timer.stop();
    b.drawer_leave_timer.stop();
    set_open_on(&b.monitor.id, false);
    log::info!("{label} : volet fermé");
    let layout = b.layout.borrow().clone();
    apply_tiles(label, &layout);
    // La fenêtre reprend la hauteur réservée : la bande, ou toute la barre si elle vient de
    // repasser « Sous la barre » (la place réservée ne change pas toujours : rien d'autre ne la
    // reposerait).
    b.drawer_h.set(BAND_HEIGHT as f32);
    set_window_height(&b, layout.reserved_height() as f32);
    if !BARS.with_borrow(|m| m.values().any(|o| o.drawer_open.get())) {
        watch_clicks(false);
    }
}

/// Clic sur le bouton : ouvre (épinglé) ; ouvert au survol, l'épingle ; épinglé, le ferme.
fn drawer_toggle(label: &str) {
    let Some(b) = get(label) else { return };
    b.drawer_hover_timer.stop();
    if !b.drawer_open.get() || !b.drawer_pinned.get() {
        open_drawer(label, true);
    } else if !b.window.get_editing() {
        close_drawer(label);
    }
}

/// Clic sur le bouton du volet de la barre de l'écran `monitor_id` (essais :
/// `HORIZON_TILES_ESSAI`).
pub fn click_drawer(monitor_id: &str) {
    let label = BARS.with_borrow(|m| m.iter().find(|(_, b)| b.layout.borrow().monitor_id == monitor_id).map(|(l, _)| l.clone()));
    if let Some(l) = label {
        drawer_toggle(&l);
    }
}

/// Survol du bouton : en mode « Volet au survol », ouverture après `DRAWER_HOVER_DELAY`.
fn drawer_hover(label: &str, on: bool) {
    let Some(b) = get(label) else { return };
    b.drawer_hovered.set(on);
    if !on {
        b.drawer_hover_timer.stop();
        return;
    }
    if b.layout.borrow().tiles_mode != TilesMode::Hover || b.drawer_open.get() {
        return;
    }
    let l = label.to_owned();
    b.drawer_hover_timer.start(slint::TimerMode::SingleShot, DRAWER_HOVER_DELAY, move || {
        if get(&l).is_some_and(|b| b.drawer_hovered.get()) {
            open_drawer(&l, false);
        }
    });
}

/// La fenêtre racine `root` (sous la souris ou sous un clic) garde le volet de la barre `b`
/// ouvert : la barre elle-même, ou une autre fenêtre d'Horizon qui n'est pas une barre
/// (panneau, menu, info-bulle).
fn keeps(b: &BarUi, root: Option<isize>) -> bool {
    let Some(h) = root else { return false };
    platform::hwnd(b.window.window()) == Some(h)
        || !BARS.with_borrow(|m| m.values().any(|o| platform::hwnd(o.window.window()) == Some(h)))
}

/// Volet ouvert au survol : fermé quand la souris n'est plus sur la barre ni sur une fenêtre
/// d'Horizon (vérifié toutes les `DRAWER_LEAVE_DELAY`, seulement tant qu'il est ouvert ainsi).
fn leave_check(label: &str) {
    let Some(b) = get(label) else { return };
    if b.drawer_pinned.get() || b.window.get_editing() {
        b.drawer_leave_timer.stop();
        return;
    }
    let root = horizon_sys::fullscreen::cursor().and_then(|(x, y)| horizon_sys::outside_click::own_window_at(x, y));
    if !keeps(&b, root) {
        close_drawer(label);
    }
}

/// Pose ou retire le crochet « clic ailleurs » ; les appuis sont relayés au fil de l'interface.
fn watch_clicks(on: bool) {
    thread_local! {
        static WATCHING: Cell<bool> = const { Cell::new(false) };
    }
    if WATCHING.with(|w| w.replace(on)) == on {
        return;
    }
    if !on {
        horizon_sys::outside_click::stop();
        return;
    }
    let (tx, rx) = crossbeam_channel::unbounded::<(i32, i32)>();
    if !horizon_sys::outside_click::start(tx) {
        // Pas de crochet : nouvel essai à la prochaine ouverture.
        WATCHING.with(|w| w.set(false));
        return;
    }
    // Le relais s'arrête quand le crochet est retiré (son envoyeur disparaît).
    let relay = crate::util::start_thread("volet-clics", move || {
        for (x, y) in rx {
            let _ = slint::invoke_from_event_loop(move || outside_click(x, y));
        }
    });
    if let Err(e) = relay {
        log::warn!("volet : {e:#}");
    }
}

/// Un appui de souris en (x, y) (écran, px physiques) : les volets qu'il ne garde pas se ferment
/// (pas pendant le mode édition, qui a sa propre fin).
fn outside_click(x: i32, y: i32) {
    let root = horizon_sys::outside_click::own_window_at(x, y);
    for label in labels() {
        if get(&label).is_some_and(|b| b.drawer_open.get() && !b.window.get_editing() && !keeps(&b, root)) {
            close_drawer(&label);
        }
    }
}

/// Hauteur logique de la fenêtre de la barre (largeur inchangée, coin haut gauche fixe).
fn set_window_height(b: &BarUi, h: f32) {
    let w = b.window.window();
    let width = w.size().width as f32 / w.scale_factor();
    w.set_size(slint::LogicalSize::new(width, h));
}

/// Après un repositionnement par Windows (`bars::reposition`, qui pose la hauteur réservée) :
/// un volet ouvert reprend sa hauteur.
pub fn reapply_drawer(label: &str) {
    if let Some(b) = get(label)
        && b.drawer_open.get()
        && b.layout.borrow().tiles_mode.is_drawer()
    {
        set_window_height(&b, b.drawer_h.get());
    }
}

/// Hauteur logique de l'écran sous le haut de la barre (Windows peut la poser plus bas : barre
/// des tâches en haut, autre appbar).
fn screen_height(b: &BarUi) -> f32 {
    let top = b.window.window().position().y;
    let bottom = b.monitor.rect.y + b.monitor.rect.h;
    (bottom - top).max(0) as f32 / b.monitor.scale.max(0.1)
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
    // Volet : les tuiles se bornent à la plus grande hauteur qu'il peut prendre, pas à la
    // fenêtre (qui suit le contenu).
    let drawer = b.layout.borrow().tiles_mode.is_drawer();
    let chrome = b.window.get_tiles_chrome_h();
    let zone_h = if drawer { screen_height(&b) - DRAWER_MARGIN as f32 - chrome } else { b.window.get_tiles_height() };
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
    if drawer && b.drawer_open.get() {
        let h = drawer_height(flow.content_h + chrome - BAND_HEIGHT as f32, screen_height(&b));
        if b.drawer_h.replace(h) != h {
            set_window_height(&b, h);
            // Mode édition : son aide se pose sous toute la barre, volet compris.
            if editing {
                super::popup::show_help(label, &b.monitor.id);
            }
        }
    }
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
    apply_layout(app, label, bar);
}

/// Les plugins ont changé (ajoutés, retirés, acceptés) : les emplacements de toutes les barres
/// sont refaits, même à disposition égale (un genre `plugin:<id>` peut apparaître ou disparaître).
pub fn plugins_changed(app: &AppHandle) {
    for label in labels() {
        if let Some(bar) = get(&label).map(|b| b.layout.borrow().clone()) {
            apply_layout(app, &label, &bar);
        }
    }
}

/// Applique la disposition `bar` à la barre `label`.
fn apply_layout(app: &AppHandle, label: &str, bar: &BarLayout) {
    let Some(b) = get(label) else { return };
    let old = b.layout.replace(bar.clone());
    apply_options(&b.window, bar);
    super::pulse::set_bar(label, bar);
    // Plus de widget Lecture en cours : la ligne d'horizon repart du milieu de la barre.
    if !bar.widgets().any(|w| w.kind == horizon_core::layout::WidgetKind::MEDIA) {
        b.window.global::<super::PulseState>().set_origin_x(-1.0);
    }
    // Volet ouvert devenu sans objet : mode changé, ou plus de tuile (hors édition).
    if b.drawer_open.get() && (bar.tiles_mode != old.tiles_mode || (bar.tiles.is_empty() && !b.window.get_editing())) {
        close_drawer(label);
    }
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
    sync_plugins(label, bar);
    log::info!("{label} : disposition mise à jour");
    schedule_fit(label);
}

/// Widgets de plugins de la bande de la barre `label` : leurs exemplaires rendus hors écran.
fn sync_plugins(label: &str, bar: &BarLayout) {
    let Some(b) = get(label) else { return };
    let list: Vec<(String, String)> = [&bar.left, &bar.center, &bar.right]
        .into_iter()
        .flatten()
        .filter(|w| crate::widgets::manifest(w.kind.as_str()).is_some())
        .filter_map(|w| Some((w.id.clone(), horizon_core::plugin::id_of_kind(w.kind.as_str())?.to_owned())))
        .collect();
    crate::plugins::view::sync_bar(label, b.window.window().scale_factor(), &list);
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
        close_drawer(label);
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
    // Volet ouvert : refermé d'abord (crochet souris retiré s'il était le dernier).
    close_drawer(label);
    super::hint::forget(label);
    super::panels::forget(label);
    super::edit::forget(label);
    crate::plugins::view::forget(label);
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

/// Largeur annoncée des témoins de la barre (au bout de la zone gauche).
const STATUS_ID: &str = "#status";

/// Largeur voulue par chaque zone si `hidden` y est masqué : les largeurs naturelles et un écart
/// avant chaque emplacement affiché sauf le premier. Un widget qui n'a rien à montrer (largeur
/// nulle : Sessions Claude sans session) ne compte pas, comme `.slot:not(:has(*))` ; les témoins
/// de la barre comptent dans la zone gauche.
fn needs(
    b: &BarUi,
    widths: &overflow::Widths,
    compact: bool,
    hidden: &[(Zone, &'static str)],
    limits: &[(String, f32)],
) -> [f32; 3] {
    let gap = b.window.get_gap();
    Zone::ALL.map(|z| {
        let status = if z == Zone::Left { widths.get(compact, STATUS_ID) } else { None };
        let shown: Vec<f32> = b.models[zone_index(z)]
            .iter()
            .filter(|s| !hidden.contains(&(z, s.kind.as_str())))
            .map(|s| {
                // Widget extensible : entier, ou à sa limite.
                let w = if crate::widgets::grows(&s.kind) { widths.full(compact, &s.id) } else { widths.get(compact, &s.id) };
                let limit = limits.iter().find(|(id, _)| id.as_str() == s.id.as_str()).map(|(_, l)| *l);
                limit.map_or(w, |l| w.map(|w| w.min(l))).unwrap_or(0.0)
            })
            .chain(status)
            .filter(|w| *w > 0.0)
            .collect();
        shown.iter().sum::<f32>() + gap * shown.len().saturating_sub(1) as f32
    })
}

/// Masque et écarte les emplacements (ligne par ligne : rien n'est recréé). Un widget vide
/// (largeur naturelle nulle) ne prend pas d'écart et laisse au suivant la place de premier,
/// sauf en mode édition (`editing`), où il garde sa place, nommée.
fn show_slots(b: &BarUi, widths: &overflow::Widths, fit: &Fit, editing: bool) {
    let (compact, hidden) = (fit.compact, &fit.hidden);
    let gap = b.window.get_gap();
    for z in Zone::ALL {
        let model = &b.models[zone_index(z)];
        let mut first = true;
        for row in 0..model.row_count() {
            let Some(mut s) = model.row_data(row) else { continue };
            let limit = fit.limits.iter().find(|(id, _)| id.as_str() == s.id.as_str()).map_or(0.0, |(_, l)| *l);
            if s.limit != limit {
                s.limit = limit;
                model.set_row_data(row, s.clone());
            }
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

/// Règle du débordement (`overflow.rs`) : resserrer, puis limiter les widgets extensibles, puis
/// seulement masquer des widgets secondaires si ça ne suffit pas.
fn fit_bar(b: &BarUi, widths: &overflow::Widths, inner: f32, editing: bool) -> Fit {
    let none: &[(String, f32)] = &[];
    let fits = |compact, limits: &[(String, f32)]| !overflow::overflowing(inner, needs(b, widths, compact, &[], limits)).contains(&true);
    if fits(false, none) {
        return Fit::default();
    }
    if editing || fits(true, none) {
        return Fit { compact: true, ..Fit::default() };
    }
    let grows: Vec<overflow::Grow> = Zone::ALL
        .into_iter()
        .flat_map(|z| {
            b.models[zone_index(z)]
                .iter()
                .filter_map(|s| {
                    let min = crate::widgets::grow_min(&s.kind)?;
                    let full = widths.full(true, &s.id).filter(|w| *w > 0.0)?;
                    Some(overflow::Grow { zone: z, id: s.id.to_string(), full, min })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let (limits, still) = overflow::limits(inner, needs(b, widths, true, &[], none), &grows);
    if !still.contains(&true) {
        return Fit { compact: true, limits, hidden: Vec::new() };
    }
    let mut fit = overflow::fit(
        crate::widgets::SQUEEZE_ORDER,
        false,
        |compact, hidden| overflow::overflowing(inner, needs(b, widths, compact, hidden, &limits)),
        |z, kind| b.models[zone_index(z)].iter().any(|s| s.kind == kind),
    );
    fit.limits = limits;
    fit
}

/// Applique la règle du débordement, d'après les largeurs annoncées.
fn run_fit(label: &str) {
    let Some(b) = get(label) else { return };
    b.fit_pending.set(false);
    let widths = b.widths.borrow().clone();
    let inner = b.window.get_inner_width();
    let editing = super::edit::editing(label);
    let fit = fit_bar(&b, &widths, inner, editing);
    show_slots(&b, &widths, &fit, editing);
    if fit != *b.fit.borrow() {
        log::info!(
            "{label} : {}",
            if fit.compact {
                format!("trop chargée, resserrée ; limités : {:?} ; masqués : {:?}", fit.limits, fit.hidden)
            } else {
                "tout tient".to_owned()
            }
        );
        *b.fit.borrow_mut() = fit;
    }
}
