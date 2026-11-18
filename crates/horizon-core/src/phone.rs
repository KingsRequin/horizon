//! Appels du téléphone, publiés par l'agent PC de SharkApp dans
//! `%LOCALAPPDATA%\KingsHub\phone.json` :
//! `{"state":"ringing"|"active"|"ended"|"idle","name":string|null,"number":string|null,
//!   "since_ms":number,"updated_ms":number}`.
//! SharkApp repasse à `idle` 5 s après `ended`, et met déjà le média du PC en pause pendant un
//! appel : Horizon ne fait qu'afficher. Un fichier qui n'a pas bougé depuis 2 min compte comme
//! `idle` (agent arrêté en plein appel).

use serde::{Deserialize, Serialize};

/// Au-delà, le fichier est trop vieux : pas d'appel.
pub const STALE_MS: u64 = 2 * 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Ringing,
    Active,
    Ended,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PhoneFile {
    pub state: CallState,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub since_ms: u64,
    #[serde(default)]
    pub updated_ms: u64,
}

/// Appel montré dans l'îlot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhoneView {
    /// `ringing`, `active` ou `ended` (jamais `idle`).
    pub state: CallState,
    /// Nom, sinon numéro, sinon « Appel ».
    pub label: String,
    /// Début de l'état (sonnerie, décroché, fin), ms Unix.
    pub since_ms: u64,
}

pub fn parse(text: &str) -> anyhow::Result<PhoneFile> {
    Ok(serde_json::from_str(text.trim_start_matches('\u{feff}'))?)
}

/// Ce que l'îlot montre : rien au repos, ni si le fichier est trop vieux.
pub fn view(file: Option<&PhoneFile>, now_ms: u64) -> Option<PhoneView> {
    let f = file?;
    if f.state == CallState::Idle || now_ms.saturating_sub(f.updated_ms) > STALE_MS {
        return None;
    }
    let pick = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let label = pick(&f.name).or_else(|| pick(&f.number)).unwrap_or_else(|| "Appel".into());
    Some(PhoneView { state: f.state, label, since_ms: if f.since_ms > 0 { f.since_ms } else { f.updated_ms } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(json: &str) -> PhoneFile {
        parse(json).unwrap()
    }

    #[test]
    fn ringing_with_name_then_number_then_nothing() {
        let f = file(r#"{"state":"ringing","name":" Léo ","number":"0612","since_ms":1000,"updated_ms":1000}"#);
        assert_eq!(view(Some(&f), 2000), Some(PhoneView { state: CallState::Ringing, label: "Léo".into(), since_ms: 1000 }));
        let f = file(r#"{"state":"active","name":null,"number":"+33 6 12","since_ms":5000,"updated_ms":5000}"#);
        assert_eq!(view(Some(&f), 6000).unwrap().label, "+33 6 12");
        let f = file(r#"{"state":"active","name":"","number":null,"since_ms":5000,"updated_ms":5000}"#);
        assert_eq!(view(Some(&f), 6000).unwrap().label, "Appel");
    }

    #[test]
    fn idle_stale_or_absent_shows_nothing() {
        let idle = file(r#"{"state":"idle","name":null,"number":null,"since_ms":0,"updated_ms":1000}"#);
        assert_eq!(view(Some(&idle), 1000), None);
        let active = file(r#"{"state":"active","name":"Léo","number":null,"since_ms":0,"updated_ms":1000}"#);
        assert!(view(Some(&active), 1000 + STALE_MS).is_some());
        assert_eq!(view(Some(&active), 1001 + STALE_MS), None, "plus de 2 min : idle");
        assert_eq!(view(None, 0), None);
        // Sans since_ms : la date de mise à jour sert.
        assert_eq!(view(Some(&active), 2000).unwrap().since_ms, 1000);
    }

    #[test]
    fn ended_is_shown_and_bad_files_are_errors() {
        let ended = file("\u{feff}{\"state\":\"ended\",\"name\":\"Léo\",\"number\":null,\"since_ms\":9,\"updated_ms\":9}");
        assert_eq!(view(Some(&ended), 10).unwrap().state, CallState::Ended);
        assert!(parse(r#"{"state":"sonne"}"#).is_err());
        assert!(parse("").is_err());
    }
}
