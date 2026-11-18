//! Délai de reconnexion croissant (1 s, 2 s, 4 s… plafonné à 30 s) pour les services locaux
//! absents (Prisme, relais Krisp) : pas de boucle serrée, pas d'erreur répétée. Une connexion
//! qui tombe aussitôt établie (service qui ferme dès l'ouverture) ne remet pas le délai à zéro.

use std::time::Duration;

#[derive(Debug, Default)]
pub struct Backoff {
    attempts: u32,
}

impl Backoff {
    pub const MAX: Duration = Duration::from_secs(30);
    /// Durée d'une connexion au-delà de laquelle le service est jugé stable.
    pub(crate) const STABLE: Duration = Duration::from_secs(10);

    pub fn new() -> Backoff {
        Backoff::default()
    }

    /// Délai avant la prochaine tentative.
    pub fn next_delay(&mut self) -> Duration {
        let secs = 1u64.checked_shl(self.attempts.min(16)).unwrap_or(u64::MAX);
        self.attempts = self.attempts.saturating_add(1);
        Duration::from_secs(secs).min(Self::MAX)
    }

    /// Repart du plus court délai (sortie de veille : on retente tout de suite).
    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    /// Fin d'une connexion restée ouverte `lasted` : le délai ne repart de 1 s que si elle a
    /// tenu au moins `STABLE`, sinon il continue de croître.
    pub fn connection_closed(&mut self, lasted: Duration) {
        if lasted >= Self::STABLE {
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_client_backoff_caps_at_30s() {
        let mut b = Backoff::new();
        let delays: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30, 30]);
        // Beaucoup d'échecs : pas de dépassement.
        for _ in 0..100 {
            assert_eq!(b.next_delay(), Backoff::MAX);
        }
    }

    #[test]
    fn backoff_reset_after_stable_connection() {
        let mut b = Backoff::new();
        b.next_delay();
        b.next_delay();
        b.connection_closed(Backoff::STABLE);
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn connection_dropped_at_once_keeps_growing() {
        let mut b = Backoff::new();
        // Le service accepte puis ferme aussitôt, encore et encore.
        let delays: Vec<u64> = (0..6)
            .map(|_| {
                b.connection_closed(Duration::from_millis(50));
                b.next_delay().as_secs()
            })
            .collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30]);
        b.connection_closed(Backoff::STABLE - Duration::from_millis(1));
        assert_eq!(b.next_delay(), Backoff::MAX);
    }

    #[test]
    fn reset_restarts_from_shortest_delay() {
        let mut b = Backoff::new();
        b.next_delay();
        b.next_delay();
        b.reset();
        assert_eq!(b.next_delay(), Duration::from_secs(1));
    }
}
