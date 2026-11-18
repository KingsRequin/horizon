//! Mode nuit de l'écran du haut : plage horaire (qui peut passer minuit) et réglages.

use serde::{Deserialize, Serialize};

/// « 22:00 » → 1320 (minutes depuis minuit).
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Vrai si `now` (minutes depuis minuit) est dans la plage [start, end[ (qui peut passer minuit).
pub fn is_night(now: u32, start: u32, end: u32) -> bool {
    if start <= end {
        (start..end).contains(&now)
    } else {
        now >= start || now < end
    }
}

/// Section `[night]` de `settings.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NightSettings {
    pub start: String,
    pub end: String,
    /// Luminosité des barres non cliquables la nuit (1 = normale).
    pub opacity: f32,
    /// « Ne pas déranger » s'active de lui-même pendant la plage de nuit.
    pub dnd: bool,
}

impl Default for NightSettings {
    fn default() -> Self {
        NightSettings { start: "22:00".into(), end: "07:00".into(), opacity: 0.55, dnd: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hours_and_minutes() {
        assert_eq!(parse_hhmm("22:00"), Some(1320));
        assert_eq!(parse_hhmm("7:05"), Some(425));
        assert_eq!(parse_hhmm(" 00:00 "), Some(0));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("midi"), None);
    }

    #[test]
    fn night_range_crossing_midnight() {
        let (s, e) = (22 * 60, 7 * 60);
        assert!(is_night(23 * 60, s, e));
        assert!(is_night(0, s, e));
        assert!(is_night(6 * 60 + 59, s, e));
        assert!(!is_night(7 * 60, s, e));
        assert!(!is_night(12 * 60, s, e));
        assert!(is_night(22 * 60, s, e));
    }

    #[test]
    fn night_range_same_day() {
        let (s, e) = (13 * 60, 15 * 60);
        assert!(is_night(14 * 60, s, e));
        assert!(!is_night(16 * 60, s, e));
        assert!(!is_night(12 * 60, s, e));
        // Plage vide : jamais.
        assert!(!is_night(10, 600, 600));
    }
}
