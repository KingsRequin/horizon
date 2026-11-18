//! Sortie de veille : détection (avis de Windows, ou à défaut saut de l'horloge murale),
//! regroupement des avis d'une même reprise, puis attente du réseau avant de tout relancer.

use crate::backoff::Backoff;
use std::time::{Duration, Instant};

/// Écart entre le temps dormi par un fil et l'horloge murale au-delà duquel l'ordinateur est
/// jugé sorti de veille (un fil peut être retardé de quelques secondes sous forte charge).
pub(crate) const CLOCK_JUMP: Duration = Duration::from_secs(30);
/// Les avis reçus moins de tant après une reprise acceptée en font partie (plusieurs avis de
/// Windows, saut d'horloge constaté en plus).
pub(crate) const COALESCE: Duration = Duration::from_secs(60);
/// Délai laissé au réseau après la reprise, avant le premier essai.
pub const FIRST_WAIT: Duration = Duration::from_secs(3);
/// Essais du réseau avant de tout relancer quand même (les widgets afficheront leur erreur).
pub(crate) const MAX_CHECKS: u32 = 6;

/// Vrai si l'horloge murale a avancé bien plus que le temps dormi : l'ordinateur était en
/// veille. `wall` = `None` quand l'horloge a reculé (réglage manuel) : rien à conclure.
pub fn clock_jumped(slept: Duration, wall: Option<Duration>) -> bool {
    wall.is_some_and(|w| w > slept + CLOCK_JUMP)
}

/// Ne laisse passer qu'un avis par reprise.
#[derive(Debug, Default)]
pub struct ResumeGate {
    last: Option<Instant>,
}

impl ResumeGate {
    pub fn accept(&mut self, now: Instant) -> bool {
        if self.last.is_some_and(|t| now.duration_since(t) < COALESCE) {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// Suite d'une reprise : attendre, vérifier le réseau, réessayer avec un délai croissant.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait(Duration),
    /// Relancer tout (réseau prêt, ou essais épuisés).
    Fire,
}

#[derive(Debug, Default)]
pub struct ResumePlan {
    checks: u32,
    backoff: Backoff,
}

impl ResumePlan {
    pub fn new() -> ResumePlan {
        ResumePlan::default()
    }

    /// Résultat d'un essai du réseau.
    pub fn after_check(&mut self, network_ready: bool) -> Step {
        self.checks += 1;
        if network_ready || self.checks >= MAX_CHECKS {
            Step::Fire
        } else {
            Step::Wait(self.backoff.next_delay())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_jump_means_sleep() {
        let tick = Duration::from_secs(10);
        assert!(!clock_jumped(tick, Some(Duration::from_secs(12))));
        assert!(!clock_jumped(tick, Some(tick + CLOCK_JUMP)));
        assert!(clock_jumped(tick, Some(Duration::from_secs(3600))));
        // Horloge reculée à la main : pas une veille.
        assert!(!clock_jumped(tick, None));
    }

    #[test]
    fn several_notices_of_one_resume_fire_once() {
        let t0 = Instant::now();
        let mut g = ResumeGate::default();
        assert!(g.accept(t0));
        assert!(!g.accept(t0 + Duration::from_secs(2)));
        assert!(!g.accept(t0 + COALESCE - Duration::from_millis(1)));
        assert!(g.accept(t0 + COALESCE));
    }

    #[test]
    fn fires_as_soon_as_network_is_ready() {
        let mut p = ResumePlan::new();
        assert_eq!(p.after_check(true), Step::Fire);
    }

    #[test]
    fn retries_with_growing_delay_then_fires_anyway() {
        let mut p = ResumePlan::new();
        let mut waits = Vec::new();
        while let Step::Wait(d) = p.after_check(false) {
            waits.push(d.as_secs());
        }
        assert_eq!(waits, vec![1, 2, 4, 8, 16]);
        assert_eq!(waits.len() as u32, MAX_CHECKS - 1);
    }
}
