//! Horloge (global `ClockData` de `ui/widgets/clock/widget.slint`) : heure mise à jour au
//! début de chaque minute, date en français pour les exemplaires qui la montrent (`showDate`),
//! minuteur en cours à côté (compte à rebours revu chaque seconde, seulement tant qu'il tourne).
//! Le minuteur est au cœur (`crate::timer`) ; son panneau : `panel.rs`.

mod panel;

use crate::events::EV_TIMER;
use crate::host::{self, AppHandle};
use crate::timer::{self, TimerPayload};
use crate::ui::{BarWindow, ClockData};
use crate::widgets::Backend;
use horizon_core::timer::{Phase, TimerView};
use chrono::{Datelike, Local, NaiveDate, Timelike};
use horizon_core::layout::BarLayout;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::time::Duration;

pub const BACKEND: Backend =
    Backend { id: "clock", name: "horloge", bind: Some(bind), panel: Some(panel::PANEL), ..Backend::NONE };

thread_local! {
    static WINDOWS: RefCell<Vec<slint::Weak<BarWindow>>> = RefCell::default();
    /// Prochain début de minute.
    static TIMER: slint::Timer = slint::Timer::default();
    /// Minuteur en cours (vu par les barres).
    static RUNNING: RefCell<Option<TimerView>> = const { RefCell::new(None) };
    /// Compte à rebours des barres, chaque seconde tant que le minuteur tourne.
    static COUNTDOWN: slint::Timer = slint::Timer::default();
}

const DAYS: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];
const MONTHS: [&str; 12] =
    ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre"];

/// « lundi 17 novembre » (`toLocaleDateString('fr-FR', { weekday: 'long', day: 'numeric',
/// month: 'long' })`).
pub(crate) fn date_fr(d: NaiveDate) -> String {
    format!("{} {} {}", DAYS[d.weekday().num_days_from_monday() as usize], d.day(), MONTHS[d.month0() as usize])
}

fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let show: Vec<bool> = super::instances(bar, "clock").map(|w| w.settings["showDate"] == true).collect();
    window.global::<ClockData>().set_show_date(ModelRc::new(VecModel::from(show)));
    // Liaison refaite (disposition modifiée) : la barre n'est comptée qu'une fois.
    WINDOWS.with_borrow_mut(|w| {
        w.retain(|x| x.upgrade().is_some_and(|x| !std::ptr::eq(x.window(), window.window())));
        w.push(window.as_weak());
    });
    tick();
    RUNNING.set(timer::timer_state(app).timer);
    show_timer();
    host::listen(EV_TIMER, Some(label), |e| {
        if let Some(p) = e.payload::<TimerPayload>() {
            // Chaque barre reçoit l'événement : la première suffit à poser l'état commun.
            if RUNNING.with_borrow(|r| *r != p.timer) {
                RUNNING.set(p.timer.clone());
                show_timer();
            }
        }
    });
}

/// Pose le minuteur en cours sur chaque barre ; le compte à rebours avance chaque seconde tant
/// qu'il tourne.
fn show_timer() {
    let view = RUNNING.with_borrow(Clone::clone);
    let now = horizon_core::time::now_ms();
    WINDOWS.with_borrow(|list| {
        for w in list.iter().filter_map(slint::Weak::upgrade) {
            let g = w.global::<ClockData>();
            g.set_timer(view.is_some());
            if let Some(t) = &view {
                g.set_timer_text(timer::countdown(timer::remaining(t, now)).into());
                g.set_timer_rest(t.phase == Phase::Break);
                g.set_timer_paused(timer::is_paused(t));
            }
        }
    });
    let run = view.as_ref().is_some_and(|t| !timer::is_paused(t));
    COUNTDOWN.with(|c| {
        if !run {
            c.stop();
        } else if !c.running() {
            c.start(slint::TimerMode::Repeated, Duration::from_secs(1), show_timer);
        }
    });
}

/// Pose l'heure sur chaque barre, puis se reprogramme au début de la minute suivante.
fn tick() {
    let now = Local::now();
    let (hours, minutes, date) = (format!("{:02}", now.hour()), format!("{:02}", now.minute()), date_fr(now.date_naive()));
    let alive = WINDOWS.with_borrow_mut(|list| {
        list.retain(|w| w.upgrade().is_some());
        for w in list.iter().filter_map(slint::Weak::upgrade) {
            let g = w.global::<ClockData>();
            g.set_hours(hours.as_str().into());
            g.set_minutes(minutes.as_str().into());
            g.set_date(date.as_str().into());
        }
        !list.is_empty()
    });
    if alive {
        let into_minute = u64::from(now.second()) * 1000 + u64::from(now.timestamp_subsec_millis().min(999));
        let wait = Duration::from_millis(60_000 - into_minute + 50);
        TIMER.with(|t| t.start(slint::TimerMode::SingleShot, wait, tick));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french_dates() {
        assert_eq!(date_fr(NaiveDate::from_ymd_opt(2026, 11, 17).unwrap()), "mardi 17 novembre");
        assert_eq!(date_fr(NaiveDate::from_ymd_opt(2026, 8, 1).unwrap()), "samedi 1 août");
    }
}
