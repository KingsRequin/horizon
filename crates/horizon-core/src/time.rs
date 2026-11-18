//! Heure : instants Unix (millisecondes ou secondes) et heure locale. Une seule lecture de
//! l'horloge pour tout Horizon ; la logique reçoit l'instant en paramètre (testable).

use chrono::TimeZone;
use std::time::{SystemTime, UNIX_EPOCH};

/// Instant courant en millisecondes Unix.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Instant courant en secondes Unix.
pub fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Heure locale « naïve » (celle de l'horloge, comptée comme si elle était UTC) d'un instant
/// UTC en secondes : sert aux statistiques par heure et par jour de la semaine.
pub fn local_of(utc_secs: i64) -> i64 {
    chrono::Local
        .timestamp_opt(utc_secs, 0)
        .single()
        .map_or(utc_secs, |t| t.naive_local().and_utc().timestamp())
}

/// Minuit local du jour de `now_ms`, en millisecondes Unix.
pub fn local_day_start_ms(now_ms: u64) -> u64 {
    let Some(t) = chrono::Local.timestamp_millis_opt(now_ms as i64).single() else { return now_ms };
    let midnight = t.date_naive().and_hms_opt(0, 0, 0).expect("minuit existe");
    chrono::Local
        .from_local_datetime(&midnight)
        .earliest()
        .map_or(now_ms - now_ms % 86_400_000, |m| m.timestamp_millis().max(0) as u64)
}

/// Minutes écoulées depuis minuit, heure locale.
pub fn local_minutes() -> u32 {
    minutes_of_day(chrono::Local::now().naive_local().time())
}

/// Date WinRT (`DateTime`, unités de 100 ns depuis 1601) → millisecondes Unix.
pub fn from_winrt(universal: i64) -> u64 {
    const EPOCH_DIFF_100NS: i64 = 116_444_736_000_000_000;
    ((universal - EPOCH_DIFF_100NS).max(0) / 10_000) as u64
}

fn minutes_of_day(t: chrono::NaiveTime) -> u32 {
    use chrono::Timelike;
    t.hour() * 60 + t.minute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_since_midnight() {
        let t = chrono::NaiveTime::from_hms_opt(22, 35, 59).unwrap();
        assert_eq!(minutes_of_day(t), 22 * 60 + 35);
        assert_eq!(minutes_of_day(chrono::NaiveTime::MIN), 0);
    }

    #[test]
    fn day_start_is_before_now_and_less_than_a_day_ago() {
        let now = now_ms();
        let start = local_day_start_ms(now);
        assert!(start <= now && now - start < 25 * 3_600_000);
        assert_eq!(local_day_start_ms(start + 1000), start);
    }

    #[test]
    fn winrt_epoch_converts_to_unix_ms() {
        assert_eq!(from_winrt(116_444_736_000_000_000), 0);
        assert_eq!(from_winrt(116_444_736_000_000_000 + 10_000 * 1500), 1500);
        assert_eq!(from_winrt(0), 0);
    }

    #[test]
    fn clock_reads_agree() {
        let (ms, s) = (now_ms(), now_secs());
        assert!((ms / 1000) as i64 - s <= 1);
        assert!(local_minutes() < 24 * 60);
    }
}
