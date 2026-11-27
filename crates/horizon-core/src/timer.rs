//! Minuteur, pomodoro et chronomètre (clic sur l'horloge) : une seule séance à la fois. Heures
//! en millisecondes depuis l'époque Unix : l'échéance reste juste après une veille, et les
//! barres calculent elles-mêmes le temps restant (`ends_at_ms`) ou écoulé (`started_at_ms`).

use crate::geometry::{MonitorInfo, Rect};
use serde::{Deserialize, Serialize};

/// Durée d'un minuteur : 1 minute à 10 heures.
pub(crate) const MIN_MINUTES: u32 = 1;
pub(crate) const MAX_MINUTES: u32 = 600;
const MINUTE: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Minuteur simple.
    Timer,
    /// Pomodoro : travail.
    Focus,
    /// Pomodoro : pause.
    Break,
    /// Chronomètre (compte le temps écoulé, sans échéance).
    Stopwatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Countdown,
    Pomodoro { focus_ms: u64, break_ms: u64 },
    Stopwatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    /// Compte à rebours en marche.
    Running { ends_at_ms: u64 },
    /// Compte à rebours en pause.
    Paused { remaining_ms: u64 },
    /// Chronomètre en marche : départ ramené à l'instant où il aurait commencé sans pause.
    Counting { since_ms: u64 },
    /// Chronomètre en pause.
    Held { elapsed_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Session {
    plan: Plan,
    phase: Phase,
    total_ms: u64,
    run: Run,
    /// Pomodoro : numéro de la séance de travail (1, 2…).
    cycle: u32,
}

/// Ce qui vient de se passer à l'échéance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerEvent {
    /// Minuteur simple terminé.
    Finished { total_ms: u64 },
    /// Pomodoro : nouvelle phase commencée.
    Phase { phase: Phase, total_ms: u64, cycle: u32 },
}

/// État envoyé aux barres (`horizon://timer`) ; `None` = pas de minuteur.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimerView {
    pub phase: Phase,
    /// Durée de la phase (0 pour le chronomètre).
    pub total_ms: u64,
    /// Échéance (en cours) ou `None` (en pause, chronomètre).
    pub ends_at_ms: Option<u64>,
    /// Temps restant figé pendant une pause.
    pub remaining_ms: Option<u64>,
    /// Chronomètre en marche : instant de départ (pauses déduites).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// Chronomètre en pause : temps écoulé figé.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    pub cycle: u32,
}

impl TimerView {
    pub fn is_stopwatch(&self) -> bool {
        self.phase == Phase::Stopwatch
    }

    /// En pause : rien n'avance (ni échéance ni départ).
    pub fn is_paused(&self) -> bool {
        self.ends_at_ms.is_none() && self.started_at_ms.is_none()
    }

    /// Temps restant à l'instant `now` (compte à rebours ; 0 pour le chronomètre).
    pub fn remaining(&self, now: u64) -> u64 {
        self.remaining_ms.unwrap_or_else(|| self.ends_at_ms.map_or(0, |end| end.saturating_sub(now)))
    }

    /// Temps écoulé à l'instant `now` (chronomètre ; 0 pour un compte à rebours).
    pub fn elapsed(&self, now: u64) -> u64 {
        self.elapsed_ms.unwrap_or_else(|| self.started_at_ms.map_or(0, |start| now.saturating_sub(start)))
    }

    /// Part écoulée de la phase, de 0 à 1 (0 pour le chronomètre, qui n'a pas de fin).
    pub fn progress(&self, now: u64) -> f32 {
        if self.is_stopwatch() {
            return 0.0;
        }
        if self.total_ms == 0 {
            return 1.0;
        }
        (1.0 - self.remaining(now) as f64 / self.total_ms as f64).clamp(0.0, 1.0) as f32
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Timer {
    session: Option<Session>,
}

fn minutes(m: u32) -> u64 {
    u64::from(m.clamp(MIN_MINUTES, MAX_MINUTES)) * MINUTE
}

impl Timer {
    /// Minuteur simple de `m` minutes (borné), qui remplace celui en cours.
    pub fn start(&mut self, m: u32, now_ms: u64) {
        let total_ms = minutes(m);
        self.session = Some(Session {
            plan: Plan::Countdown,
            phase: Phase::Timer,
            total_ms,
            run: Run::Running { ends_at_ms: now_ms + total_ms },
            cycle: 0,
        });
    }

    /// Pomodoro : `focus` minutes de travail, `rest` minutes de pause, en boucle.
    pub fn pomodoro(&mut self, focus: u32, rest: u32, now_ms: u64) {
        let (focus_ms, break_ms) = (minutes(focus), minutes(rest));
        self.session = Some(Session {
            plan: Plan::Pomodoro { focus_ms, break_ms },
            phase: Phase::Focus,
            total_ms: focus_ms,
            run: Run::Running { ends_at_ms: now_ms + focus_ms },
            cycle: 1,
        });
    }

    /// Chronomètre lancé à `now_ms` (remplace la séance en cours).
    pub fn stopwatch(&mut self, now_ms: u64) {
        self.session = Some(Session {
            plan: Plan::Stopwatch,
            phase: Phase::Stopwatch,
            total_ms: 0,
            run: Run::Counting { since_ms: now_ms },
            cycle: 0,
        });
    }

    pub fn stop(&mut self) {
        self.session = None;
    }

    /// Chronomètre remis à zéro, en marche comme en pause (sans effet sur un compte à rebours).
    pub fn reset(&mut self, now_ms: u64) {
        let Some(s) = &mut self.session else { return };
        match &mut s.run {
            Run::Counting { since_ms } => *since_ms = now_ms,
            Run::Held { elapsed_ms } => *elapsed_ms = 0,
            Run::Running { .. } | Run::Paused { .. } => {}
        }
    }

    /// Ajoute `m` minutes à la phase en cours (« +5 min »), en pause comme en marche ; la durée
    /// totale reste bornée à `MAX_MINUTES`. Sans effet sur le chronomètre.
    pub fn extend(&mut self, m: u32) {
        let Some(s) = &mut self.session else { return };
        let max = u64::from(MAX_MINUTES) * MINUTE;
        let add = (u64::from(m) * MINUTE).min(max.saturating_sub(s.total_ms));
        match &mut s.run {
            Run::Running { ends_at_ms } => *ends_at_ms += add,
            Run::Paused { remaining_ms } => *remaining_ms += add,
            Run::Counting { .. } | Run::Held { .. } => return,
        }
        s.total_ms += add;
    }

    /// En pause (`Some(true)`), en marche (`Some(false)`), ou pas de séance.
    pub fn paused(&self) -> Option<bool> {
        self.session.map(|s| matches!(s.run, Run::Paused { .. } | Run::Held { .. }))
    }

    pub fn pause(&mut self, now_ms: u64) {
        let Some(s) = &mut self.session else { return };
        s.run = match s.run {
            Run::Running { ends_at_ms } => Run::Paused { remaining_ms: ends_at_ms.saturating_sub(now_ms) },
            Run::Counting { since_ms } => Run::Held { elapsed_ms: now_ms.saturating_sub(since_ms) },
            paused => paused,
        };
    }

    pub fn resume(&mut self, now_ms: u64) {
        let Some(s) = &mut self.session else { return };
        s.run = match s.run {
            Run::Paused { remaining_ms } => Run::Running { ends_at_ms: now_ms + remaining_ms },
            Run::Held { elapsed_ms } => Run::Counting { since_ms: now_ms.saturating_sub(elapsed_ms) },
            running => running,
        };
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// Le NPD de séance s'applique : minuteur, chronomètre ou travail du pomodoro (pas pendant
    /// la pause).
    pub fn wants_dnd(&self) -> bool {
        self.session.is_some_and(|s| s.phase != Phase::Break)
    }

    /// Prochaine échéance (pour dormir jusque-là) ; `None` en pause, pour le chronomètre ou
    /// sans séance.
    pub fn deadline(&self) -> Option<u64> {
        match self.session?.run {
            Run::Running { ends_at_ms } => Some(ends_at_ms),
            Run::Paused { .. } | Run::Counting { .. } | Run::Held { .. } => None,
        }
    }

    /// À appeler à l'échéance (ou plus tard, après une veille). Le pomodoro enchaîne sa phase
    /// suivante à partir de l'échéance passée, ou de maintenant si une phase entière a été
    /// manquée (veille).
    pub fn tick(&mut self, now_ms: u64) -> Option<TimerEvent> {
        let s = self.session.as_mut()?;
        let Run::Running { ends_at_ms } = s.run else { return None };
        if now_ms < ends_at_ms {
            return None;
        }
        match s.plan {
            Plan::Countdown => {
                let total_ms = s.total_ms;
                self.session = None;
                Some(TimerEvent::Finished { total_ms })
            }
            Plan::Pomodoro { focus_ms, break_ms } => {
                let (phase, total_ms) = match s.phase {
                    Phase::Focus => (Phase::Break, break_ms),
                    _ => (Phase::Focus, focus_ms),
                };
                if phase == Phase::Focus {
                    s.cycle += 1;
                }
                let from = if now_ms - ends_at_ms >= total_ms { now_ms } else { ends_at_ms };
                s.phase = phase;
                s.total_ms = total_ms;
                s.run = Run::Running { ends_at_ms: from + total_ms };
                Some(TimerEvent::Phase { phase, total_ms, cycle: s.cycle })
            }
            // Jamais d'échéance (`Run::Counting` ou `Run::Held`).
            Plan::Stopwatch => None,
        }
    }

    pub fn view(&self) -> Option<TimerView> {
        let s = self.session?;
        let mut v = TimerView {
            phase: s.phase,
            total_ms: s.total_ms,
            ends_at_ms: None,
            remaining_ms: None,
            started_at_ms: None,
            elapsed_ms: None,
            cycle: s.cycle,
        };
        match s.run {
            Run::Running { ends_at_ms } => v.ends_at_ms = Some(ends_at_ms),
            Run::Paused { remaining_ms } => v.remaining_ms = Some(remaining_ms),
            Run::Counting { since_ms } => v.started_at_ms = Some(since_ms),
            Run::Held { elapsed_ms } => v.elapsed_ms = Some(elapsed_ms),
        }
        Some(v)
    }
}

/// Fichier `timer-float.json` : mini-fenêtre du minuteur demandée (elle s'affiche pendant chaque
/// séance), et sa place (coin haut gauche, pixels physiques du bureau virtuel).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FloatFile {
    pub open: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl FloatFile {
    /// Fichier illisible ou absent : fermée, à sa place par défaut.
    pub fn parse(text: &str) -> FloatFile {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn pos(&self) -> Option<(i32, i32)> {
        Some((self.x?, self.y?))
    }
}

/// Écart (pixels logiques) entre la mini-fenêtre posée par défaut et les bords de la zone de
/// travail.
const FLOAT_MARGIN: f32 = 16.0;

/// Carré de la distance du point `(x, y)` au rectangle `r` (0 dedans).
fn distance2(r: &Rect, (x, y): (i32, i32)) -> i64 {
    let dx = i64::from((r.x - x).max(x - (r.x + r.w - 1)).max(0));
    let dy = i64::from((r.y - y).max(y - (r.y + r.h - 1)).max(0));
    dx * dx + dy * dy
}

/// Rectangle (pixels physiques) de la mini-fenêtre de `size` pixels logiques. Coin haut gauche
/// en `saved`, sur l'écran qui le contient (ou le plus proche : écran débranché, fenêtre lâchée
/// à moitié dehors), ramenée tout entière dans la zone de travail de cet écran, à son échelle ;
/// sans place choisie, en haut à droite de l'écran principal. `None` sans écran.
pub fn float_rect(saved: Option<(i32, i32)>, size: (f32, f32), monitors: &[MonitorInfo]) -> Option<Rect> {
    let px = |m: &MonitorInfo, v: f32| (v * m.scale).round() as i32;
    let fit = |m: &MonitorInfo, (x, y): (i32, i32)| {
        let (w, h) = (px(m, size.0), px(m, size.1));
        let work = m.work;
        let x = x.min(work.x + work.w - w).max(work.x);
        let y = y.min(work.y + work.h - h).max(work.y);
        Rect { x, y, w, h }
    };
    if let Some(p) = saved {
        let m = monitors.iter().min_by_key(|m| distance2(&m.rect, p))?;
        return Some(fit(m, p));
    }
    let m = monitors.iter().find(|m| m.primary).or(monitors.first())?;
    let corner = (m.work.x + m.work.w - px(m, size.0 + FLOAT_MARGIN), m.work.y + px(m, FLOAT_MARGIN));
    Some(fit(m, corner))
}

/// Texte d'une durée pour une alerte : « 25 min », « 1 h 30 ».
pub fn duration_label(ms: u64) -> String {
    let m = ms.div_ceil(MINUTE);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_700_000_000_000;

    #[test]
    fn countdown_runs_to_the_end() {
        let mut t = Timer::default();
        t.start(5, T0);
        assert!(t.is_active() && t.wants_dnd());
        assert_eq!(t.deadline(), Some(T0 + 5 * MINUTE));
        assert_eq!(t.tick(T0 + 5 * MINUTE - 1), None);
        assert_eq!(t.tick(T0 + 5 * MINUTE), Some(TimerEvent::Finished { total_ms: 5 * MINUTE }));
        assert!(!t.is_active());
        assert_eq!(t.tick(T0 + 6 * MINUTE), None);
        assert_eq!(t.view(), None);
    }

    #[test]
    fn extend_adds_to_the_running_or_paused_phase() {
        let mut t = Timer::default();
        t.extend(5);
        assert!(!t.is_active(), "rien à prolonger");
        t.start(10, T0);
        t.extend(5);
        assert_eq!(t.deadline(), Some(T0 + 15 * MINUTE));
        assert_eq!(t.view().unwrap().total_ms, 15 * MINUTE);
        assert_eq!(t.paused(), Some(false));
        t.pause(T0 + MINUTE);
        t.extend(5);
        assert_eq!(t.view().unwrap().remaining_ms, Some(19 * MINUTE));
        assert_eq!(t.paused(), Some(true));
        // Jamais au-delà de la durée maximale.
        t.start(MAX_MINUTES, T0);
        t.extend(5);
        assert_eq!(t.view().unwrap().total_ms, u64::from(MAX_MINUTES) * MINUTE);
    }

    #[test]
    fn duration_is_bounded() {
        let mut t = Timer::default();
        t.start(0, T0);
        assert_eq!(t.view().unwrap().total_ms, MINUTE);
        t.start(10_000, T0);
        assert_eq!(t.view().unwrap().total_ms, u64::from(MAX_MINUTES) * MINUTE);
    }

    #[test]
    fn pause_freezes_and_resume_shifts_the_deadline() {
        let mut t = Timer::default();
        t.start(15, T0);
        t.pause(T0 + MINUTE);
        let v = t.view().unwrap();
        assert_eq!((v.ends_at_ms, v.remaining_ms), (None, Some(14 * MINUTE)));
        assert_eq!(t.deadline(), None);
        // Pas d'échéance pendant la pause, même longtemps après.
        assert_eq!(t.tick(T0 + 60 * MINUTE), None);
        t.resume(T0 + 10 * MINUTE);
        assert_eq!(t.deadline(), Some(T0 + 24 * MINUTE));
        // Reprendre deux fois ne décale rien.
        t.resume(T0 + 11 * MINUTE);
        assert_eq!(t.deadline(), Some(T0 + 24 * MINUTE));
        t.stop();
        assert!(!t.is_active());
    }

    #[test]
    fn pomodoro_alternates_focus_and_break() {
        let mut t = Timer::default();
        t.pomodoro(25, 5, T0);
        assert_eq!(t.view().unwrap().phase, Phase::Focus);
        assert!(t.wants_dnd());
        let e = t.tick(T0 + 25 * MINUTE).unwrap();
        assert_eq!(e, TimerEvent::Phase { phase: Phase::Break, total_ms: 5 * MINUTE, cycle: 1 });
        assert!(!t.wants_dnd());
        assert_eq!(t.deadline(), Some(T0 + 30 * MINUTE));
        let e = t.tick(T0 + 30 * MINUTE + 200).unwrap();
        assert_eq!(e, TimerEvent::Phase { phase: Phase::Focus, total_ms: 25 * MINUTE, cycle: 2 });
        // Enchaîné sur l'échéance, pas sur l'instant (léger retard du fil).
        assert_eq!(t.deadline(), Some(T0 + 55 * MINUTE));
        assert!(t.wants_dnd());
    }

    #[test]
    fn pomodoro_after_sleep_restarts_from_now() {
        let mut t = Timer::default();
        t.pomodoro(25, 5, T0);
        // Réveil 2 h plus tard : la pause commence maintenant, pas dans le passé.
        let now = T0 + 120 * MINUTE;
        t.tick(now).unwrap();
        assert_eq!(t.deadline(), Some(now + 5 * MINUTE));
    }

    #[test]
    fn view_serializes_for_the_bars() {
        let mut t = Timer::default();
        t.pomodoro(25, 5, T0);
        let v = serde_json::to_value(t.view()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "phase": "focus", "total_ms": 25 * MINUTE, "ends_at_ms": T0 + 25 * MINUTE,
                "remaining_ms": null, "cycle": 1 })
        );
    }

    #[test]
    fn stopwatch_counts_up_through_pauses() {
        let mut t = Timer::default();
        t.stopwatch(T0);
        assert!(t.is_active() && t.wants_dnd());
        assert_eq!((t.paused(), t.deadline()), (Some(false), None));
        // Jamais d'échéance, même des heures plus tard.
        assert_eq!(t.tick(T0 + 600 * MINUTE), None);
        let v = t.view().unwrap();
        assert!(v.is_stopwatch() && !v.is_paused());
        assert_eq!(v.elapsed(T0 + 90_000), 90_000);
        assert_eq!((v.remaining(T0 + 90_000), v.progress(T0 + 90_000)), (0, 0.0));
        // Pause à 2 min, reprise 10 min plus tard : le temps de pause ne compte pas.
        t.pause(T0 + 2 * MINUTE);
        let v = t.view().unwrap();
        assert!(v.is_paused());
        assert_eq!(v.elapsed(T0 + 9 * MINUTE), 2 * MINUTE);
        assert_eq!(t.paused(), Some(true));
        t.resume(T0 + 12 * MINUTE);
        assert_eq!(t.view().unwrap().elapsed(T0 + 13 * MINUTE), 3 * MINUTE);
        // « +5 min » n'a pas de sens ici.
        t.extend(5);
        assert_eq!(t.view().unwrap().elapsed(T0 + 13 * MINUTE), 3 * MINUTE);
        assert_eq!(t.view().unwrap().total_ms, 0);
    }

    #[test]
    fn reset_restarts_the_stopwatch_only() {
        let mut t = Timer::default();
        t.stopwatch(T0);
        t.reset(T0 + 5 * MINUTE);
        assert_eq!(t.view().unwrap().elapsed(T0 + 6 * MINUTE), MINUTE);
        t.pause(T0 + 7 * MINUTE);
        t.reset(T0 + 8 * MINUTE);
        let v = t.view().unwrap();
        assert!(v.is_paused(), "remis à zéro, il reste en pause");
        assert_eq!(v.elapsed(T0 + 20 * MINUTE), 0);
        // Sans effet sur un compte à rebours.
        t.start(10, T0);
        t.reset(T0 + MINUTE);
        assert_eq!(t.deadline(), Some(T0 + 10 * MINUTE));
    }

    #[test]
    fn one_session_at_a_time() {
        let mut t = Timer::default();
        t.start(10, T0);
        t.stopwatch(T0 + MINUTE);
        assert_eq!(t.view().unwrap().phase, Phase::Stopwatch);
        assert_eq!(t.deadline(), None);
        t.pomodoro(25, 5, T0 + 2 * MINUTE);
        assert_eq!(t.view().unwrap().phase, Phase::Focus);
    }

    #[test]
    fn countdown_view_helpers() {
        let mut t = Timer::default();
        t.start(1, T0);
        let v = t.view().unwrap();
        assert_eq!(v.remaining(T0 + 30_000), 30_000);
        assert_eq!(v.remaining(T0 + 200_000), 0);
        assert!((v.progress(T0 + 30_000) - 0.5).abs() < 1e-6);
        assert_eq!(v.elapsed(T0 + 30_000), 0);
        t.pause(T0 + 45_000);
        let v = t.view().unwrap();
        assert!(v.is_paused() && !v.is_stopwatch());
        assert_eq!(v.remaining(T0 + 999_999), 15_000);
        assert!((v.progress(0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn stopwatch_view_serializes_its_own_fields_only() {
        let mut t = Timer::default();
        t.stopwatch(T0);
        let v = serde_json::to_value(t.view()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "phase": "stopwatch", "total_ms": 0, "ends_at_ms": null, "remaining_ms": null,
                "started_at_ms": T0, "cycle": 0 })
        );
    }

    fn screen(x: i32, y: i32, w: i32, h: i32, scale: f32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: format!("{x}"),
            device_name: String::new(),
            rect: Rect { x, y, w, h },
            // Barre réservée en haut : 40 px logiques.
            work: Rect { x, y: y + (40.0 * scale) as i32, w, h: h - (40.0 * scale) as i32 },
            scale,
            primary,
            portrait: false,
        }
    }

    #[test]
    fn float_window_placement() {
        let main = screen(0, 0, 1920, 1080, 1.0, true);
        let big = screen(-3840, -2160, 3840, 2160, 1.5, false);
        let screens = [big.clone(), main.clone()];
        let size = (200.0, 50.0);
        // Par défaut : en haut à droite de l'écran principal, sous la barre.
        assert_eq!(float_rect(None, size, &screens), Some(Rect { x: 1920 - 216, y: 56, w: 200, h: 50 }));
        // Place gardée, à l'échelle de son écran.
        assert_eq!(float_rect(Some((-3000, -1000)), size, &screens), Some(Rect { x: -3000, y: -1000, w: 300, h: 75 }));
        // Débordant à droite ou en bas : ramenée dans la zone de travail.
        assert_eq!(float_rect(Some((1900, 1070)), size, &screens), Some(Rect { x: 1720, y: 1030, w: 200, h: 50 }));
        // Coin sur la barre réservée : poussée sous la barre.
        assert_eq!(float_rect(Some((10, 5)), size, &screens), Some(Rect { x: 10, y: 40, w: 200, h: 50 }));
        // Hors de tout écran (écran débranché, lâchée à moitié dehors) : sur le plus proche.
        assert_eq!(float_rect(Some((5000, 5000)), size, &screens), Some(Rect { x: 1720, y: 1030, w: 200, h: 50 }));
        assert_eq!(float_rect(Some((-4000, -1000)), size, &screens), Some(Rect { x: -3840, y: -1000, w: 300, h: 75 }));
        assert_eq!(float_rect(None, size, &[]), None);
        assert_eq!(float_rect(Some((0, 0)), size, &[]), None);
        // Sans écran principal connu : le premier.
        let other = screen(1920, 0, 1920, 1080, 1.0, false);
        assert_eq!(float_rect(None, size, &[other]).map(|r| r.x), Some(3840 - 216));
    }

    #[test]
    fn float_file_is_forgiving() {
        assert_eq!(FloatFile::parse("pas du json"), FloatFile::default());
        let f = FloatFile::parse(r#"{"open":true,"x":-12,"y":40}"#);
        assert!(f.open);
        assert_eq!(f.pos(), Some((-12, 40)));
        assert_eq!(FloatFile::parse(r#"{"open":true,"x":3}"#).pos(), None);
    }

    #[test]
    fn duration_labels() {
        assert_eq!(duration_label(5 * MINUTE), "5 min");
        assert_eq!(duration_label(60 * MINUTE), "1 h");
        assert_eq!(duration_label(90 * MINUTE), "1 h 30");
        assert_eq!(duration_label(65 * MINUTE), "1 h 05");
    }
}
