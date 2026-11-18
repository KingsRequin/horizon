//! Batteries du téléphone et de la montre, publiées par l'agent PC de SharkApp dans
//! `%LOCALAPPDATA%\KingsHub\batteries.json` :
//! `{"version":1,"phone":{"name","percent","charging","at"}|null,"watch":{…}|null}`,
//! `at` = instant de la mesure (ISO 8601 avec décalage).

use chrono::DateTime;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Battery {
    pub name: String,
    pub percent: u8,
    pub charging: bool,
    /// Instant de la mesure, en millisecondes Unix.
    pub at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Batteries {
    pub phone: Option<Battery>,
    pub watch: Option<Battery>,
}

#[derive(Deserialize)]
struct RawBattery {
    #[serde(default)]
    name: String,
    percent: f64,
    #[serde(default)]
    charging: bool,
    at: String,
}

#[derive(Deserialize)]
struct RawFile {
    #[serde(default)]
    phone: Option<RawBattery>,
    #[serde(default)]
    watch: Option<RawBattery>,
}

fn convert(r: Option<RawBattery>) -> Option<Battery> {
    let r = r?;
    let at = DateTime::parse_from_rfc3339(r.at.trim()).ok()?;
    Some(Battery {
        name: r.name.trim().to_owned(),
        percent: r.percent.clamp(0.0, 100.0).round() as u8,
        charging: r.charging,
        at_ms: at.timestamp_millis(),
    })
}

/// Lit le fichier ; une entrée illisible (date invalide…) est ignorée, pas tout le fichier.
pub fn parse(text: &str) -> anyhow::Result<Batteries> {
    let raw: RawFile = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
    Ok(Batteries { phone: convert(raw.phone), watch: convert(raw.watch) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_contract_file() {
        let b = parse(
            r#"{"version":1,
              "phone":{"name":"Xiaomi 15","percent":82,"charging":false,"at":"2026-11-11T10:00:00+01:00"},
              "watch":{"name":"Galaxy Watch","percent":64.4,"charging":true,"at":"2026-11-11T09:58:12+01:00"}}"#,
        )
        .unwrap();
        let p = b.phone.unwrap();
        assert_eq!((p.name.as_str(), p.percent, p.charging), ("Xiaomi 15", 82, false));
        assert_eq!(p.at_ms, 1_794_387_600_000);
        let w = b.watch.unwrap();
        assert_eq!((w.percent, w.charging), (64, true));
    }

    #[test]
    fn missing_null_or_bad_entries() {
        let b = parse("\u{feff}{\"version\":1,\"phone\":null}").unwrap();
        assert_eq!(b, Batteries::default());
        let b = parse(r#"{"watch":{"percent":120,"at":"2026-11-11T10:00:00Z"},"phone":{"percent":5,"at":"hier"}}"#).unwrap();
        assert_eq!(b.watch.unwrap().percent, 100);
        assert!(b.phone.is_none());
        assert!(parse("pas du json").is_err());
    }
}
