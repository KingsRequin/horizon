//! Historique borné de mesures en pourcentage (courbes des widgets système).

use serde::Serialize;
use std::collections::VecDeque;

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct Series {
    #[serde(skip)]
    cap: usize,
    values: VecDeque<f32>,
}

impl Series {
    pub fn new(cap: usize) -> Series {
        Series { cap, values: VecDeque::with_capacity(cap) }
    }

    /// Ajoute une mesure, ramenée entre 0 et 100 (NaN → 0) et arrondie au dixième (une courbe
    /// de quelques pixels n'en montre pas plus, et l'historique part souvent aux fenêtres). La
    /// plus ancienne sort si plein.
    pub fn push(&mut self, v: f32) {
        if self.values.len() == self.cap {
            self.values.pop_front();
        }
        let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 100.0) };
        self.values.push_back((v * 10.0).round() / 10.0);
    }

    pub fn values(&self) -> Vec<f32> {
        self.values.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_last_values_up_to_capacity() {
        let mut s = Series::new(3);
        for v in [1.0, 2.0, 3.0, 4.0] {
            s.push(v);
        }
        assert_eq!(s.values(), vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn clamps_to_percent() {
        let mut s = Series::new(4);
        s.push(-5.0);
        s.push(150.0);
        s.push(f32::NAN);
        s.push(42.5);
        assert_eq!(s.values(), vec![0.0, 100.0, 0.0, 42.5]);
    }

    #[test]
    fn rounds_to_a_tenth() {
        let mut s = Series::new(2);
        s.push(12.345_678);
        s.push(7.06);
        assert_eq!(serde_json::to_string(&s).unwrap(), "[12.3,7.1]");
    }

    #[test]
    fn empty_series() {
        let s = Series::new(60);
        assert!(s.values().is_empty());
        assert_eq!(serde_json::to_string(&s).unwrap(), "[]");
    }
}
