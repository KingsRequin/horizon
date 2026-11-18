//! Panneau Lecture en cours (global `MediaPanelData` de `ui/widgets/media/panel.slint`) : état et
//! pochette reçus du service, position qui avance pendant que le panneau est ouvert (toutes les
//! 500 ms, comme `every(500)` de la version Tauri), volume de l'appli qui joue (mixeur, suivi
//! seulement panneau ouvert), autres lectures en cours, commandes. Fil de l'interface.

use super::{Art, EV_MEDIA, EV_MEDIA_ART, MediaPayload, SEEK_MS};
use crate::events::EV_MIXER;
use crate::host::{self, AppHandle};
use crate::mixer::{self, MixerView};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{MediaPanelData, MediaPanelSession, format};
use crate::widgets::Panel;
use horizon_core::media::MediaInfo;
use slint::{ModelRc, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

/// Lecture vue par le panneau.
#[derive(Default)]
struct Now {
    info: Option<MediaInfo>,
    position_ms: u64,
    /// Réception de `position_ms`.
    received: Option<Instant>,
    /// Clé du mixeur de l'appli qui joue (curseur de volume).
    media_key: Option<String>,
}

impl Now {
    /// Position courante extrapolée (`mediaPosition` de media.svelte.ts).
    fn position(&self, at: Instant) -> u64 {
        let Some(info) = &self.info else { return 0 };
        let elapsed = match (info.playing, self.received) {
            (true, Some(r)) => at.saturating_duration_since(r).as_millis() as u64,
            _ => 0,
        };
        let pos = self.position_ms + elapsed;
        if info.duration_ms > 0 { pos.min(info.duration_ms) } else { pos }
    }
}

thread_local! {
    static NOW: RefCell<Now> = RefCell::default();
    static OTHERS: Rc<VecModel<MediaPanelSession>> = Rc::new(VecModel::default());
    /// Avance de la position, panneau ouvert et lecture en marche.
    static TICK: slint::Timer = slint::Timer::default();
    /// Le panneau est affiché.
    static OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn with_data(mut f: impl FnMut(MediaPanelData<'_>)) {
    panel_hosts::each("media", |h| f(h.global::<MediaPanelData>()));
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<MediaPanelData>();
    g.set_seek_s((SEEK_MS / 1000) as i32);
    g.set_others(ModelRc::from(OTHERS.with(Rc::clone)));

    let full = super::media_state(app);
    on_media(app, &full.payload);
    on_art(full.art.as_ref());
    on_mixer(&mixer::view(app));

    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        let a = app.clone();
        host::listen(EV_MEDIA, Some(LABEL), move |e| {
            if let Some(p) = e.payload::<MediaPayload>() {
                on_media(&a, p);
            }
        });
        host::listen(EV_MEDIA_ART, Some(LABEL), |e| {
            if let Some(art) = e.payload::<Option<Art>>() {
                on_art(art.as_ref());
            }
        });
        host::listen(EV_MIXER, Some(LABEL), |e| {
            if let Some(v) = e.payload::<MixerView>() {
                on_mixer(v);
            }
        });
    }

    let a = app.clone();
    g.on_seek(move |dir| report(super::media_seek_relative(&a, dir.signum() as i64 * SEEK_MS)));
    let a = app.clone();
    g.on_previous(move || report(super::media_prev(&a)));
    let a = app.clone();
    g.on_play_pause(move || report(super::media_play_pause(&a)));
    let a = app.clone();
    g.on_next(move || report(super::media_next(&a)));
    let a = app.clone();
    g.on_select(move |id| report(super::media_select_session(&a, id.to_string())));
    let a = app.clone();
    g.on_set_volume(move |v| {
        if let Some(key) = media_key() {
            report(mixer::set_app_volume(&a, key, v));
        }
    });
    let a = app.clone();
    g.on_set_mute(move |m| {
        if let Some(key) = media_key() {
            report(mixer::set_app_mute(&a, key, m));
        }
    });
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::warn!("panneau Lecture en cours : {e}");
    }
}

fn media_key() -> Option<String> {
    NOW.with_borrow(|n| n.media_key.clone())
}

/// Panneau ouvert : le mixeur est suivi, la position avance.
fn opened(app: &AppHandle, _kind: &str) {
    OPEN.set(true);
    mixer::watch(app, true);
    on_mixer(&mixer::view(app));
    show_position();
    sync_tick();
}

fn closed(app: &AppHandle, _kind: &str) {
    OPEN.set(false);
    mixer::watch(app, false);
    sync_tick();
}

fn on_media(app: &AppHandle, p: &MediaPayload) {
    let app_changed = NOW.with_borrow_mut(|n| {
        let changed = n.info.as_ref().map(|i| &i.app_id) != p.info.as_ref().map(|i| &i.app_id);
        n.info = p.info.clone();
        n.position_ms = p.position_ms;
        n.received = Some(Instant::now());
        changed
    });
    with_data(|g| {
        g.set_has_info(p.info.is_some());
        if let Some(i) = &p.info {
            g.set_app(format::app_name(&i.app_id).into());
            g.set_title(i.title.as_str().into());
            g.set_artist(i.artist.as_str().into());
            g.set_playing(i.playing);
            g.set_has_duration(i.duration_ms > 0);
            g.set_duration(format::clock(i.duration_ms).into());
        }
    });
    let current = p.info.as_ref().map(|i| i.app_id.as_str());
    let others: Vec<MediaPanelSession> = p
        .sessions
        .iter()
        .filter(|(id, _)| Some(id.as_str()) != current)
        .map(|(id, title)| MediaPanelSession {
            id: id.as_str().into(),
            app: format::app_name(id).into(),
            title: title.as_str().into(),
        })
        .collect();
    OTHERS.with(|m| crate::ui::model::sync(m, others));
    show_position();
    sync_tick();
    // L'appli qui joue a changé : le curseur de volume suit la nouvelle (mixeur relu).
    if app_changed {
        mixer::publish(app);
    }
}

fn on_art(art: Option<&Art>) {
    let image = art.map(|a| {
        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&a.panel, a.panel_size, a.panel_size);
        slint::Image::from_rgba8(buf)
    });
    with_data(|g| {
        g.set_has_art(image.is_some());
        if let Some(i) = &image {
            g.set_art(i.clone());
        }
    });
}

fn on_mixer(v: &MixerView) {
    let app = v.media_app();
    NOW.with_borrow_mut(|n| n.media_key = app.map(|a| a.key.clone()));
    with_data(|g| {
        g.set_has_volume(app.is_some());
        if let Some(a) = app {
            g.set_volume_label(a.name.as_str().into());
            g.set_volume(a.level.volume);
            g.set_muted(a.level.muted);
        }
    });
}

/// Position et part lue, à l'instant.
fn show_position() {
    let (pos, duration) = NOW.with_borrow(|n| (n.position(Instant::now()), n.info.as_ref().map_or(0, |i| i.duration_ms)));
    with_data(|g| {
        g.set_position(format::clock(pos).into());
        g.set_progress(if duration > 0 { (pos as f32 / duration as f32).min(1.0) } else { 0.0 });
    });
}

/// La position avance toutes les 500 ms, seulement panneau ouvert et lecture en marche.
fn sync_tick() {
    let run = OPEN.get() && NOW.with_borrow(|n| n.info.as_ref().is_some_and(|i| i.playing && i.duration_ms > 0));
    TICK.with(|t| {
        if !run {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_millis(500), show_position);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_advances_only_while_playing() {
        let t0 = Instant::now();
        let info = |playing| MediaInfo {
            app_id: "a".into(),
            title: "t".into(),
            artist: String::new(),
            playing,
            position_ms: 0,
            duration_ms: 60_000,
            timeline_at_ms: 0,
        };
        let n = Now { info: Some(info(true)), position_ms: 10_000, received: Some(t0), media_key: None };
        assert_eq!(n.position(t0 + Duration::from_millis(2_500)), 12_500);
        assert_eq!(n.position(t0 + Duration::from_secs(600)), 60_000, "bornée à la durée");
        let paused = Now { info: Some(info(false)), position_ms: 10_000, received: Some(t0), media_key: None };
        assert_eq!(paused.position(t0 + Duration::from_secs(5)), 10_000);
    }
}
