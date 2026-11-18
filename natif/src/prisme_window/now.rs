//! Page « Lumière » (`pages/Now.svelte`) : scène en cours et pourquoi, mode (automatique ou une
//! scène forcée), luminosité, appareils pilotés. Global `PrismeNow`.

use super::effects::{order_scenes, scene_label};
use super::types::{DaemonState, DeviceStatus};
use super::{Ctx, act, call, list, with};
use crate::ui::{PrismeChip, PrismeMode, PrismeNow, PrismeWindow};
use serde_json::json;
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// Pause avant l'envoi de la luminosité pendant le glissement.
const BRIGHTNESS_DELAY: Duration = Duration::from_millis(120);

#[derive(Default)]
pub(super) struct State {
    /// Valeur locale du curseur (fluide pendant le glissement).
    brightness: f32,
    dragging: bool,
    error: String,
}

thread_local! {
    static SEND: slint::Timer = slint::Timer::default();
}

pub(super) fn bind(win: &PrismeWindow) {
    let g = win.global::<PrismeNow>();
    g.on_choose(|key| {
        let scene = (!key.is_empty()).then(|| key.to_string());
        choose(scene);
    });
    g.on_brightness_moved(|v| {
        act(move |c| {
            c.now.brightness = v;
            c.now.dragging = true;
            super::refresh(c);
        });
        SEND.with(|t| t.start(slint::TimerMode::SingleShot, BRIGHTNESS_DELAY, move || send_brightness(v)));
    });
}

fn send_brightness(v: f32) {
    call("set_brightness", json!({ "value": v }), move |_| {
        with(|c| c.now.dragging = false);
    });
}

/// Force une scène (`None` : retour aux règles automatiques).
pub(super) fn choose(scene: Option<String>) {
    call("set_override", json!({ "scene": scene }), |r| {
        with(|c| {
            c.now.error = r.err().unwrap_or_default();
            super::refresh(c);
        });
    });
}

/// Nouvel état du moteur : la luminosité suit, sauf pendant un glissement.
pub(super) fn on_state(c: &mut Ctx) {
    if let Some(st) = &c.state
        && !c.now.dragging
    {
        c.now.brightness = st.brightness;
    }
}

/// `\\.\DISPLAY2` → « 2 ».
fn screen_number(m: Option<&str>) -> Option<String> {
    let m = m?.to_ascii_uppercase();
    let i = m.find("DISPLAY")?;
    let n: String = m[i + 7..].chars().take_while(char::is_ascii_digit).collect();
    (!n.is_empty()).then_some(n)
}

/// Pourquoi cette scène.
pub(super) fn reason(st: &DaemonState) -> String {
    if st.override_scene.is_some() {
        return "Choisie à la main. Repasse en automatique pour suivre ce qui joue.".into();
    }
    if let Some(v) = &st.video {
        let on = screen_number(st.monitor.as_deref()).map_or_else(String::new, |n| format!(" sur l'écran {n}"));
        return format!("Vidéo détectée dans {v}{on}.");
    }
    if let Some(m) = &st.music {
        return format!("Musique détectée dans {m}.");
    }
    if st.reason == "par défaut" {
        return "Rien ne joue : animation par défaut.".into();
    }
    format!("Règle « {} ».", st.reason)
}

fn chips(devices: &[DeviceStatus]) -> (Vec<PrismeChip>, String) {
    let chips = devices
        .iter()
        .filter(|d| d.enabled)
        .map(|d| PrismeChip {
            name: if d.label.is_empty() { d.name.as_str() } else { d.label.as_str() }.into(),
            tip: d.error.clone().unwrap_or_else(|| format!("{} LED · {:.0} img/s", d.leds, d.fps)).into(),
            state: if d.connected {
                1
            } else if d.error.is_some() {
                2
            } else {
                0
            },
        })
        .collect::<Vec<_>>();
    let connected = devices.iter().filter(|d| d.connected).count();
    let summary = if chips.is_empty() { String::new() } else { format!("{connected} sur {} connectés", chips.len()) };
    (chips, summary)
}

pub(super) fn refresh(c: &mut Ctx) {
    let g = c.win.global::<PrismeNow>();
    let st = c.state.as_ref();
    g.set_title(st.map_or_else(|| "…".into(), |s| scene_label(&s.scene)).into());
    g.set_reason(st.map(reason).unwrap_or_default().into());
    g.set_error(c.now.error.as_str().into());
    let forced = st.and_then(|s| s.override_scene.as_deref());
    let mut modes = vec![PrismeMode { key: "".into(), label: "Auto".into(), on: forced.is_none() }];
    modes.extend(order_scenes(st.map_or(&[][..], |s| &s.scenes)).into_iter().map(|s| PrismeMode {
        label: scene_label(&s).into(),
        on: forced == Some(s.as_str()),
        key: s.into(),
    }));
    if g.get_modes().iter().ne(modes.iter().cloned()) {
        g.set_modes(list(modes));
    }
    g.set_brightness(c.now.brightness);
    let (chips, summary) = chips(&c.devices);
    if g.get_devices().iter().ne(chips.iter().cloned()) {
        g.set_devices(list(chips));
    }
    g.set_summary(summary.into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_follow_what_plays() {
        let mut st = DaemonState { reason: "par défaut".into(), ..DaemonState::default() };
        assert_eq!(reason(&st), "Rien ne joue : animation par défaut.");
        st.music = Some("spotify.exe".into());
        assert_eq!(reason(&st), "Musique détectée dans spotify.exe.");
        st.video = Some("vlc.exe".into());
        st.monitor = Some(r"\\.\DISPLAY2".into());
        assert_eq!(reason(&st), "Vidéo détectée dans vlc.exe sur l'écran 2.");
        st.override_scene = Some("off".into());
        assert!(reason(&st).starts_with("Choisie à la main"));
        let st = DaemonState { reason: "jeux".into(), ..DaemonState::default() };
        assert_eq!(reason(&st), "Règle « jeux ».");
    }

    #[test]
    fn chips_show_enabled_devices() {
        let d = vec![
            DeviceStatus { id: "a".into(), name: "Bande".into(), enabled: true, connected: true, leds: 60, fps: 59.6, ..DeviceStatus::default() },
            DeviceStatus { id: "b".into(), label: "Lampe".into(), name: "Yeelight".into(), enabled: true, error: Some("absente".into()), ..DeviceStatus::default() },
            DeviceStatus { id: "c".into(), name: "Souris".into(), enabled: false, ..DeviceStatus::default() },
        ];
        let (chips, summary) = chips(&d);
        assert_eq!(chips.len(), 2);
        assert_eq!((chips[0].tip.as_str(), chips[0].state), ("60 LED · 60 img/s", 1));
        assert_eq!((chips[1].name.as_str(), chips[1].tip.as_str(), chips[1].state), ("Lampe", "absente", 2));
        assert_eq!(summary, "1 sur 2 connectés");
    }
}
