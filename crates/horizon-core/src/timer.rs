//! Minuteur et pomodoro (clic sur l'horloge). Heures en millisecondes depuis l'époque Unix :
//! l'échéance reste juste après une veille, et les barres calculent elles-mêmes le temps
//! restant à partir de `ends_at_ms`.

use serde::Serialize;

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    Countdown,
    Pomodoro { focus_ms: u64, break_ms: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Running { ends_at_ms: u64 },
    Paused { remaining_ms: u64 },
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
    pub total_ms: u64,
    /// Échéance (en cours) ou `None` (en pause).
    pub ends_at_ms: Option<u64>,
    /// Temps restant figé pendant une pause.
    pub remaining_ms: Option<u64>,
    pub cycle: u32,
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

    pub fn stop(&mut self) {
        self.session = None;
    }

    /// Ajoute `m` minutes à la phase en cours (« +5 min »), en pause comme en marche ; la durée
    /// totale reste bornée à `MAX_MINUTES`.
    pub fn extend(&mut self, m: u32) {
        let Some(s) = &mut self.session else { return };
        let max = u64::from(MAX_MINUTES) * MINUTE;
        let add = (u64::from(m) * MINUTE).min(max.saturating_sub(s.total_ms));
        s.total_ms += add;
        match &mut s.run {
            Run::Running { ends_at_ms } => *ends_at_ms += add,
            Run::Paused { remaining_ms } => *remaining_ms += add,
        }
    }

    /// En pause (`Some(true)`), en marche (`Some(false)`), ou pas de minuteur.
    pub fn paused(&self) -> Option<bool> {
        self.session.map(|s| matches!(s.run, Run::Paused { .. }))
    }

    pub fn pause(&mut self, now_ms: u64) {
        if let Some(s) = &mut self.session
            && let Run::Running { ends_at_ms } = s.run
        {
            s.run = Run::Paused { remaining_ms: ends_at_ms.saturating_sub(now_ms) };
        }
    }

    pub fn resume(&mut self, now_ms: u64) {
        if let Some(s) = &mut self.session
            && let Run::Paused { remaining_ms } = s.run
        {
            s.run = Run::Running { ends_at_ms: now_ms + remaining_ms };
        }
    }

    pub fn is_active(&self) -> bool {
        self.session.is_some()
    }

    /// Le NPD de séance s'applique : minuteur ou travail du pomodoro (pas pendant la pause).
    pub fn wants_dnd(&self) -> bool {
        self.session.is_some_and(|s| s.phase != Phase::Break)
    }

    /// Prochaine échéance (pour dormir jusque-là) ; `None` en pause ou sans minuteur.
    pub fn deadline(&self) -> Option<u64> {
        match self.session?.run {
            Run::Running { ends_at_ms } => Some(ends_at_ms),
            Run::Paused { .. } => None,
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
        }
    }

    pub fn view(&self) -> Option<TimerView> {
        let s = self.session?;
        let (ends_at_ms, remaining_ms) = match s.run {
            Run::Running { ends_at_ms } => (Some(ends_at_ms), None),
            Run::Paused { remaining_ms } => (None, Some(remaining_ms)),
        };
        Some(TimerView { phase: s.phase, total_ms: s.total_ms, ends_at_ms, remaining_ms, cycle: s.cycle })
    }
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
    fn duration_labels() {
        assert_eq!(duration_label(5 * MINUTE), "5 min");
        assert_eq!(duration_label(60 * MINUTE), "1 h");
        assert_eq!(duration_label(90 * MINUTE), "1 h 30");
        assert_eq!(duration_label(65 * MINUTE), "1 h 05");
    }
}
