//! Fenêtre de la palette (`ui/palette.slint`), sur le fil de l'interface : fenêtre sans cadre
//! transparente, au premier plan, absente de la barre des tâches et d'Alt+Tab mais activable
//! (la saisie a besoin du clavier), créée au démarrage et seulement cachée ensuite. Chaque
//! ouverture la remet à zéro (recherche vide, première ligne) ; le catalogue reconstruit en
//! arrière-plan (`EV_PALETTE_ENTRIES`) relance la recherche en cours en gardant la ligne choisie.
//! Règles et actions : `crate::palette` (repris de `app/src-tauri/src/palette/`).

use super::{PaletteRow, PaletteSeg, PaletteWindow};
use crate::events::EV_PALETTE_ENTRIES;
use crate::host::{self, AppHandle};
use crate::palette::{IconRef, ItemView, LABEL, highlight, palette_run, palette_search, step};
use crate::platform::{self, Frame, WindowSpec};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Durée de l'état « apparition » : la première image est présentée en haut et transparente,
/// puis le fondu et le glissement d'entrée jouent (`play` de PaletteApp.svelte).
const ENTER: Duration = Duration::from_millis(30);

struct Pal {
    app: AppHandle,
    window: PaletteWindow,
    rows: Rc<VecModel<PaletteRow>>,
    items: Vec<ItemView>,
    sel: usize,
    act: usize,
    visible: bool,
}

thread_local! {
    static PAL: RefCell<Option<Pal>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Pal) -> R) -> Option<R> {
    PAL.with_borrow_mut(|p| p.as_mut().map(f))
}

/// Crée la fenêtre (cachée).
pub fn create(app: &AppHandle) -> anyhow::Result<()> {
    let window = PaletteWindow::new()?;
    let spec = WindowSpec {
        title: "Horizon — palette".into(),
        frame: Frame::Layered,
        rect: Some((0, 0, 1, 1)),
        focusable: true,
        topmost: true,
        taskbar: false,
        ..Default::default()
    };
    let hwnd = platform::configure(window.window(), spec)?;
    app.register_window(LABEL, hwnd);
    let rows = Rc::new(VecModel::default());
    window.set_items(ModelRc::from(rows.clone()));
    window.on_search(|q| search(&q, false));
    window.on_select(|i| select(i.max(0) as usize));
    window.on_run(|i, a| run(i.max(0) as usize, a.max(0) as usize));
    window.on_nav(|k| nav(&k));
    let a = app.clone();
    // Clic ailleurs (la fenêtre perd le premier plan) : fermeture, différée (Windows prévient
    // pendant l'affichage).
    platform::on_active(window.window(), move |active| {
        if !active {
            let a = a.clone();
            slint::Timer::single_shot(Duration::ZERO, move || crate::palette::deactivated(&a));
        }
    });
    host::listen(EV_PALETTE_ENTRIES, Some(LABEL), |_| {
        if is_visible() {
            let q = with(|p| p.window.get_query()).unwrap_or_default();
            search(&q, true);
        }
    });
    super::accent::follow(&window);
    PAL.set(Some(Pal { app: app.clone(), window, rows, items: Vec::new(), sel: 0, act: 0, visible: false }));
    Ok(())
}

/// La palette est affichée.
pub fn is_visible() -> bool {
    with(|p| p.visible).unwrap_or(false)
}

/// Affiche la palette au rectangle `rect` (pixels physiques), remise à zéro, clavier au champ.
pub fn show_window(rect: (i32, i32, u32, u32)) {
    let Some(window) = with(|p| {
        p.visible = true;
        p.window.clone_strong()
    }) else {
        return;
    };
    let spec = WindowSpec {
        title: "Horizon — palette".into(),
        frame: Frame::Layered,
        rect: Some(rect),
        focusable: true,
        topmost: true,
        taskbar: false,
        ..Default::default()
    };
    if let Err(e) = platform::configure(window.window(), spec) {
        log::warn!("palette : {e:#}");
    }
    window.set_query("".into());
    search("", false);
    window.set_entering(!crate::motion::reduced());
    window.set_open(true);
    if let Err(e) = window.show() {
        log::warn!("palette : affichage : {e}");
        return;
    }
    platform::activate(window.window());
    window.invoke_focus_input();
    let w = window.as_weak();
    slint::Timer::single_shot(ENTER, move || {
        if let Some(w) = w.upgrade() {
            w.set_entering(false);
        }
    });
}

/// Cache la palette ; vrai si elle était affichée.
pub fn hide_window() -> bool {
    let Some((window, was)) = with(|p| (p.window.clone_strong(), std::mem::replace(&mut p.visible, false))) else {
        return false;
    };
    if was {
        let _ = window.hide();
        window.set_open(false);
        super::tooltip::hide();
        with(|p| {
            p.items.clear();
            p.rows.set_vec(Vec::new());
        });
    }
    was
}

/// Arrêt d'Horizon : la fenêtre est cachée (et lâchée par Slint).
pub fn stop() {
    if let Some(p) = PAL.take() {
        let _ = p.window.hide();
    }
}

/// Recherche ; `keep` : le catalogue a changé pendant la saisie, la ligne choisie le reste.
fn search(query: &str, keep: bool) {
    let Some(app) = with(|p| p.app.clone()) else { return };
    let found = palette_search(&app, query);
    with(|p| {
        let chosen = keep.then(|| p.items.get(p.sel).map(|it| it.id.clone())).flatten();
        let i = chosen.and_then(|id| found.iter().position(|it| it.id == id));
        if i != Some(p.sel) {
            p.act = 0;
        }
        p.sel = i.unwrap_or(0);
        p.items = found;
        let q = query.trim();
        p.window.set_empty_text(if q.is_empty() { "Aucun résultat".into() } else { format!("Aucun résultat pour « {q} »").into() });
    });
    refresh();
}

/// Lignes de la fenêtre d'après les résultats (de nouveau quand une icône arrive).
fn refresh() {
    let Some((items, sel, act)) = with(|p| (p.items.clone(), p.sel, p.act)) else { return };
    let rows: Vec<PaletteRow> = items.iter().map(row).collect();
    with(|p| {
        super::model::sync(&p.rows, rows);
        p.window.set_sel(sel as i32);
        p.window.set_act(act as i32);
    });
}

fn row(it: &ItemView) -> PaletteRow {
    let (icon, tint_mode, tint) = icon(&it.icon);
    let segs: Vec<PaletteSeg> = highlight(&it.title, &it.hits).into_iter().map(|(text, hit)| PaletteSeg { text: text.into(), hit }).collect();
    let actions: Vec<SharedString> = it.actions.iter().map(|a| a.as_str().into()).collect();
    PaletteRow {
        id: it.id.as_str().into(),
        segs: ModelRc::new(VecModel::from(segs)),
        subtitle: it.subtitle.as_str().into(),
        icon,
        tint_mode,
        tint,
        actions: ModelRc::new(VecModel::from(actions)),
    }
}

/// Icône d'un résultat (PaletteIcon.svelte) : icône d'interface, icône de raccourci (logo, image,
/// icône d'exe ou de .lnk), icône de suggestion (favicon, appli) ; « sparkle » en attendant.
fn icon(r: &IconRef) -> (Image, i32, slint::Color) {
    let glyph = |name: &str| (super::icons::image(name), 2, slint::Color::default());
    let redraw = |_: &Image| refresh();
    match r {
        IconRef::Glyph { name } => glyph(name),
        IconRef::Shortcut { item } => crate::widgets::shortcuts::look(item, redraw).unwrap_or_else(|| glyph("sparkle")),
        IconRef::Suggestion { key } => {
            let Some(app) = with(|p| p.app.clone()) else { return glyph("sparkle") };
            let k = key.clone();
            super::images::get(&format!("suggestion:{key}"), move || crate::widgets::suggestions::suggestion_icon(&app, &k), redraw)
                .map(|i| (i, 0, slint::Color::default()))
                .unwrap_or_else(|| glyph("sparkle"))
        }
    }
}

fn select(i: usize) {
    with(|p| {
        if i != p.sel && i < p.items.len() {
            p.sel = i;
            p.act = 0;
            p.window.set_sel(i as i32);
            p.window.set_act(0);
        }
    });
}

fn run(i: usize, a: usize) {
    let Some((app, id)) = with(|p| p.items.get(i).filter(|it| a < it.actions.len()).map(|it| (p.app.clone(), it.id.clone()))).flatten() else {
        return;
    };
    if let Err(e) = palette_run(&app, &id, a) {
        log::warn!("palette {id} : {e}");
    }
}

/// Touches de navigation (↑ ↓ Tab Maj+Tab Entrée Échap).
fn nav(key: &str) {
    let Some((app, sel, act, n, actions)) =
        with(|p| (p.app.clone(), p.sel, p.act, p.items.len(), p.items.get(p.sel).map_or(0, |it| it.actions.len())))
    else {
        return;
    };
    match key {
        "down" | "up" => select(step(sel, n, if key == "down" { 1 } else { -1 })),
        "tab" | "backtab" if actions > 1 => {
            let next = step(act, actions, if key == "tab" { 1 } else { -1 });
            with(|p| {
                p.act = next;
                p.window.set_act(next as i32);
            });
        }
        "enter" => run(sel, act),
        "escape" => crate::palette::hide(&app, true),
        _ => {}
    }
}
