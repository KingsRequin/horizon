//! Cache négatif : recherches coûteuses qui ont échoué récemment (résolution d'une appli dans le
//! menu Démarrer…). Une clé ratée n'est pas recherchée de nouveau avant `ttl` ; passé ce délai,
//! la recherche est retentée (l'appli a pu être installée entre-temps).

use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Misses<K> {
    ttl: Duration,
    at: HashMap<K, Instant>,
}

impl<K: Eq + Hash> Misses<K> {
    pub fn new(ttl: Duration) -> Misses<K> {
        Misses { ttl, at: HashMap::new() }
    }

    /// Vrai si `key` a échoué il y a moins de `ttl` : inutile de la rechercher.
    pub fn recent(&mut self, key: &K, now: Instant) -> bool {
        match self.at.get(key) {
            Some(t) if now.saturating_duration_since(*t) < self.ttl => true,
            Some(_) => {
                self.at.remove(key);
                false
            }
            None => false,
        }
    }

    /// Note l'échec de `key` à l'instant `now` (et oublie les échecs expirés).
    pub fn record(&mut self, key: K, now: Instant) {
        let ttl = self.ttl;
        self.at.retain(|_, t| now.saturating_duration_since(*t) < ttl);
        self.at.insert(key, now);
    }

    /// Nombre d'échecs gardés.
    pub fn len(&self) -> usize {
        self.at.len()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_miss_is_remembered_for_its_lifetime_only() {
        let t0 = Instant::now();
        let mut m = Misses::new(Duration::from_secs(60));
        assert!(!m.recent(&"a", t0));
        m.record("a", t0);
        assert!(m.recent(&"a", t0 + Duration::from_secs(59)));
        assert!(!m.recent(&"b", t0));
        assert!(!m.recent(&"a", t0 + Duration::from_secs(60)), "expiré : on retente");
        assert!(m.is_empty(), "l'échec expiré est oublié");
    }

    #[test]
    fn expired_misses_are_purged_on_record() {
        let t0 = Instant::now();
        let mut m = Misses::new(Duration::from_secs(10));
        m.record(1, t0);
        m.record(2, t0 + Duration::from_secs(5));
        m.record(3, t0 + Duration::from_secs(12));
        assert_eq!(m.len(), 2);
        assert!(m.recent(&2, t0 + Duration::from_secs(12)));
        assert!(!m.recent(&1, t0 + Duration::from_secs(12)));
    }
}
