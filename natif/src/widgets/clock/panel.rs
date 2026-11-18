//! Panneau du minuteur (global `ClockPanelData` de `ui/widgets/clock/panel.slint`) : état du
//! minuteur du cœur (`crate::timer`), compte à rebours revu chaque seconde seulement panneau
//! ouvert et minuteur en marche (`every(1000)` de la version Tauri), actions vers le service.
//! Fil de l'interface.

use crate::events::EV_TIMER;
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::timer::{self, TimerPayload};
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{ClockPanelData};
use crate::widgets::Panel;
use horizon_core::timer::Phase;
use slint::{Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::time::Duration;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

/// Minutes ajoutées par « +5 min ».
const EXTEND_MIN: u32 = 5;

thread_local! {
    static STATE: RefCell<Option<TimerPayload>> = const { RefCell::new(None) };
    static OPEN: Cell<bool> = const { Cell::new(false) };
    /// Compte à rebours, panneau ouvert et minuteur en marche.
    static TICK: slint::Timer = slint::Timer::default();
}

fn with_data(mut f: impl FnMut(ClockPanelData<'_>)) {
    panel_hosts::each("clock", |h| f(h.global::<ClockPanelData>()));
}

/// Action vers le service, hors du fil de l'interface (verrous, « Ne pas déranger » qui prévient
/// Discord et Prisme).
fn act(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let a = app.clone();
    crate::util::spawn_named("minuteur-action", move || f(&a));
}

fn bind(app: &AppHandle, win: &Host) {
    on_timer(&timer::timer_state(app));
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_TIMER, Some(LABEL), |e| {
            if let Some(p) = e.payload::<TimerPayload>() {
                on_timer(p);
            }
        });
    }

    let g = win.global::<ClockPanelData>();
    g.on_parse(|text| timer::parse_minutes(&text).map_or(0, |m| m as i32));
    let a = app.clone();
    g.on_start(move |m| {
        if let Ok(m) = u32::try_from(m) {
            act(&a, move |a| timer::timer_start(a, m));
        }
    });
    let a = app.clone();
    g.on_pomodoro(move || act(&a, timer::timer_pomodoro));
    let a = app.clone();
    g.on_pause(move |on| act(&a, move |a| timer::timer_pause(a, on)));
    let a = app.clone();
    g.on_extend(move || act(&a, |a| timer::timer_extend(a, EXTEND_MIN)));
    let a = app.clone();
    g.on_stop(move || act(&a, timer::timer_stop));
    let a = app.clone();
    g.on_set_dnd(move |on| act(&a, move |a| crate::dnd::timer_set_dnd(a, on)));
    // Réglages d'Horizon › Minuteur (`settings_open`, page `options:minuteur`).
    let a = app.clone();
    g.on_settings(move || {
        crate::settings_window::open(&a, crate::settings_window::SettingsPage::Options("minuteur".into()));
    });
}

fn opened(app: &AppHandle, _kind: &str) {
    OPEN.set(true);
    // Durées relues (réglages modifiés depuis).
    on_timer(&timer::timer_state(app));
}

fn closed(_app: &AppHandle, _kind: &str) {
    OPEN.set(false);
    sync_tick();
}

fn on_timer(p: &TimerPayload) {
    STATE.set(Some(p.clone()));
    with_data(|g| {
        let presets: Vec<i32> = p.presets.iter().map(|&m| m as i32).collect();
        if g.get_presets().iter().ne(presets.iter().copied()) {
            g.set_presets(ModelRc::new(VecModel::from(presets)));
        }
        g.set_focus_min(p.pomodoro.0 as i32);
        g.set_rest_min(p.pomodoro.1 as i32);
        g.set_dnd(p.dnd);
        g.set_running(p.timer.is_some());
        if let Some(t) = &p.timer {
            let paused = timer::is_paused(t);
            g.set_title(format!("{}{}", timer::phase_label(t), if paused { " · en pause" } else { "" }).into());
            g.set_rest(t.phase == Phase::Break);
            g.set_paused(paused);
        }
    });
    show_countdown();
    sync_tick();
}

/// Compte à rebours et part écoulée, à l'instant.
fn show_countdown() {
    let now = horizon_core::time::now_ms();
    STATE.with_borrow(|s| {
        let Some(t) = s.as_ref().and_then(|p| p.timer.as_ref()) else { return };
        with_data(|g| {
            g.set_countdown(timer::countdown(timer::remaining(t, now)).into());
            g.set_progress(timer::progress(t, now));
        });
    });
}

/// Chaque seconde, seulement panneau ouvert et minuteur en marche.
fn sync_tick() {
    let run = OPEN.get() && STATE.with_borrow(|s| s.as_ref().and_then(|p| p.timer.as_ref()).is_some_and(|t| !timer::is_paused(t)));
    TICK.with(|t| {
        if !run {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_secs(1), show_countdown);
        }
    });
}
