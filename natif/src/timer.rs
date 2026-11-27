//! Minuteur, pomodoro et chronomètre (panneau de l'horloge) : une séance à la fois. L'état vit
//! ici, partagé par toutes les barres et conservé quand elles se rechargent ; les barres
//! affichent le temps restant (ou écoulé) à partir de l'échéance (ou du départ). Fin ou
//! changement de phase : alerte dans l'îlot (et événement `timer` des plugins : Prisme y joue un
//! bref signal). La mini-fenêtre déplaçable (`ui::timer_float`) suit chaque séance quand elle
//! est demandée ; ce choix et sa place sont gardés dans `%APPDATA%\Horizon\timer-float.json`.
//!
//! Repris de `app/src-tauri/src/timer.rs`, avec les calculs d'affichage de `app/src/lib/timer.ts`
//! (titre, durée libre saisie).

use crate::alerts::{Alert, Severity};
use crate::events::EV_TIMER;
use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{Waker, start_thread};
use horizon_core::config::OrderedWriter;
use horizon_core::dnd::AlertKind;
use horizon_core::time::now_ms;
use horizon_core::timer::{FloatFile, Phase, Timer, TimerEvent, TimerView, duration_label};
use serde::Serialize;
use std::sync::Mutex;
use std::time::Duration;

/// Le fil se réveille au moins à ce rythme : une veille ne fait pas manquer l'échéance.
const MAX_SLEEP: Duration = Duration::from_secs(30);
/// Durée la plus longue d'un minuteur (`horizon_core::timer::MAX_MINUTES`).
const MAX_MINUTES: u32 = 600;

pub struct TimerHub {
    timer: Mutex<Timer>,
    wake: Waker,
    /// Mini-fenêtre : demandée ou non, et sa place.
    float: Mutex<FloatFile>,
    writer: OrderedWriter,
}

impl Default for TimerHub {
    fn default() -> Self {
        let float = std::fs::read_to_string(float_path()).map(|t| FloatFile::parse(&t)).unwrap_or_default();
        TimerHub { timer: Mutex::default(), wake: Waker::default(), float: Mutex::new(float), writer: OrderedWriter::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimerPayload {
    pub timer: Option<TimerView>,
    /// Option « NPD pendant la séance ».
    pub dnd: bool,
    /// Durées proposées (minutes) et pomodoro (travail, pause), réglés dans `[timer]`.
    pub presets: Vec<u32>,
    pub pomodoro: (u32, u32),
    /// Mini-fenêtre demandée (affichée pendant chaque séance).
    pub floating: bool,
}

fn lock(app: &AppHandle) -> std::sync::MutexGuard<'_, Timer> {
    crate::util::lock(&app.state::<TimerHub>().inner().timer)
}

fn lock_float(app: &AppHandle) -> std::sync::MutexGuard<'_, FloatFile> {
    crate::util::lock(&app.state::<TimerHub>().inner().float)
}

fn payload(app: &AppHandle) -> TimerPayload {
    let t = crate::state::settings().timer;
    TimerPayload {
        timer: lock(app).view(),
        dnd: crate::dnd::timer_dnd(app),
        presets: t.presets,
        pomodoro: (t.focus, t.rest),
        floating: lock_float(app).open,
    }
}

fn float_path() -> std::path::PathBuf {
    crate::state::app_dir().join("timer-float.json")
}

/// `timer-float.json` réécrit ; `f` change l'état sous le verrou, l'écriture se fait après.
fn save_float(app: &AppHandle, f: impl FnOnce(&mut FloatFile)) {
    let hub = app.state::<TimerHub>();
    let (ticket, text) = {
        let mut file = lock_float(app);
        f(&mut file);
        (hub.inner().writer.ticket(), serde_json::to_string(&*file).unwrap_or_default())
    };
    if let Err(e) = hub.inner().writer.write(&float_path(), ticket, text.as_bytes()) {
        log::warn!("enregistrement de timer-float.json : {e}");
    }
}

/// Mini-fenêtre : demandée ou non, et sa place.
pub fn float_state(app: &AppHandle) -> FloatFile {
    *lock_float(app)
}

/// Mini-fenêtre demandée (bouton du panneau, palette) ou fermée (sa croix).
pub fn float_set_open(app: &AppHandle, open: bool) {
    save_float(app, |f| f.open = open);
    broadcast(app);
}

/// Mini-fenêtre déplacée : sa nouvelle place (coin haut gauche, pixels physiques).
pub fn float_moved(app: &AppHandle, x: i32, y: i32) {
    save_float(app, |f| (f.x, f.y) = (Some(x), Some(y)));
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
                // Fin du minuteur ou nouvelle phase du pomodoro : les téléphones le savent aussi.
                crate::widgets::phone::actions_changed(&app);
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

/// Chronomètre (remplace la séance en cours).
pub fn timer_stopwatch(app: &AppHandle) {
    lock(app).stopwatch(now_ms());
    changed(app);
}

/// Chronomètre remis à zéro (en marche comme en pause).
pub fn timer_reset(app: &AppHandle) {
    lock(app).reset(now_ms());
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

/// Essai sans souris (`horizon.exe --minuteur-essai <quoi>`, fil de l'interface) : `chrono`,
/// `minuteur` (1 min), `pomodoro`, `pause`, `reprise`, `zero`, `stop`, `flottante` / `fixe`
/// (mini-fenêtre demandée ou non), `panneau` (panneau de l'horloge épinglé sur la barre
/// principale).
pub fn essai(app: &AppHandle, what: &str) {
    log::info!("essai du minuteur : {what}");
    if what == "panneau" {
        let state = app.state::<crate::state::AppState>();
        if let Some(label) = state.main_monitor().and_then(|m| crate::edit::label_of(app, &m)) {
            crate::ui::popup::show(&label, "clock", 900.0, 60.0, true);
        }
        return;
    }
    let f: fn(&AppHandle) = match what {
        "chrono" => timer_stopwatch,
        "minuteur" => |a| timer_start(a, 1),
        "pomodoro" => timer_pomodoro,
        "pause" => |a| timer_pause(a, true),
        "reprise" => |a| timer_pause(a, false),
        "zero" => timer_reset,
        "stop" => timer_stop,
        "flottante" => |a| float_set_open(a, true),
        "fixe" => |a| float_set_open(a, false),
        _ => return log::warn!("essai du minuteur inconnu : {what}"),
    };
    let a = app.clone();
    crate::util::spawn_named("minuteur-essai", move || f(&a));
}

/// Séance en cours (palette) : `None` sans séance.
pub fn current(app: &AppHandle) -> Option<TimerView> {
    lock(app).view()
}

// ---- Affichage (lib/timer.ts) ----

/// Compte à rebours affiché (« 4:59 », « 1:02:03 ») : la seconde entamée compte.
pub fn countdown(ms: u64) -> String {
    crate::ui::format::clock(ms.div_ceil(1000) * 1000)
}

/// Temps affiché à l'instant `now` : restant (compte à rebours) ou écoulé (chronomètre, en
/// secondes pleines).
pub fn shown(t: &TimerView, now: u64) -> String {
    if t.is_stopwatch() { crate::ui::format::clock(t.elapsed(now)) } else { countdown(t.remaining(now)) }
}

pub fn phase_label(t: &TimerView) -> String {
    match t.phase {
        Phase::Focus => format!("Pomodoro · séance {}", t.cycle),
        Phase::Break => "Pomodoro · pause".to_owned(),
        Phase::Timer => "Minuteur".to_owned(),
        Phase::Stopwatch => "Chronomètre".to_owned(),
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
        TimerView {
            phase: Phase::Timer,
            total_ms: 60_000,
            ends_at_ms: ends,
            remaining_ms: rem,
            started_at_ms: None,
            elapsed_ms: None,
            cycle: 0,
        }
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
    fn shown_time() {
        // Compte à rebours : la seconde entamée compte.
        assert_eq!(shown(&view(Some(100_000), None), 70_500), "0:30");
        assert_eq!(shown(&view(None, Some(15_000)), 999_999), "0:15");
        // Chronomètre : secondes pleines.
        let mut t = Timer::default();
        t.stopwatch(1_000);
        let v = t.view().unwrap();
        assert_eq!(shown(&v, 1_999), "0:00");
        assert_eq!(shown(&v, 62_000), "1:01");
        assert_eq!(phase_label(&v), "Chronomètre");
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
