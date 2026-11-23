//! Messages échangés avec l'appli Horizon Mobile (JSON, champ `type`). Référence :
//! `docs/PHONE.md` ; exemples : `tests/fixtures/phone/`, que l'appli teste aussi.
//!
//! Tolérance : un type inconnu donne [`FromPhone::Unknown`], un champ inconnu est ignoré. Une
//! version plus récente de l'appli peut donc parler à cette version d'Horizon.

use serde::{Deserialize, Serialize};

/// Version du protocole parlée par cette version d'Horizon.
pub const PROTOCOL: u32 = 1;
/// Fonctions annoncées au téléphone dans `hello.ok`.
pub const FEATURES: &[&str] = &["battery"];

/// Genre d'un appareil dont le téléphone envoie la batterie.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Phone,
    Watch,
    Earbuds,
    /// Tout autre genre, y compris ceux qu'une appli plus récente inventerait.
    #[serde(other)]
    Other,
}

/// Batterie d'un appareil, mesurée par le téléphone à `at_ms` (heure Unix du téléphone).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceBattery {
    pub id: String,
    pub kind: DeviceKind,
    pub name: String,
    pub percent: u8,
    pub charging: bool,
    pub at_ms: u64,
}

/// Message reçu du téléphone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum FromPhone {
    #[serde(rename = "pair.request")]
    PairRequest { token: String, device_id: String, device_name: String, app_version: String },
    #[serde(rename = "hello")]
    Hello { device_id: String, device_token: String, protocol: u32, app_version: String },
    #[serde(rename = "phone.battery")]
    Battery { devices: Vec<DeviceBattery> },
    #[serde(other)]
    Unknown,
}

/// Message envoyé au téléphone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum ToPhone {
    #[serde(rename = "pair.accepted")]
    PairAccepted { device_token: String, pc_id: String, pc_name: String },
    #[serde(rename = "pair.rejected")]
    PairRejected { reason: String },
    #[serde(rename = "hello.ok")]
    HelloOk { pc_id: String, pc_name: String, protocol: u32, features: Vec<String>, accent: String },
    #[serde(rename = "hello.rejected")]
    HelloRejected { reason: String },
    #[serde(rename = "accent")]
    Accent { color: String },
}

/// Message du téléphone ; `None` : JSON illisible, sans `type`, ou champs obligatoires absents.
pub fn parse(text: &str) -> Option<FromPhone> {
    serde_json::from_str(text).ok()
}

impl ToPhone {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("message sans clé non textuelle")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> &'static str {
        let path = format!("{}/tests/fixtures/phone/{name}", env!("CARGO_MANIFEST_DIR"));
        Box::leak(std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path} : {e}")).into_boxed_str())
    }

    #[test]
    fn every_fixture_from_the_phone_parses() {
        assert!(matches!(parse(fixture("pair.request.json")), Some(FromPhone::PairRequest { .. })));
        assert!(matches!(parse(fixture("hello.json")), Some(FromPhone::Hello { protocol: 1, .. })));
        let Some(FromPhone::Battery { devices }) = parse(fixture("phone.battery.json")) else { panic!() };
        assert_eq!((devices[0].kind, devices[0].percent, devices[0].charging), (DeviceKind::Phone, 82, true));
        assert_eq!(devices[1].kind, DeviceKind::Watch);
    }

    #[test]
    fn unknown_types_and_fields_are_tolerated() {
        assert_eq!(parse(fixture("inconnu.json")), Some(FromPhone::Unknown));
        assert!(matches!(parse(fixture("battery-champs-en-plus.json")), Some(FromPhone::Battery { .. })));
        assert_eq!(parse("pas du json"), None);
        assert_eq!(parse(r#"{"x":1}"#), None);
        let odd = r#"{"type":"phone.battery","devices":[{"id":"a","kind":"grille-pain","name":"x","percent":5,"charging":false,"atMs":1}]}"#;
        let kind = parse(odd).map(|m| match m {
            FromPhone::Battery { devices } => devices[0].kind,
            _ => panic!(),
        });
        assert_eq!(kind, Some(DeviceKind::Other));
    }

    #[test]
    fn messages_to_the_phone_match_their_fixture() {
        let same = |msg: ToPhone, name: &str| {
            let sent: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
            let expected: serde_json::Value = serde_json::from_str(fixture(name)).unwrap();
            assert_eq!(sent, expected, "{name}");
        };
        let ok = ToPhone::HelloOk { pc_id: "pc-1".into(), pc_name: "Bureau".into(), protocol: 1, features: vec!["battery".into()], accent: "#f97316".into() };
        same(ok, "hello.ok.json");
        same(ToPhone::PairRejected { reason: "expired".into() }, "pair.rejected.json");
        same(ToPhone::HelloRejected { reason: "unknown_device".into() }, "hello.rejected.json");
        same(ToPhone::Accent { color: "#a06cd5".into() }, "accent.json");
        let token = "3qX8mZ1kLr5vYt0aBn7cWd2eFg4hJi6oPs9uQx_-AbC";
        same(ToPhone::PairAccepted { device_token: token.into(), pc_id: "pc-1".into(), pc_name: "Bureau".into() }, "pair.accepted.json");
    }
}
