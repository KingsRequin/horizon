//! Budget de performance d'un plugin : temps de script et de rendu mesurés, moyenne glissante
//! sur une minute ; un plugin trop gourmand ou qui plante sans cesse est désactivé par l'hôte.

use std::collections::VecDeque;

/// Fenêtre de la moyenne (ms).
pub const WINDOW_MS: u64 = 60_000;
/// Part d'un cœur au-delà de laquelle le plugin est trop gourmand (moyenne sur la fenêtre).
pub const MAX_LOAD: f32 = 0.02;
/// Plantages de script tolérés dans la fenêtre (le dernier fait désactiver).
pub const MAX_CRASHES: usize = 3;

/// Verdict du budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    TooHeavy,
    Crashing,
}

/// Mesures d'un plugin.
#[derive(Debug, Clone, Default)]
pub struct Budget {
    /// (instant en ms, coût en µs).
    samples: VecDeque<(u64, u64)>,
    crashes: VecDeque<u64>,
    /// Instant de la première mesure depuis le dernier `reset` (verdict seulement après une
    /// fenêtre complète).
    since: Option<u64>,
}

impl Budget {
    /// Coût d'un travail (rappel de script, rendu) terminé à `now_ms`.
    pub fn record(&mut self, now_ms: u64, cost_us: u64) {
        self.since.get_or_insert(now_ms);
        self.samples.push_back((now_ms, cost_us));
        purge(&mut self.samples, now_ms, |s| s.0);
    }

    /// Plantage du script à `now_ms`.
    pub fn crash(&mut self, now_ms: u64) {
        self.crashes.push_back(now_ms);
        purge(&mut self.crashes, now_ms, |t| *t);
    }

    /// Part d'un cœur utilisée sur la dernière fenêtre.
    pub fn load(&self, now_ms: u64) -> f32 {
        let start = now_ms.saturating_sub(WINDOW_MS);
        let us: u64 = self.samples.iter().filter(|(t, _)| *t > start && *t <= now_ms).map(|(_, c)| c).sum();
        us as f32 / (WINDOW_MS * 1000) as f32
    }

    /// Verdict à `now_ms`.
    pub fn verdict(&self, now_ms: u64) -> Verdict {
        let start = now_ms.saturating_sub(WINDOW_MS);
        if self.crashes.iter().filter(|t| **t > start).count() >= MAX_CRASHES {
            return Verdict::Crashing;
        }
        let full = self.since.is_some_and(|s| now_ms.saturating_sub(s) >= WINDOW_MS);
        if full && self.load(now_ms) > MAX_LOAD { Verdict::TooHeavy } else { Verdict::Ok }
    }

    /// Repart de zéro (plugin réactivé ou rechargé).
    pub fn reset(&mut self) {
        *self = Budget::default();
    }
}

/// Retire de la tête de `q` ce qui est sorti de la fenêtre.
fn purge<T>(q: &mut VecDeque<T>, now_ms: u64, at: impl Fn(&T) -> u64) {
    let start = now_ms.saturating_sub(WINDOW_MS);
    while q.front().is_some_and(|x| at(x) <= start) {
        q.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cost_us` chaque seconde de 1 à `secs`.
    fn every_second(b: &mut Budget, secs: u64, cost_us: u64) {
        for s in 1..=secs {
            b.record(s * 1000, cost_us);
        }
    }

    #[test]
    fn light_plugin_is_ok() {
        let mut b = Budget::default();
        every_second(&mut b, 61, 1_000);
        assert_eq!(b.verdict(61_000), Verdict::Ok);
        assert!((b.load(61_000) - 0.001).abs() < 0.0002, "{}", b.load(61_000));
    }

    #[test]
    fn heavy_plugin_after_a_full_window() {
        let mut b = Budget::default();
        every_second(&mut b, 59, 30_000);
        assert_eq!(b.verdict(59_000), Verdict::Ok);
        b.record(60_000, 30_000);
        b.record(61_000, 30_000);
        assert_eq!(b.verdict(61_000), Verdict::TooHeavy);
    }

    #[test]
    fn old_samples_expire() {
        let mut b = Budget::default();
        b.record(1_000, 5_000_000);
        assert!(b.load(30_000) > 0.05);
        assert_eq!(b.load(62_000), 0.0);
    }

    #[test]
    fn three_crashes_in_a_minute() {
        let mut b = Budget::default();
        b.crash(1_000);
        b.crash(2_000);
        assert_eq!(b.verdict(2_000), Verdict::Ok);
        b.crash(3_000);
        assert_eq!(b.verdict(3_000), Verdict::Crashing);
        let mut spaced = Budget::default();
        for i in 0..3 {
            spaced.crash(1_000 + i * 31_000);
        }
        assert_eq!(spaced.verdict(63_000), Verdict::Ok);
    }

    #[test]
    fn reset_clears() {
        let mut b = Budget::default();
        every_second(&mut b, 61, 30_000);
        b.crash(61_000);
        b.reset();
        assert_eq!(b.load(61_000), 0.0);
        assert_eq!(b.verdict(61_000), Verdict::Ok);
    }
}
