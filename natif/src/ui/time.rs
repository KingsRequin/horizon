//! Temps affiché (repris de `app/src/lib/time.ts`) : une seule règle pour les durées écoulées ou
//! restantes. Apostrophe typographique (’) partout. Brique commune des widgets (phase B).

const MIN: u64 = 60;
const HOUR: u64 = 3600;
const DAY: u64 = 86_400;

/// Temps écoulé depuis un instant : « à l’instant », « il y a 5 min », « il y a 3 h »,
/// « il y a 2 j ».
pub fn ago(at_ms: u64, now_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    if s < MIN {
        "à l’instant".to_owned()
    } else if s < HOUR {
        format!("il y a {} min", s / MIN)
    } else if s < DAY {
        format!("il y a {} h", s / HOUR)
    } else {
        format!("il y a {} j", s / DAY)
    }
}

/// Durée courte depuis un instant : « 12 s », « 4 min », « 1 h 05 », « 2 j ».
pub fn since(at_ms: u64, now_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    if s < MIN {
        format!("{s} s")
    } else if s < HOUR {
        format!("{} min", s / MIN)
    } else if s < DAY {
        format!("{} h {:02}", s / HOUR, (s % HOUR) / MIN)
    } else {
        format!("{} j", s / DAY)
    }
}

/// Temps restant jusqu'à un instant : « dans 45 min », « dans 3 h 05 », « dans 4 j 15 h ».
pub fn until(at_ms: u64, now_ms: u64) -> String {
    let s = at_ms.saturating_sub(now_ms) as f64 / 1000.0;
    let (m, h, d) = (MIN as f64, HOUR as f64, DAY as f64);
    if s < h {
        format!("dans {} min", ((s / m).ceil() as u64).max(1))
    } else if s < d {
        let s = s as u64;
        format!("dans {} h {:02}", s / HOUR, (s % HOUR) / MIN)
    } else {
        let s = s as u64;
        format!("dans {} j {} h", s / DAY, (s % DAY) / HOUR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_matches_time_ts() {
        let now = 10_000_000_000;
        assert_eq!(ago(now - 30_000, now), "à l’instant");
        assert_eq!(ago(now - 5 * 60_000, now), "il y a 5 min");
        assert_eq!(ago(now - 3 * 3_600_000 - 5, now), "il y a 3 h");
        assert_eq!(ago(now - 2 * 86_400_000, now), "il y a 2 j");
        assert_eq!(ago(now + 5000, now), "à l’instant", "instant à venir");
    }

    #[test]
    fn since_matches_time_ts() {
        let now = 10_000_000_000;
        assert_eq!(since(now - 12_000, now), "12 s");
        assert_eq!(since(now - 95_000, now), "1 min");
        assert_eq!(since(now - (3600 + 5 * 60) * 1000, now), "1 h 05");
        assert_eq!(since(now - 2 * 86_400_000 - 5, now), "2 j");
        assert_eq!(since(now + 5000, now), "0 s", "instant à venir : zéro");
    }

    #[test]
    fn until_matches_time_ts() {
        let now = 10_000_000_000;
        assert_eq!(until(now + 10_000, now), "dans 1 min");
        assert_eq!(until(now + 45 * 60_000, now), "dans 45 min");
        assert_eq!(until(now + (3 * 3600 + 5 * 60) * 1000, now), "dans 3 h 05");
        assert_eq!(until(now + (4 * 86_400 + 15 * 3600) * 1000, now), "dans 4 j 15 h");
    }
}
