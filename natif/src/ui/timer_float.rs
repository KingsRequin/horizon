//! Mini-fenêtre du minuteur (`ui/timer-float.slint`) : le temps restant ou écoulé de la séance en
//! cours, dans une petite fenêtre sans cadre toujours au premier plan, que l'on place n'importe
//! où d'un glisser (sur n'importe quel écran). Elle n'apparaît que pendant une séance, et
//! seulement si elle est demandée (bouton du panneau de l'horloge, palette) ; sa croix la
//! retire. Ce choix et sa place sont gardés par `crate::timer` (`timer-float.json`) ; la place
//! est revue à chaque affichage (`horizon_core::timer::float_rect` : écran débranché, place
//! hors de la zone de travail). Jamais de focus (`WS_EX_NOACTIVATE`) ; le temps est revu chaque
//! seconde, seulement affichée et séance en marche. Fil de l'interface.

use super::TimerFloatWindow;
use crate::events::EV_TIMER;
use crate::host::{self, AppHandle};
use crate::platform::{self, Frame, WindowSpec};
use crate::timer::{self, TimerPayload};
use horizon_core::geometry::Rect;
use horizon_core::timer::{Phase, TimerView, float_rect};
use slint::{ComponentHandle, LogicalSize, PhysicalPosition};
use std::cell::RefCell;
use std::time::Duration;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

pub const LABEL: &str = "timer-float";
/// Taille logique de la fenêtre (`width` et `height` de `TimerFloatWindow`).
const SIZE: (f32, f32) = (248.0, 52.0);

/// Glisser en cours : pointeur et coin de la fenêtre à l'appui (pixels physiques).
#[derive(Clone, Copy)]
struct Drag {
    cursor: (i32, i32),
    origin: (i32, i32),
}

struct Float {
    window: TimerFloatWindow,
    shown: bool,
    drag: Option<Drag>,
    /// Échelle de l'écran de la fenêtre : la taille logique est reposée quand elle change.
    scale: f32,
}

thread_local! {
    static APP: RefCell<Option<AppHandle>> = const { RefCell::new(None) };
    static FLOAT: RefCell<Option<Float>> = const { RefCell::new(None) };
    /// Séance en cours (dernier état reçu).
    static VIEW: RefCell<Option<TimerView>> = const { RefCell::new(None) };
    /// Temps revu chaque seconde, fenêtre affichée et séance en marche.
    static TICK: slint::Timer = slint::Timer::default();
}

/// Suit le minuteur (fil de l'interface, au démarrage) ; la fenêtre est créée à son premier
/// affichage.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    APP.set(Some(app.clone()));
    host::listen(EV_TIMER, None, |e| {
        if let Some(p) = e.payload::<TimerPayload>() {
            update(p);
        }
    });
    update(&timer::timer_state(app));
    Ok(())
}

/// Fenêtre lâchée (arrêt d'Horizon), sur le fil de l'interface.
pub fn stop() {
    TICK.with(slint::Timer::stop);
    if let Some(f) = FLOAT.take() {
        let _ = f.window.hide();
    }
}

fn app() -> Option<AppHandle> {
    APP.with_borrow(Clone::clone)
}

fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY : lecture de la position du pointeur dans une sortie locale.
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

/// Place de la fenêtre : `saved` revue sur les écrans branchés.
fn spot(saved: Option<(i32, i32)>) -> Option<Rect> {
    float_rect(saved, SIZE, &horizon_sys::monitors::list())
}

fn spec(r: Rect) -> WindowSpec {
    WindowSpec {
        title: "Horizon — minuteur".into(),
        frame: Frame::Layered,
        rect: Some((r.x, r.y, r.w.max(1) as u32, r.h.max(1) as u32)),
        focusable: false,
        topmost: true,
        taskbar: false,
        ..Default::default()
    }
}

/// Nouvel état du minuteur : la fenêtre paraît, change ou disparaît.
fn update(p: &TimerPayload) {
    VIEW.set(p.timer.clone());
    let want = p.floating && p.timer.is_some();
    if want {
        show();
    } else {
        hide();
    }
    sync_tick();
}

fn show() {
    let Some(app) = app() else { return };
    let shown = FLOAT.with_borrow(|f| f.as_ref().is_some_and(|f| f.shown));
    if !shown {
        let Some(rect) = spot(timer::float_state(&app).pos()) else { return };
        if FLOAT.with_borrow(Option::is_none)
            && let Err(e) = create(&app, rect)
        {
            log::warn!("mini-fenêtre du minuteur : {e:#}");
            return;
        }
        let Some(window) = FLOAT.with_borrow(|f| f.as_ref().map(|f| f.window.clone_strong())) else { return };
        if let Err(e) = platform::configure(window.window(), spec(rect)) {
            log::warn!("mini-fenêtre du minuteur : {e:#}");
        }
        refresh();
        if let Err(e) = window.show() {
            log::warn!("mini-fenêtre du minuteur : affichage : {e}");
            return;
        }
        let scale = platform::scale(window.window()).unwrap_or(1.0);
        FLOAT.with_borrow_mut(|f| {
            if let Some(f) = f {
                (f.shown, f.scale, f.drag) = (true, scale, None);
            }
        });
    } else {
        refresh();
    }
}

fn hide() {
    let Some(window) = FLOAT.with_borrow_mut(|f| {
        let f = f.as_mut().filter(|f| f.shown)?;
        (f.shown, f.drag) = (false, None);
        Some(f.window.clone_strong())
    }) else {
        return;
    };
    let _ = window.hide();
}

fn create(app: &AppHandle, rect: Rect) -> anyhow::Result<()> {
    let window = TimerFloatWindow::new()?;
    let hwnd = platform::configure(window.window(), spec(rect))?;
    app.register_window(LABEL, hwnd);
    window.global::<super::Tip>().on_show(move |text| super::tooltip::show(hwnd, &text));
    window.global::<super::Tip>().on_hide(super::tooltip::hide);
    let a = app.clone();
    window.on_pause(move |on| act(&a, move |a| timer::timer_pause(a, on)));
    let a = app.clone();
    window.on_stop(move || act(&a, timer::timer_stop));
    let a = app.clone();
    window.on_dismiss(move || {
        super::tooltip::hide();
        act(&a, |a| timer::float_set_open(a, false));
    });
    window.on_drag_start(drag_start);
    window.on_drag_move(drag_move);
    window.on_drag_end(drag_end);
    super::accent::follow(&window);
    FLOAT.set(Some(Float { window, shown: false, drag: None, scale: 1.0 }));
    Ok(())
}

/// Action vers le service, hors du fil de l'interface (verrous, NPD qui prévient Discord).
fn act(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let a = app.clone();
    crate::util::spawn_named("minuteur-action", move || f(&a));
}

/// Temps, titre, piste et pause de la séance, à l'instant.
fn refresh() {
    let now = horizon_core::time::now_ms();
    VIEW.with_borrow(|v| {
        let Some(t) = v else { return };
        FLOAT.with_borrow(|f| {
            let Some(f) = f else { return };
            let w = &f.window;
            let paused = t.is_paused();
            w.set_time(timer::shown(t, now).into());
            // Place comptée : « en pause » remplace la nature de la séance.
            w.set_label(if paused { "En pause".into() } else { timer::phase_label(t).into() });
            w.set_progress(if t.is_stopwatch() { -1.0 } else { t.progress(now) });
            w.set_rest(t.phase == Phase::Break);
            w.set_paused(paused);
        });
    });
}

/// Chaque seconde, seulement fenêtre affichée et séance en marche.
fn sync_tick() {
    let shown = FLOAT.with_borrow(|f| f.as_ref().is_some_and(|f| f.shown));
    let run = shown && VIEW.with_borrow(|v| v.as_ref().is_some_and(|t| !t.is_paused()));
    TICK.with(|t| {
        if !run {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_secs(1), refresh);
        }
    });
}

fn drag_start() {
    let Some(cursor) = cursor() else { return };
    FLOAT.with_borrow_mut(|f| {
        if let Some(f) = f {
            let p = f.window.window().position();
            f.drag = Some(Drag { cursor, origin: (p.x, p.y) });
        }
    });
}

/// La fenêtre suit le pointeur ; passée sur un écran d'une autre échelle, elle reprend sa
/// taille logique (Windows ne redimensionne pas une fenêtre sans cadre).
fn drag_move() {
    let Some((cx, cy)) = cursor() else { return };
    let Some((window, d)) = FLOAT.with_borrow(|f| f.as_ref().and_then(|f| Some((f.window.clone_strong(), f.drag?)))) else {
        return;
    };
    window.window().set_position(PhysicalPosition::new(d.origin.0 + cx - d.cursor.0, d.origin.1 + cy - d.cursor.1));
    keep_size(&window);
}

fn keep_size(window: &TimerFloatWindow) {
    let scale = platform::scale(window.window()).unwrap_or(1.0);
    let changed = FLOAT.with_borrow_mut(|f| f.as_mut().is_some_and(|f| std::mem::replace(&mut f.scale, scale) != scale));
    if changed {
        window.window().set_size(LogicalSize::new(SIZE.0, SIZE.1));
    }
}

/// Relâchement : la fenêtre est ramenée dans la zone de travail de son écran, et sa place
/// gardée.
fn drag_end() {
    let Some(window) = FLOAT.with_borrow_mut(|f| {
        let f = f.as_mut()?;
        f.drag.take()?;
        Some(f.window.clone_strong())
    }) else {
        return;
    };
    let p = window.window().position();
    let Some(r) = spot(Some((p.x, p.y))) else { return };
    if (r.x, r.y) != (p.x, p.y) {
        window.window().set_position(PhysicalPosition::new(r.x, r.y));
        keep_size(&window);
    }
    if let Some(app) = app() {
        act(&app, move |a| timer::float_moved(a, r.x, r.y));
    }
}
