//! Alerte de batterie faible (casque, téléphone, montre) : annoncée une seule fois quand le
//! niveau passe sous le seuil, réarmée quand il remonte nettement au-dessus ou en charge.

use crate::layout::{Layout, WidgetKind};
use serde_json::Value;

/// Seuil par défaut (pourcentage).
pub(crate) const DEFAULT_THRESHOLD: u8 = 15;
/// Remontée au-dessus du seuil qui réarme l'alerte (évite de la répéter quand le niveau
/// oscille autour du seuil).
pub(crate) const REARM_MARGIN: u8 = 5;
/// Réglage du widget (`settings.lowAlert`) : seuil en %, 0 = pas d'alerte.
pub(crate) const SETTING: &str = "lowAlert";
/// Mesure plus ancienne (téléphone, montre) : pas d'alerte, elle ne dit plus rien d'actuel.
pub(crate) const MAX_AGE_MS: i64 = 2 * 3_600_000;

/// Vrai si une mesure prise à `at_ms` peut encore donner lieu à une alerte.
pub fn recent(at_ms: i64, now_ms: i64) -> bool {
    now_ms - at_ms <= MAX_AGE_MS
}

/// Seuil réglé dans un widget : absent → `DEFAULT_THRESHOLD`, 0 → `None` (alerte coupée).
pub fn threshold(settings: &Value) -> Option<u8> {
    match settings.get(SETTING).and_then(Value::as_u64) {
        None => Some(DEFAULT_THRESHOLD),
        Some(0) => None,
        Some(v) => Some(v.min(100) as u8),
    }
}

/// Seuil du premier widget `kind` des barres actives ; `None` sans widget ou alerte coupée.
pub fn threshold_in(layout: &Layout, kind: WidgetKind) -> Option<u8> {
    threshold(&layout.first_of(kind)?.settings)
}

#[derive(Debug, Default)]
pub struct LowAlert {
    fired: bool,
}

impl LowAlert {
    /// Nouvelle mesure ; `true` = l'annoncer (une fois par passage sous le seuil).
    pub fn update(&mut self, level: u8, charging: bool, threshold: u8) -> bool {
        if charging || level >= threshold.saturating_add(REARM_MARGIN) {
            self.fired = false;
            return false;
        }
        if level <= threshold && !self.fired {
            self.fired = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::real_monitors;
    use serde_json::json;

    fn run(a: &mut LowAlert, levels: &[(u8, bool)], t: u8) -> Vec<u8> {
        levels.iter().filter(|(l, c)| a.update(*l, *c, t)).map(|(l, _)| *l).collect()
    }

    #[test]
    fn fires_once_when_crossing_down() {
        let mut a = LowAlert::default();
        let levels = [(40, false), (20, false), (16, false), (15, false), (14, false), (10, false), (5, false)];
        assert_eq!(run(&mut a, &levels, 15), vec![15]);
    }

    #[test]
    fn hysteresis_needs_threshold_plus_margin_to_rearm() {
        let mut a = LowAlert::default();
        // Oscillation autour du seuil : une seule alerte.
        let levels = [(15, false), (17, false), (14, false), (19, false), (13, false)];
        assert_eq!(run(&mut a, &levels, 15), vec![15]);
        // Remontée à seuil + 5 : réarmée.
        assert_eq!(run(&mut a, &[(20, false), (15, false)], 15), vec![15]);
    }

    #[test]
    fn charging_rearms_and_never_fires() {
        let mut a = LowAlert::default();
        assert_eq!(run(&mut a, &[(10, false)], 15), vec![10]);
        assert_eq!(run(&mut a, &[(11, true), (12, true)], 15), Vec::<u8>::new());
        // Débranché encore bas : nouvelle alerte.
        assert_eq!(run(&mut a, &[(12, false)], 15), vec![12]);
    }

    #[test]
    fn already_low_at_first_reading_fires() {
        let mut a = LowAlert::default();
        assert_eq!(run(&mut a, &[(8, false), (8, false)], 15), vec![8]);
    }

    #[test]
    fn old_measures_are_not_announced() {
        let now = 1_794_387_600_000;
        assert!(recent(now - 60_000, now));
        assert!(recent(now - MAX_AGE_MS, now));
        assert!(!recent(now - MAX_AGE_MS - 1, now));
    }

    #[test]
    fn threshold_from_widget_settings() {
        assert_eq!(threshold(&Value::Null), Some(DEFAULT_THRESHOLD));
        assert_eq!(threshold(&json!({ "lowAlert": 25 })), Some(25));
        assert_eq!(threshold(&json!({ "lowAlert": 0 })), None);
        assert_eq!(threshold(&json!({ "lowAlert": 500 })), Some(100));
        assert_eq!(threshold(&json!({ "lowAlert": "x" })), Some(DEFAULT_THRESHOLD));
    }

    #[test]
    fn threshold_needs_the_widget_in_the_layout() {
        let mut l = Layout::default_for(&real_monitors());
        assert_eq!(threshold_in(&l, WidgetKind::HEADSET), None, "pas de widget Casque sur une barre neuve");
        let id = l.add_widget("main", crate::layout::Zone::Right, WidgetKind::HEADSET).unwrap();
        assert_eq!(threshold_in(&l, WidgetKind::HEADSET), Some(DEFAULT_THRESHOLD));
        l.set_widget_settings(&id, json!({ "lowAlert": 30 })).unwrap();
        assert_eq!(threshold_in(&l, WidgetKind::HEADSET), Some(30));
        l.remove_widget(&id).unwrap();
        assert_eq!(threshold_in(&l, WidgetKind::HEADSET), None);
    }
}
