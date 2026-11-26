//! Minuteur et pomodoro (panneau de l'horloge). L'état vit ici, partagé par toutes les barres et
//! conservé quand elles se rechargent ; les barres affichent le compte à rebours à partir de
//! l'échéance. Fin ou changement de phase : alerte dans l'îlot (et événement `timer` des plugins : Prisme y joue un bref signal).
//!
//! Repris de `app/src-tauri/src/timer.rs`, avec les calculs d'affichage de `app/src/lib/timer.ts`
//! (temps restant, part écoulée, titre, durée libre saisie).

use crate::alerts::{Alert, Severity};
use crate::events::EV_TIMER;
use horizon_core::dnd::AlertKind;
use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{Waker, start_thread};
use horizon_core::time::now_ms;
use horizon_core::timer::{Phase, Timer, TimerEvent, TimerView, duration_label};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;

/// Le fil se réveille au moins à ce rythme : une veille ne fait pas manquer l'échéance.
const MAX_SLEEP: Duration = Duration::from_secs(30);
/// Durée la plus longue d'un minuteur (`horizon_core::timer::MAX_MINUTES`).
const MAX_MINUTES: u32 = 600;

#[derive(Default)]
pub struct TimerHub {
    timer: Mutex<Timer>,
    wake: Waker,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimerPayload {
    pub timer: Option<TimerView>,
    /// Option « NPD pendant la séance ».
    pub dnd: bool,
    /// Durées proposées (minutes) et pomodoro (travail, pause), réglés dans `[timer]`.
    pub presets: Vec<u32>,
    pub pomodoro: (u32, u32),
}

fn lock(app: &AppHandle) -> std::sync::MutexGuard<'_, Timer> {
    crate::util::lock(&app.state::<TimerHub>().inner().timer)
}

fn payload(app: &AppHandle) -> TimerPayload {
    let t = crate::state::settings().timer;
    TimerPayload { timer: lock(app).view(), dnd: crate::dnd::timer_dnd(app), presets: t.presets, pomodoro: (t.focus, t.rest) }
}

/// Réglages modifiés : les panneaux reçoivent les nouvelles durées.
pub fn broadcast(app: &AppHandle) {
    let _ = app.emit(EV_TIMER, payload(app));
    crate::widgets::phone::actions_changed(app);
}

/// NPD de séance selon l'option et la phase en cours.
pub fn sync_session(app: &AppHandle) {
    let on = crate::dnd::timer_dnd(app) && lock(app).wants_dnd();
    crate::dnd::set_session(app, on);
}

/// Après un changement : diffusion, NPD de séance, nouvelle échéance pour le fil.
fn changed(app: &AppHandle) {
    let _ = app.emit(EV_TIMER, payload(app));
    sync_session(app);
    // Après le NPD de séance : les téléphones reçoivent l'un et l'autre à jour.
    crate::widgets::phone::actions_changed(app);
    app.state::<TimerHub>().wake.wake();
}

/// Titre et texte de l'alerte d'une échéance.
fn announcement(ev: TimerEvent) -> (String, String) {
    match ev {
        TimerEvent::Finished { total_ms } => ("Minuteur terminé".to_owned(), format!("{} écoulées", duration_label(total_ms))),
        TimerEvent::Phase { phase: Phase::Break, total_ms, cycle } => {
            ("Pomodoro : pause".to_owned(), format!("Séance {cycle} terminée — {} de pause", duration_label(total_ms)))
        }
        TimerEvent::Phase { total_ms, cycle, .. } => {
            ("Pomodoro : au travail".to_owned(), format!("Séance {cycle} — {}", duration_label(total_ms)))
        }
    }
}

fn announce(app: &AppHandle, ev: TimerEvent) {
    let (title, body) = announcement(ev);
    log::info!("minuteur : {title} ({body})");
    crate::plugins::events::emit(app, "timer", serde_json::json!({ "state": "done", "title": title }));
    crate::alerts::show(app, AlertKind::Timer, Alert::icon("timer", Severity::Attention, title, body));
}

/// Fil des échéances : dort jusqu'à la prochaine (au plus `MAX_SLEEP`) ou jusqu'à un
/// changement.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("minuteur", move || {
        let hub = app.state::<TimerHub>();
        loop {
            let deadline = lock(&app).deadline();
            let wait = deadline.map_or(MAX_SLEEP, |d| Duration::from_millis(d.saturating_sub(now_ms())).min(MAX_SLEEP));
            if hub.wake.wait(wait) {
                continue;
            }
            let ev = lock(&app).tick(now_ms());
            if let Some(ev) = ev {
                announce(&app, ev);
                let _ = app.emit(EV_TIMER, payload(&app));
                sync_session(&app);
            }
        }
    })
}

pub fn timer_state(app: &AppHandle) -> TimerPayload {
    payload(app)
}

/// Minuteur simple de `minutes` minutes (remplace celui en cours).
pub fn timer_start(app: &AppHandle, minutes: u32) {
    lock(app).start(minutes, now_ms());
    changed(app);
}

pub fn timer_pomodoro(app: &AppHandle) {
    let t = crate::state::settings().timer;
    lock(app).pomodoro(t.focus, t.rest, now_ms());
    changed(app);
}

/// Pause (`true`) ou reprise (`false`).
pub fn timer_pause(app: &AppHandle, pause: bool) {
    {
        let mut t = lock(app);
        if pause { t.pause(now_ms()) } else { t.resume(now_ms()) }
    }
    changed(app);
}

pub fn timer_stop(app: &AppHandle) {
    lock(app).stop();
    changed(app);
}

/// « +5 min » : prolonge la phase en cours.
pub fn timer_extend(app: &AppHandle, minutes: u32) {
    lock(app).extend(minutes);
    changed(app);
}

/// Minuteur en pause (`Some(true)`), en marche (`Some(false)`) ou absent (palette).
pub fn paused(app: &AppHandle) -> Option<bool> {
    lock(app).paused()
}

// ---- Affichage (lib/timer.ts) ----

pub fn is_paused(t: &TimerView) -> bool {
    t.ends_at_ms.is_none()
}

/// Temps restant à l'instant `now`.
pub fn remaining(t: &TimerView, now: u64) -> u64 {
    t.remaining_ms.unwrap_or_else(|| t.ends_at_ms.unwrap_or(now).saturating_sub(now))
}

/// Part écoulée, de 0 à 1.
pub fn progress(t: &TimerView, now: u64) -> f32 {
    if t.total_ms == 0 {
        return 1.0;
    }
    (1.0 - remaining(t, now) as f64 / t.total_ms as f64).clamp(0.0, 1.0) as f32
}

/// Compte à rebours affiché (« 4:59 », « 1:02:03 ») : la seconde entamée compte.
pub fn countdown(ms: u64) -> String {
    crate::ui::format::clock(ms.div_ceil(1000) * 1000)
}

pub fn phase_label(t: &TimerView) -> String {
    match t.phase {
        Phase::Focus => format!("Pomodoro · séance {}", t.cycle),
        Phase::Break => "Pomodoro · pause".to_owned(),
        Phase::Timer => "Minuteur".to_owned(),
    }
}

/// Durée libre saisie dans le panneau, en minutes : « 12 », « 12 min », « 1 h 30 », « 1h »,
/// « 1:30 ». `None` si illisible ou hors de 1 min à 10 h.
pub fn parse_minutes(text: &str) -> Option<u32> {
    let t: String = text.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let digits = |s: &str, min: usize, max: usize| {
        (s.len() >= min && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse::<u32>().unwrap_or(0))
    };
    // « 1h30 », « 1:30 », « 1h », « 1h30min ».
    let hm = t.find(['h', ':']).and_then(|i| {
        let rest = &t[i + 1..];
        let rest = rest.strip_suffix("min").unwrap_or(rest);
        Some(digits(&t[..i], 1, 2)? * 60 + if rest.is_empty() { 0 } else { digits(rest, 1, 2)? })
    });
    let m = match hm {
        Some(m) => m,
        None => {
            let num = ["min", "mn", "m"].iter().find_map(|s| t.strip_suffix(s)).unwrap_or(&t);
            digits(num, 1, 3)?
        }
    };
    (1..=MAX_MINUTES).contains(&m).then_some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(ends: Option<u64>, rem: Option<u64>) -> TimerView {
        TimerView { phase: Phase::Timer, total_ms: 60_000, ends_at_ms: ends, remaining_ms: rem, cycle: 0 }
    }

    #[test]
    fn free_durations() {
        for (text, m) in [
            ("12", Some(12)),
            (" 12 min ", Some(12)),
            ("12mn", Some(12)),
            ("12m", Some(12)),
            ("1 h 30", Some(90)),
            ("1h", Some(60)),
            ("1:30", Some(90)),
            ("1h05min", Some(65)),
            ("10h", Some(600)),
            ("600", Some(600)),
            ("601", None),
            ("0", None),
            ("", None),
            ("abc", None),
            ("1h300", None),
            ("1234", None),
            ("h30", None),
            ("1.5", None),
        ] {
            assert_eq!(parse_minutes(text), m, "{text:?}");
        }
    }

    #[test]
    fn remaining_and_progress() {
        let running = view(Some(100_000), None);
        assert_eq!(remaining(&running, 70_000), 30_000);
        assert_eq!(remaining(&running, 200_000), 0);
        assert!((progress(&running, 70_000) - 0.5).abs() < 1e-6);
        let paused = view(None, Some(15_000));
        assert!(is_paused(&paused) && !is_paused(&running));
        assert_eq!(remaining(&paused, 999_999), 15_000);
        assert!((progress(&paused, 0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn labels() {
        assert_eq!(countdown(4 * 60_000 + 59_001), "5:00");
        assert_eq!(countdown(61_000), "1:01");
        assert_eq!(countdown(3_723_000), "1:02:03");
        let focus = TimerView { phase: Phase::Focus, cycle: 2, ..view(None, Some(1)) };
        assert_eq!(phase_label(&focus), "Pomodoro · séance 2");
        assert_eq!(phase_label(&TimerView { phase: Phase::Break, ..focus.clone() }), "Pomodoro · pause");
        assert_eq!(phase_label(&view(None, Some(1))), "Minuteur");
        let (title, body) = announcement(TimerEvent::Finished { total_ms: 25 * 60_000 });
        assert_eq!((title.as_str(), body.as_str()), ("Minuteur terminé", "25 min écoulées"));
    }
}
