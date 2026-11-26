//! Messages échangés avec l'appli Horizon Mobile (JSON, champ `type`). Référence :
//! `docs/PHONE.md` ; exemples : `tests/fixtures/phone/`, que l'appli teste aussi.
//!
//! Tolérance : un type inconnu donne [`FromPhone::Unknown`], un champ inconnu est ignoré. Une
//! version plus récente de l'appli peut donc parler à cette version d'Horizon.

use serde::{Deserialize, Serialize};

/// Version du protocole parlée par cette version d'Horizon.
pub const PROTOCOL: u32 = 1;
/// Fonctions annoncées au téléphone dans `hello.ok`.
pub const FEATURES: &[&str] = &["battery", "media", "mixer", "actions"];

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
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum FromPhone {
    #[serde(rename = "pair.request")]
    PairRequest { token: String, device_id: String, device_name: String, app_version: String },
    #[serde(rename = "hello")]
    Hello { device_id: String, device_token: String, protocol: u32, app_version: String },
    #[serde(rename = "phone.battery")]
    Battery { devices: Vec<DeviceBattery> },
    /// Ce que le téléphone veut recevoir (`media`, `volume`) ; remplace la liste précédente.
    #[serde(rename = "watch")]
    Watch { topics: Vec<String> },
    /// `play_pause`, `next` ou `previous` (trié par `remote::command`).
    #[serde(rename = "media.command")]
    MediaCommand { action: String },
    /// Aller à cette position du morceau en cours.
    #[serde(rename = "media.seek")]
    MediaSeek { position_ms: u64 },
    /// Suivre cette lecture plutôt que celle choisie par Windows.
    #[serde(rename = "media.select")]
    MediaSelect { app_id: String },
    /// Volume général, de 0 à 1.
    #[serde(rename = "volume.set")]
    VolumeSet { volume: f32 },
    #[serde(rename = "volume.mute")]
    VolumeMute { muted: bool },
    /// Volume d'une appli (clé d'exe du mixeur), de 0 à 1.
    #[serde(rename = "mixer.app_volume")]
    MixerAppVolume { key: String, volume: f32 },
    #[serde(rename = "mixer.app_mute")]
    MixerAppMute { key: String, muted: bool },
    /// Sortie audio par défaut (identifiant d'endpoint du mixeur).
    #[serde(rename = "mixer.output")]
    MixerOutput { id: String },
    /// Verrouiller la session Windows.
    #[serde(rename = "action.lock")]
    ActionLock,
    /// Ne pas déranger, allumé ou éteint.
    #[serde(rename = "action.dnd")]
    ActionDnd { on: bool },
    /// Lancer le minuteur pour `minutes`.
    #[serde(rename = "timer.start")]
    TimerStart { minutes: u32 },
    #[serde(rename = "timer.pause")]
    TimerPause { paused: bool },
    #[serde(rename = "timer.stop")]
    TimerStop,
    #[serde(other)]
    Unknown,
}

/// Message envoyé au téléphone.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum ToPhone {
    #[serde(rename = "pair.accepted")]
    PairAccepted { device_token: String, pc_id: String, pc_name: String },
    #[serde(rename = "pair.rejected")]
    PairRejected { reason: String },
    /// `features` : ce que ce PC sait faire ; `granted` : ce que ce téléphone a le droit de faire.
    #[serde(rename = "hello.ok")]
    HelloOk { pc_id: String, pc_name: String, protocol: u32, features: Vec<String>, granted: Vec<String>, accent: String },
    #[serde(rename = "hello.rejected")]
    HelloRejected { reason: String },
    #[serde(rename = "accent")]
    Accent { color: String },
    /// Les droits de ce téléphone ont changé pendant la session.
    #[serde(rename = "rights")]
    Rights { granted: Vec<String> },
    /// Lecture en cours (`None` : rien ne joue) et lectures entre lesquelles choisir.
    #[serde(rename = "media.state")]
    /// (Dans une boîte : le message reste petit, un refus de connexion aussi.)
    MediaState { media: Option<Box<MediaView>>, sessions: Vec<SessionView> },
    /// Pochette du morceau `art_key` : JPEG en base64.
    #[serde(rename = "media.art")]
    MediaArt { art_key: String, jpeg: String },
    /// Volume général (sortie par défaut), de 0 à 1.
    #[serde(rename = "volume.state")]
    VolumeState { volume: f32, muted: bool },
    /// Sorties audio, sortie par défaut et volume de chaque appli (ordre d'Horizon : celles qui
    /// émettent d'abord).
    #[serde(rename = "mixer.state")]
    MixerState { outputs: Vec<OutputView>, default_output: Option<String>, apps: Vec<AppView> },
    /// Ne pas déranger, minuteur (`None` : arrêté), durées proposées et casque du PC.
    #[serde(rename = "actions.state")]
    ActionsState { dnd: bool, timer: Option<TimerState>, timer_presets: Vec<u32>, headset: Option<HeadsetView> },
}

/// Une sortie audio du PC ; `kind` : `headset` ou `speaker` (l'icône du téléphone).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputView {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// Une appli du mixeur : `key` est la clé d'exe que les commandes reprennent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    pub key: String,
    pub name: String,
    pub volume: f32,
    pub muted: bool,
    /// Elle émet du son en ce moment.
    pub active: bool,
}

/// Minuteur en cours : `remaining_ms` vaut au moment de l'envoi, le téléphone le décompte
/// depuis la réception (sauf en pause).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerState {
    pub remaining_ms: u64,
    pub total_ms: u64,
    pub paused: bool,
}

/// Casque du PC (batterie lue par Horizon).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadsetView {
    pub name: String,
    pub level: u8,
    pub charging: bool,
}

/// Lecture en cours telle que le téléphone la reçoit. `position_ms` vaut au moment de l'envoi :
/// le téléphone la fait avancer depuis la réception (les deux horloges ne sont pas les mêmes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaView {
    pub app_id: String,
    pub app_name: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub position_ms: u64,
    /// 0 : pas de durée (radio, direct).
    pub duration_ms: u64,
    /// Clé de la pochette de ce morceau (`media.art`) ; `None` : pas de pochette.
    pub art_key: Option<String>,
}

/// Une lecture que le téléphone peut choisir de suivre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub app_id: String,
    pub name: String,
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
        same(ToPhone::PairRejected { reason: "expired".into() }, "pair.rejected.json");
        same(ToPhone::HelloRejected { reason: "unknown_device".into() }, "hello.rejected.json");
        same(ToPhone::Accent { color: "#a06cd5".into() }, "accent.json");
        let token = "3qX8mZ1kLr5vYt0aBn7cWd2eFg4hJi6oPs9uQx_-AbC";
        same(ToPhone::PairAccepted { device_token: token.into(), pc_id: "pc-1".into(), pc_name: "Bureau".into() }, "pair.accepted.json");
    }

    #[test]
    fn every_new_message_from_the_phone_parses() {
        assert_eq!(parse(fixture("watch.json")), Some(FromPhone::Watch { topics: vec!["media".into(), "volume".into()] }));
        assert_eq!(parse(fixture("media.command.json")), Some(FromPhone::MediaCommand { action: "play_pause".into() }));
        assert_eq!(parse(fixture("media.seek.json")), Some(FromPhone::MediaSeek { position_ms: 68_000 }));
        assert_eq!(parse(fixture("media.select.json")), Some(FromPhone::MediaSelect { app_id: "Spotify.exe".into() }));
        assert_eq!(parse(fixture("volume.set.json")), Some(FromPhone::VolumeSet { volume: 0.72 }));
        assert_eq!(parse(fixture("volume.mute.json")), Some(FromPhone::VolumeMute { muted: true }));
    }

    #[test]
    fn the_mixer_and_action_messages_from_the_phone_parse() {
        assert_eq!(parse(fixture("mixer.app_volume.json")), Some(FromPhone::MixerAppVolume { key: "spotify.exe".into(), volume: 0.4 }));
        assert_eq!(parse(fixture("mixer.app_mute.json")), Some(FromPhone::MixerAppMute { key: "discord.exe".into(), muted: true }));
        assert_eq!(parse(fixture("mixer.output.json")), Some(FromPhone::MixerOutput { id: "{0.0.0.00000000}.{5a1c}".into() }));
        assert_eq!(parse(fixture("action.lock.json")), Some(FromPhone::ActionLock));
        assert_eq!(parse(fixture("action.dnd.json")), Some(FromPhone::ActionDnd { on: true }));
        assert_eq!(parse(fixture("timer.start.json")), Some(FromPhone::TimerStart { minutes: 25 }));
        assert_eq!(parse(fixture("timer.pause.json")), Some(FromPhone::TimerPause { paused: true }));
        assert_eq!(parse(fixture("timer.stop.json")), Some(FromPhone::TimerStop));
    }

    #[test]
    fn the_mixer_and_action_states_match_their_fixtures() {
        let same = |msg: ToPhone, name: &str| {
            let sent: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
            let expected: serde_json::Value = serde_json::from_str(fixture(name)).unwrap();
            assert_eq!(sent, expected, "{name}");
        };
        let outputs = vec![
            OutputView { id: "out-1".into(), name: "Casque".into(), kind: "headset".into() },
            OutputView { id: "out-2".into(), name: "Enceintes".into(), kind: "speaker".into() },
        ];
        let apps = vec![
            AppView { key: "spotify.exe".into(), name: "Spotify".into(), volume: 0.8, muted: false, active: true },
            AppView { key: "discord.exe".into(), name: "Discord".into(), volume: 0.55, muted: true, active: false },
        ];
        same(ToPhone::MixerState { outputs, default_output: Some("out-1".into()), apps }, "mixer.state.json");
        let timer = TimerState { remaining_ms: 760_000, total_ms: 1_500_000, paused: false };
        let headset = HeadsetView { name: "Casque".into(), level: 92, charging: false };
        same(ToPhone::ActionsState { dnd: true, timer: Some(timer), timer_presets: vec![5, 15, 25, 45], headset: Some(headset) }, "actions.state.json");
        same(ToPhone::ActionsState { dnd: false, timer: None, timer_presets: Vec::new(), headset: None }, "actions.state-vide.json");
    }

    #[test]
    fn a_media_command_with_an_unknown_action_is_kept_as_text() {
        // Le tri des actions est fait par `remote::command` : ici, rien ne se perd.
        assert_eq!(parse(r#"{"type":"media.command","action":"shuffle"}"#), Some(FromPhone::MediaCommand { action: "shuffle".into() }));
    }

    #[test]
    fn every_new_message_to_the_phone_matches_its_fixture() {
        let same = |msg: ToPhone, name: &str| {
            let sent: serde_json::Value = serde_json::from_str(&msg.to_json()).unwrap();
            let expected: serde_json::Value = serde_json::from_str(fixture(name)).unwrap();
            assert_eq!(sent, expected, "{name}");
        };
        let rights = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let ok = ToPhone::HelloOk {
            pc_id: "pc-1".into(),
            pc_name: "Bureau".into(),
            protocol: 1,
            features: rights(&["battery", "media"]),
            granted: rights(&["battery", "media", "sound"]),
            accent: "#f97316".into(),
        };
        same(ok, "hello.ok.json");
        same(ToPhone::Rights { granted: rights(&["battery", "sound"]) }, "rights.json");
        let media = MediaView {
            app_id: "Spotify.exe".into(),
            app_name: "Spotify".into(),
            title: "FROGZ".into(),
            artist: "SPLATA".into(),
            playing: true,
            position_ms: 68_000,
            duration_ms: 233_000,
            art_key: Some("9f2c1e0a7b3d4c55".into()),
        };
        let sessions = vec![SessionView { app_id: "Spotify.exe".into(), name: "Spotify".into() }, SessionView { app_id: "Chrome".into(), name: "Google Chrome".into() }];
        same(ToPhone::MediaState { media: Some(Box::new(media)), sessions }, "media.state.json");
        same(ToPhone::MediaState { media: None, sessions: Vec::new() }, "media.state-vide.json");
        same(ToPhone::MediaArt { art_key: "9f2c1e0a7b3d4c55".into(), jpeg: "/9j/4AAQSkZJRgABAQAAAQABAAD/2w==".into() }, "media.art.json");
        same(ToPhone::VolumeState { volume: 0.5, muted: false }, "volume.state.json");
    }

    #[test]
    fn the_hello_of_the_first_version_had_no_rights() {
        // Gardé pour l'appli : un PC 0.8.0 n'envoie pas `granted`.
        let old: serde_json::Value = serde_json::from_str(fixture("hello.ok-sans-droits.json")).unwrap();
        assert!(old.get("granted").is_none() && old["features"] == serde_json::json!(["battery"]));
    }
}
