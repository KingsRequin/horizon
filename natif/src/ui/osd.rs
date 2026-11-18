//! Affichage du volume et du média dans la bande de la barre principale (`ui/osd.slint`, global
//! `OsdData`), d'après `osd.rs` (`EV_OSD`) et la lecture en cours (`EV_MEDIA`, `EV_MEDIA_ART`) :
//! un seul à la fois, gardé 1,5 s (volume) ou 2,8 s (média) après le dernier changement, puis
//! effacé après son fondu de sortie (rien ne reste construit). Fil de l'interface.
//!
//! Repris de `app/src/lib/Osd.svelte`.

use super::{BarWindow, OsdData};
use crate::events::EV_OSD;
use crate::host::{self, AppHandle};
use crate::osd::{MediaAction, OsdEvent};
use crate::widgets::media::{Art, EV_MEDIA, EV_MEDIA_ART, MediaPayload, media_state};
use slint::{ComponentHandle, SharedPixelBuffer};
use std::cell::RefCell;
use std::time::Duration;

/// Durée d'affichage après le dernier évènement.
const SHOWN_VOLUME: Duration = Duration::from_millis(1500);
const SHOWN_MEDIA: Duration = Duration::from_millis(2800);
/// Effacement après le fondu de sortie (`--dur-base`, 180 ms).
const CLEAR_AFTER: Duration = Duration::from_millis(250);

thread_local! {
    /// Barre principale.
    static MAIN: RefCell<Option<slint::Weak<BarWindow>>> = RefCell::default();
    static HIDE: slint::Timer = slint::Timer::default();
    static CLEAR: slint::Timer = slint::Timer::default();
}

/// Nom de l'action (comme la charge JSON de la version Tauri).
fn action_name(a: MediaAction) -> &'static str {
    match a {
        MediaAction::PlayPause => "play_pause",
        MediaAction::Next => "next",
        MediaAction::Previous => "previous",
        MediaAction::Stop => "stop",
        MediaAction::Track => "track",
    }
}

/// Durée d'affichage d'un évènement.
fn shown_for(e: &OsdEvent) -> Duration {
    match e {
        OsdEvent::Volume { .. } => SHOWN_VOLUME,
        OsdEvent::Media { .. } => SHOWN_MEDIA,
    }
}

fn with_main(f: impl FnOnce(&BarWindow)) {
    if let Some(w) = MAIN.with_borrow(|m| m.as_ref().and_then(slint::Weak::upgrade)) {
        f(&w);
    }
}

/// Relie la barre `label` : seule la barre principale montre l'affichage.
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, main: bool) {
    if !main {
        return;
    }
    MAIN.set(Some(window.as_weak()));
    let full = media_state(app);
    on_media(window, &full.payload);
    on_art(window, full.art.as_ref());
    host::listen(EV_OSD, Some(label), |e| {
        if let Some(ev) = e.payload::<OsdEvent>() {
            show(ev);
        }
    });
    host::listen(EV_MEDIA, Some(label), |e| {
        if let Some(p) = e.payload::<MediaPayload>() {
            with_main(|w| on_media(w, p));
        }
    });
    host::listen(EV_MEDIA_ART, Some(label), |e| {
        if let Some(a) = e.payload::<Option<Art>>() {
            with_main(|w| on_art(w, a.as_ref()));
        }
    });
}

/// Un évènement : affiché (le précédent est remplacé), masqué après son délai.
pub fn show(e: &OsdEvent) {
    with_main(|w| {
        let g = w.global::<OsdData>();
        match e {
            OsdEvent::Volume { level, muted } => {
                g.set_kind("volume".into());
                g.set_level(*level as i32);
                g.set_muted(*muted);
            }
            OsdEvent::Media { action } => {
                g.set_kind("media".into());
                g.set_action(action_name(*action).into());
            }
        }
        g.set_shown(true);
    });
    CLEAR.with(slint::Timer::stop);
    HIDE.with(|t| {
        t.start(slint::TimerMode::SingleShot, shown_for(e), || {
            with_main(|w| w.global::<OsdData>().set_shown(false));
            CLEAR.with(|c| {
                c.start(slint::TimerMode::SingleShot, CLEAR_AFTER, || {
                    with_main(|w| {
                        let g = w.global::<OsdData>();
                        if !g.get_shown() {
                            g.set_kind("".into());
                        }
                    })
                })
            });
        })
    });
}

/// Lecture en cours (titre qui arrive après « suivant », lecture/pause).
fn on_media(w: &BarWindow, p: &MediaPayload) {
    let g = w.global::<OsdData>();
    let (title, artist, playing) = p.info.as_ref().map_or(("", "", false), |i| (i.title.as_str(), i.artist.as_str(), i.playing));
    g.set_title(title.into());
    g.set_artist(artist.into());
    g.set_playing(playing);
}

fn on_art(w: &BarWindow, art: Option<&Art>) {
    let g = w.global::<OsdData>();
    g.set_has_art(art.is_some());
    if let Some(a) = art {
        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&a.rgba, a.size, a.size);
        g.set_art(slint::Image::from_rgba8(buf));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_action_names_match_the_tauri_osd() {
        assert_eq!(shown_for(&OsdEvent::Volume { level: 3, muted: false }), Duration::from_millis(1500));
        assert_eq!(shown_for(&OsdEvent::Media { action: MediaAction::Track }), Duration::from_millis(2800));
        for a in [MediaAction::PlayPause, MediaAction::Next, MediaAction::Previous, MediaAction::Stop, MediaAction::Track] {
            let json = serde_json::to_value(a).unwrap();
            assert_eq!(json, serde_json::json!(action_name(a)));
        }
    }
}
