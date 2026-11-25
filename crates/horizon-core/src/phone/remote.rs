//! Télécommande : ce que le téléphone a le droit de regarder et de commander, commandes bornées,
//! état du média tel que le téléphone le reçoit. Le module `phone` d'Horizon relie ces décisions
//! aux fonctions de lecture et de mixeur.

use super::proto::{FromPhone, MediaView, SessionView, ToPhone};
use crate::media::{MediaInfo, position_at};

/// Droit qui ouvre la télécommande (commandes, état du média, volume général).
pub const MEDIA: &str = "media";
/// Longueur maximale d'un identifiant d'appli reçu du téléphone.
const MAX_APP_ID: usize = 512;

/// Ce qu'un téléphone peut regarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    Media,
    Volume,
}

/// Une commande de la télécommande, déjà vérifiée et bornée.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
    SeekTo(u64),
    Select(String),
    SetVolume(f32),
    SetMute(bool),
}

fn has(granted: &[String], right: &str) -> bool {
    granted.iter().any(|g| g == right)
}

/// Ce que le téléphone demande à regarder, sans les sujets inconnus ni ceux qu'il n'a pas le
/// droit de voir, sans doublon.
pub fn topics(requested: &[String], granted: &[String]) -> Vec<Topic> {
    let mut out = Vec::new();
    for t in requested {
        let topic = match t.as_str() {
            "media" => Topic::Media,
            "volume" => Topic::Volume,
            _ => continue,
        };
        if has(granted, MEDIA) && !out.contains(&topic) {
            out.push(topic);
        }
    }
    out
}

/// La commande que porte `msg`, si ce téléphone a le droit de la donner et qu'elle est valable.
pub fn command(msg: &FromPhone, granted: &[String]) -> Option<Command> {
    if !has(granted, MEDIA) {
        return None;
    }
    Some(match msg {
        FromPhone::MediaCommand { action } => match action.as_str() {
            "play_pause" => Command::PlayPause,
            "next" => Command::Next,
            "previous" => Command::Previous,
            _ => return None,
        },
        FromPhone::MediaSeek { position_ms } => Command::SeekTo(*position_ms),
        FromPhone::MediaSelect { app_id } if !app_id.is_empty() && app_id.len() <= MAX_APP_ID => Command::Select(app_id.clone()),
        FromPhone::VolumeSet { volume } if volume.is_finite() => Command::SetVolume(volume.clamp(0.0, 1.0)),
        FromPhone::VolumeMute { muted } => Command::SetMute(*muted),
        _ => return None,
    })
}

/// Clé de la pochette d'un morceau : FNV-1a 64 bits des trois champs séparés, en hexadécimal.
pub fn art_key(app_id: &str, title: &str, artist: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for field in [app_id, title, artist] {
        // Un octet 0 entre les champs : « ab » + « c » ne donne pas « a » + « bc ».
        for b in field.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// État du média pour le téléphone, position extrapolée à `now_ms`. `has_art` : une pochette
/// est connue pour ce morceau.
pub fn media_message(info: Option<&MediaInfo>, app_name: &str, sessions: &[(String, String)], now_ms: u64, has_art: bool) -> ToPhone {
    let media = info.map(|i| MediaView {
        app_id: i.app_id.clone(),
        app_name: app_name.to_owned(),
        title: i.title.clone(),
        artist: i.artist.clone(),
        playing: i.playing,
        position_ms: position_at(i, now_ms),
        duration_ms: i.duration_ms,
        art_key: has_art.then(|| art_key(&i.app_id, &i.title, &i.artist)),
    });
    let sessions = sessions.iter().map(|(app_id, name)| SessionView { app_id: app_id.clone(), name: name.clone() }).collect();
    ToPhone::MediaState { media, sessions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaInfo;
    use crate::phone::proto::{FromPhone, ToPhone};

    fn rights(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn all() -> Vec<String> {
        rights(&["battery", "media", "sound"])
    }

    #[test]
    fn commands_need_the_media_right() {
        let without = rights(&["battery", "sound"]);
        for msg in [
            FromPhone::MediaCommand { action: "play_pause".into() },
            FromPhone::MediaSeek { position_ms: 5 },
            FromPhone::MediaSelect { app_id: "Spotify.exe".into() },
            FromPhone::VolumeSet { volume: 0.5 },
            FromPhone::VolumeMute { muted: true },
        ] {
            assert_eq!(command(&msg, &without), None, "{msg:?}");
            assert!(command(&msg, &all()).is_some(), "{msg:?}");
        }
        assert_eq!(command(&FromPhone::MediaCommand { action: "next".into() }, &all()), Some(Command::Next));
        assert_eq!(command(&FromPhone::MediaCommand { action: "previous".into() }, &all()), Some(Command::Previous));
        assert_eq!(command(&FromPhone::MediaSeek { position_ms: 68_000 }, &all()), Some(Command::SeekTo(68_000)));
        assert_eq!(command(&FromPhone::Watch { topics: vec![] }, &all()), None, "pas une commande");
    }

    #[test]
    fn the_volume_is_clamped_and_nan_is_refused() {
        assert_eq!(command(&FromPhone::VolumeSet { volume: 1.7 }, &all()), Some(Command::SetVolume(1.0)));
        assert_eq!(command(&FromPhone::VolumeSet { volume: -0.2 }, &all()), Some(Command::SetVolume(0.0)));
        assert_eq!(command(&FromPhone::VolumeSet { volume: f32::NAN }, &all()), None);
        assert_eq!(command(&FromPhone::VolumeSet { volume: f32::INFINITY }, &all()), None);
        assert_eq!(command(&FromPhone::VolumeMute { muted: false }, &all()), Some(Command::SetMute(false)));
    }

    #[test]
    fn unknown_actions_and_huge_app_ids_are_refused() {
        assert_eq!(command(&FromPhone::MediaCommand { action: "shuffle".into() }, &all()), None);
        assert_eq!(command(&FromPhone::MediaSelect { app_id: "x".repeat(513) }, &all()), None);
        assert_eq!(command(&FromPhone::MediaSelect { app_id: String::new() }, &all()), None);
        assert_eq!(command(&FromPhone::MediaSelect { app_id: "x".repeat(512) }, &all()), Some(Command::Select("x".repeat(512))));
    }

    #[test]
    fn topics_follow_the_rights() {
        assert_eq!(topics(&rights(&["media", "volume", "inconnu", "media"]), &all()), vec![Topic::Media, Topic::Volume]);
        assert!(topics(&rights(&["media", "volume"]), &rights(&["battery", "sound"])).is_empty());
    }

    fn playing(position_ms: u64, duration_ms: u64, at: u64) -> MediaInfo {
        MediaInfo { app_id: "Spotify.exe".into(), title: "FROGZ".into(), artist: "SPLATA".into(), playing: true, position_ms, duration_ms, timeline_at_ms: at }
    }

    #[test]
    fn the_media_message_extrapolates_the_position() {
        let sessions = vec![("Spotify.exe".to_owned(), "Spotify".to_owned())];
        let ToPhone::MediaState { media: Some(m), sessions: s } = media_message(Some(&playing(60_000, 233_000, 1_000)), "Spotify", &sessions, 9_000, true) else { panic!() };
        assert_eq!((m.position_ms, m.duration_ms, m.app_name.as_str()), (68_000, 233_000, "Spotify"));
        assert_eq!(m.art_key, Some(art_key("Spotify.exe", "FROGZ", "SPLATA")));
        assert_eq!((s[0].app_id.as_str(), s[0].name.as_str()), ("Spotify.exe", "Spotify"));
        let ToPhone::MediaState { media: Some(end), .. } = media_message(Some(&playing(230_000, 233_000, 0)), "Spotify", &[], 60_000, false) else { panic!() };
        assert_eq!((end.position_ms, end.art_key), (233_000, None), "bornée à la durée ; pas de pochette");
        let ToPhone::MediaState { media: Some(live), .. } = media_message(Some(&playing(5_000, 0, 0)), "Radio", &[], 10_000, false) else { panic!() };
        assert_eq!(live.position_ms, 15_000, "durée 0 : pas de borne");
        assert_eq!(media_message(None, "", &sessions, 0, false), ToPhone::MediaState { media: None, sessions: vec![crate::phone::proto::SessionView { app_id: "Spotify.exe".into(), name: "Spotify".into() }] });
    }

    #[test]
    fn the_art_key_is_stable() {
        let a = art_key("Spotify.exe", "FROGZ", "SPLATA");
        assert_eq!(a, art_key("Spotify.exe", "FROGZ", "SPLATA"));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, art_key("Spotify.exe", "FROGZ", "autre"));
        assert_ne!(art_key("ab", "c", ""), art_key("a", "bc", ""), "les champs ne se mélangent pas");
    }
}
