//! Télécommande : ce que le téléphone a le droit de regarder et de commander, commandes bornées,
//! état du média tel que le téléphone le reçoit. Le module `phone` d'Horizon relie ces décisions
//! aux fonctions de lecture et de mixeur.

use super::proto::{AppView, FromPhone, MediaView, OutputView, SessionView, TimerState, ToPhone};
use crate::media::{MediaInfo, position_at};

/// Droit qui ouvre la télécommande (commandes, état du média, volume général).
pub const MEDIA: &str = "media";
/// Droit « Son et actions » : mixeur par appli, sortie audio, actions rapides.
pub const SOUND: &str = "sound";
/// Applis du mixeur envoyées au plus.
const MAX_APPS: usize = 24;
/// Durée d'un minuteur, en minutes, au plus (10 h).
const MAX_TIMER_MINUTES: u32 = 600;
/// Longueur maximale d'un identifiant d'appli reçu du téléphone.
const MAX_APP_ID: usize = 512;

/// Ce qu'un téléphone peut regarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    Media,
    Volume,
    Mixer,
    Actions,
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
    AppVolume(String, f32),
    AppMute(String, bool),
    Output(String),
    Lock,
    Dnd(bool),
    TimerStart(u32),
    TimerPause(bool),
    TimerStop,
}

/// Un identifiant reçu du téléphone (clé d'appli, sortie) : ni vide ni démesuré.
fn sane(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_APP_ID
}

fn has(granted: &[String], right: &str) -> bool {
    granted.iter().any(|g| g == right)
}

/// Ce que le téléphone demande à regarder, sans les sujets inconnus ni ceux qu'il n'a pas le
/// droit de voir, sans doublon.
pub fn topics(requested: &[String], granted: &[String]) -> Vec<Topic> {
    let mut out = Vec::new();
    for t in requested {
        let (topic, right) = match t.as_str() {
            "media" => (Topic::Media, MEDIA),
            "volume" => (Topic::Volume, MEDIA),
            "mixer" => (Topic::Mixer, SOUND),
            "actions" => (Topic::Actions, SOUND),
            _ => continue,
        };
        if has(granted, right) && !out.contains(&topic) {
            out.push(topic);
        }
    }
    out
}

/// La commande que porte `msg`, si ce téléphone a le droit de la donner et qu'elle est valable.
pub fn command(msg: &FromPhone, granted: &[String]) -> Option<Command> {
    let right = match msg {
        FromPhone::MediaCommand { .. } | FromPhone::MediaSeek { .. } | FromPhone::MediaSelect { .. } | FromPhone::VolumeSet { .. } | FromPhone::VolumeMute { .. } => MEDIA,
        _ => SOUND,
    };
    if !has(granted, right) {
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
        FromPhone::MixerAppVolume { key, volume } if sane(key) && volume.is_finite() => Command::AppVolume(key.clone(), volume.clamp(0.0, 1.0)),
        FromPhone::MixerAppMute { key, muted } if sane(key) => Command::AppMute(key.clone(), *muted),
        FromPhone::MixerOutput { id } if sane(id) => Command::Output(id.clone()),
        FromPhone::ActionLock => Command::Lock,
        FromPhone::ActionDnd { on } => Command::Dnd(*on),
        FromPhone::TimerStart { minutes } if (1..=MAX_TIMER_MINUTES).contains(minutes) => Command::TimerStart(*minutes),
        FromPhone::TimerPause { paused } => Command::TimerPause(*paused),
        FromPhone::TimerStop => Command::TimerStop,
        _ => return None,
    })
}

/// Icône d'une sortie audio d'après son nom : `headset` (casque, écouteurs) ou `speaker`.
pub fn output_kind(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    let headset = ["casque", "headset", "headphone", "écouteur", "earbud", "earphone", "airpods"].iter().any(|w| lower.contains(w));
    if headset { "headset" } else { "speaker" }
}

/// État du mixeur pour le téléphone : sorties, sortie par défaut, applis (celles qui émettent
/// d'abord, puis l'ordre d'Horizon), [`MAX_APPS`] au plus.
pub fn mixer_message(s: &crate::mixer::MixerSnapshot) -> ToPhone {
    let outputs = s.outputs.iter().map(|d| OutputView { id: d.id.clone(), name: d.name.clone(), kind: output_kind(&d.name).to_owned() }).collect();
    let playing = s.apps.iter().filter(|a| a.active);
    let quiet = s.apps.iter().filter(|a| !a.active);
    let apps = playing
        .chain(quiet)
        .take(MAX_APPS)
        .map(|a| AppView { key: a.key.clone(), name: a.name.clone(), volume: a.level.volume, muted: a.level.muted, active: a.active })
        .collect();
    ToPhone::MixerState { outputs, default_output: s.default_output.clone(), apps }
}

/// Minuteur pour le téléphone : le temps restant à `now_ms` (figé en pause, jamais négatif).
pub fn timer_state(t: &crate::timer::TimerView, now_ms: u64) -> TimerState {
    let remaining_ms = match (t.ends_at_ms, t.remaining_ms) {
        (Some(end), _) => end.saturating_sub(now_ms),
        (None, Some(left)) => left,
        (None, None) => 0,
    };
    TimerState { remaining_ms, total_ms: t.total_ms, paused: t.ends_at_ms.is_none() }
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
    let media = info.map(|i| Box::new(MediaView {
        app_id: i.app_id.clone(),
        app_name: app_name.to_owned(),
        title: i.title.clone(),
        artist: i.artist.clone(),
        playing: i.playing,
        position_ms: position_at(i, now_ms),
        duration_ms: i.duration_ms,
        art_key: has_art.then(|| art_key(&i.app_id, &i.title, &i.artist)),
    }));
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
    fn mixer_and_actions_need_the_sound_right() {
        let media_only = rights(&["battery", "media"]);
        let msgs = [
            FromPhone::MixerAppVolume { key: "spotify.exe".into(), volume: 0.4 },
            FromPhone::MixerAppMute { key: "spotify.exe".into(), muted: true },
            FromPhone::MixerOutput { id: "out-1".into() },
            FromPhone::ActionLock,
            FromPhone::ActionDnd { on: true },
            FromPhone::TimerStart { minutes: 25 },
            FromPhone::TimerPause { paused: true },
            FromPhone::TimerStop,
        ];
        for msg in &msgs {
            assert_eq!(command(msg, &media_only), None, "{msg:?}");
        }
        let expected = [
            Command::AppVolume("spotify.exe".into(), 0.4),
            Command::AppMute("spotify.exe".into(), true),
            Command::Output("out-1".into()),
            Command::Lock,
            Command::Dnd(true),
            Command::TimerStart(25),
            Command::TimerPause(true),
            Command::TimerStop,
        ];
        for (msg, want) in msgs.iter().zip(expected) {
            assert_eq!(command(msg, &rights(&["battery", "sound"])), Some(want), "le droit sound suffit");
        }
        assert_eq!(topics(&rights(&["mixer", "actions", "media"]), &rights(&["sound"])), vec![Topic::Mixer, Topic::Actions]);
        assert_eq!(topics(&rights(&["mixer", "actions"]), &media_only), Vec::<Topic>::new());
    }

    #[test]
    fn timer_minutes_keys_and_volumes_are_bounded() {
        let all = all();
        assert_eq!(command(&FromPhone::TimerStart { minutes: 0 }, &all), None);
        assert_eq!(command(&FromPhone::TimerStart { minutes: 601 }, &all), None);
        assert_eq!(command(&FromPhone::TimerStart { minutes: 600 }, &all), Some(Command::TimerStart(600)));
        assert_eq!(command(&FromPhone::MixerAppVolume { key: String::new(), volume: 0.4 }, &all), None);
        assert_eq!(command(&FromPhone::MixerOutput { id: "x".repeat(513) }, &all), None);
        assert_eq!(command(&FromPhone::MixerAppVolume { key: "a.exe".into(), volume: f32::NAN }, &all), None);
        assert_eq!(command(&FromPhone::MixerAppVolume { key: "a.exe".into(), volume: 7.0 }, &all), Some(Command::AppVolume("a.exe".into(), 1.0)));
    }

    #[test]
    fn an_output_named_like_a_headset_is_one() {
        for name in ["Casque (Arctis Nova 7)", "Headset Earphone", "Écouteurs AirPods", "Realtek Headphones"] {
            assert_eq!(output_kind(name), "headset", "{name}");
        }
        for name in ["Haut-parleurs (Realtek)", "LG ULTRAGEAR (NVIDIA High Definition Audio)"] {
            assert_eq!(output_kind(name), "speaker", "{name}");
        }
    }

    #[test]
    fn the_mixer_message_puts_playing_apps_first_and_is_bounded() {
        use crate::mixer::{AppAudio, Device, Flow, Level, MixerSnapshot};
        let app = |key: &str, active: bool| AppAudio { key: key.into(), name: key.into(), exe: String::new(), level: Level { volume: 0.5, muted: false }, active, aumids: vec![] };
        let mut apps: Vec<AppAudio> = (0..30).map(|i| app(&format!("a{i}.exe"), false)).collect();
        apps.push(app("joue.exe", true));
        let s = MixerSnapshot {
            outputs: vec![Device { id: "o1".into(), name: "Casque".into(), flow: Flow::Output, pinned: false }],
            default_output: Some("o1".into()),
            apps,
            ..MixerSnapshot::default()
        };
        let ToPhone::MixerState { outputs, default_output, apps } = mixer_message(&s) else { panic!() };
        assert_eq!((outputs[0].kind.as_str(), default_output.as_deref()), ("headset", Some("o1")));
        assert_eq!(apps.len(), 24);
        assert_eq!(apps[0].key, "joue.exe");
        assert_eq!(apps[1].key, "a0.exe", "l'ordre d'Horizon est gardé pour les autres");
    }

    #[test]
    fn the_timer_is_sent_as_time_left() {
        use crate::timer::{Phase, TimerView};
        let running = TimerView { phase: Phase::Timer, total_ms: 1_500_000, ends_at_ms: Some(10_000 + 760_000), remaining_ms: None, cycle: 1 };
        let paused = TimerView { ends_at_ms: None, remaining_ms: Some(300_000), ..running.clone() };
        assert_eq!(timer_state(&running, 10_000), TimerState { remaining_ms: 760_000, total_ms: 1_500_000, paused: false });
        assert_eq!(timer_state(&paused, 10_000), TimerState { remaining_ms: 300_000, total_ms: 1_500_000, paused: true });
        assert_eq!(timer_state(&running, 5_000_000).remaining_ms, 0, "échu : jamais négatif");
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
