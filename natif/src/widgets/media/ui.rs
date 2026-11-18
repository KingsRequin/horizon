//! Lecture en cours dans les barres (global `MediaData` de `ui/widgets/media/widget.slint`) :
//! état et pochette reçus du service, position qui avance chaque seconde pendant la lecture,
//! abonnement au son (halo des îlots, spectre du style graphique), commandes et molette ±10 s
//! avec la bulle de la barre. Fil de l'interface.

use super::audio::{AudioFrame, AudioHub, BANDS, EV_AUDIO};
use super::{Art, EV_MEDIA, EV_MEDIA_ART, MediaPayload, SEEK_MS};
use crate::host::{self, AppHandle, Manager};
use crate::ui::{BarWindow, MediaData, format, hint};
use horizon_core::layout::{BarLayout, BarStyle};
use horizon_core::media::MediaInfo;
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Barre qui montre le widget.
struct Bar {
    label: String,
    window: slint::Weak<BarWindow>,
    graphic: bool,
    bands: Rc<VecModel<f32>>,
    /// Abonnée au son.
    subscribed: bool,
    /// Avance cumulée des crans de molette rapprochés (ms).
    seek_total: i64,
}

/// Lecture en cours vue par l'interface.
#[derive(Default)]
struct Now {
    info: Option<MediaInfo>,
    position_ms: u64,
    /// Réception de `position_ms`.
    received: Option<Instant>,
}

impl Now {
    /// Position courante extrapolée (avance pendant la lecture, sans dépasser la durée).
    fn position(&self, at: Instant) -> u64 {
        let Some(info) = &self.info else { return 0 };
        let elapsed = match (info.playing, self.received) {
            (true, Some(r)) => at.saturating_duration_since(r).as_millis() as u64,
            _ => 0,
        };
        let pos = self.position_ms + elapsed;
        if info.duration_ms > 0 { pos.min(info.duration_ms) } else { pos }
    }

    /// Part lue (0–1).
    fn progress(&self, at: Instant) -> f32 {
        match &self.info {
            Some(i) if i.duration_ms > 0 => (self.position(at) as f32 / i.duration_ms as f32).min(1.0),
            _ => 0.0,
        }
    }

    fn playing(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.playing)
    }
}

thread_local! {
    static BARS: RefCell<Vec<Bar>> = RefCell::default();
    static NOW: RefCell<Now> = RefCell::default();
    /// Avance de la progression, chaque seconde pendant la lecture.
    static TICK: slint::Timer = slint::Timer::default();
    /// Masque du halo (dessiné une fois) et dernière pochette convertie (partagée par les barres).
    static GLOW_MASK: slint::Image = glow_mask();
    static ART: RefCell<Option<(std::sync::Arc<Vec<u8>>, slint::Image)>> = RefCell::default();
}

/// Masque du halo : `radial-gradient(… , accent 55 %, transparent 72 %)` sur l'ellipse entière
/// (étirée par `image-fit: fill`), en blanc à recolorer (`colorize`).
fn glow_mask() -> slint::Image {
    const N: u32 = 128;
    let mut buf = SharedPixelBuffer::<slint::Rgba8Pixel>::new(N, N);
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = ((i as u32 % N) as f32, (i as u32 / N) as f32);
        let a = glow_alpha((x + 0.5) / N as f32 * 2.0 - 1.0, (y + 0.5) / N as f32 * 2.0 - 1.0);
        *px = slint::Rgba8Pixel { r: 255, g: 255, b: 255, a: (a * 255.0).round() as u8 };
    }
    slint::Image::from_rgba8(buf)
}

/// Opacité du halo au point (`dx`, `dy`) de l'ellipse (−1 à 1 sur chaque axe).
fn glow_alpha(dx: f32, dy: f32) -> f32 {
    let r = (dx * dx + dy * dy).sqrt();
    0.55 * (1.0 - r / 0.72).max(0.0)
}

/// Halo des îlots : suit le niveau, avec un éclat sur les battements ; au repos, faible.
fn glow(playing: bool, frame: Option<&AudioFrame>) -> f32 {
    let v = match frame {
        Some(f) if playing => (0.2 + f.level * 0.9 + if f.beat { 0.15 } else { 0.0 }).min(1.0),
        _ => 0.12,
    };
    // Au centième : une variation invisible ne redessine pas l'îlot.
    (v * 100.0).round() / 100.0
}

fn art_image(art: &Art) -> slint::Image {
    ART.with_borrow_mut(|cache| {
        if let Some((rgba, img)) = cache.as_ref()
            && std::sync::Arc::ptr_eq(rgba, &art.rgba)
        {
            return img.clone();
        }
        let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&art.rgba, art.size, art.size);
        let img = slint::Image::from_rgba8(buf);
        *cache = Some((art.rgba.clone(), img.clone()));
        img
    })
}

fn with_bar<T>(label: &str, f: impl FnOnce(&mut Bar, &BarWindow) -> T) -> Option<T> {
    BARS.with_borrow_mut(|bars| {
        let bar = bars.iter_mut().find(|b| b.label == label)?;
        let w = bar.window.upgrade()?;
        Some(f(bar, &w))
    })
}

/// Barres encore ouvertes (les fermées sont oubliées).
fn live_bars() -> Vec<(String, BarWindow)> {
    BARS.with_borrow_mut(|bars| {
        bars.retain(|b| b.window.upgrade().is_some());
        bars.iter().filter_map(|b| Some((b.label.clone(), b.window.upgrade()?))).collect()
    })
}

pub(super) fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let g = window.global::<MediaData>();
    g.set_glow_mask(GLOW_MASK.with(Clone::clone));
    let bands = Rc::new(VecModel::from(vec![0.0f32; BANDS]));
    g.set_bands(ModelRc::from(bands.clone()));
    BARS.with_borrow_mut(|b| {
        b.retain(|x| x.label != label);
        b.push(Bar {
            label: label.to_owned(),
            window: window.as_weak(),
            graphic: bar.style == BarStyle::Graphic,
            bands,
            subscribed: false,
            seek_total: 0,
        })
    });

    let full = super::media_state(app);
    on_media(app, label, &full.payload);
    on_art(label, full.art.as_ref());

    let (a, l) = (app.clone(), label.to_owned());
    host::listen(EV_MEDIA, Some(label), move |e| {
        if let Some(p) = e.payload::<MediaPayload>() {
            on_media(&a, &l, p);
        }
    });
    let l = label.to_owned();
    host::listen(EV_MEDIA_ART, Some(label), move |e| {
        if let Some(art) = e.payload::<Option<Art>>() {
            on_art(&l, art.as_ref());
        }
    });
    let l = label.to_owned();
    host::listen(EV_AUDIO, Some(label), move |e| {
        if let Some(f) = e.payload::<AudioFrame>() {
            on_audio(&l, f);
        }
    });

    let a = app.clone();
    g.on_previous(move || report(super::media_prev(&a)));
    let a = app.clone();
    g.on_play_pause(move || report(super::media_play_pause(&a)));
    let a = app.clone();
    g.on_next(move || report(super::media_next(&a)));
    let (a, l) = (app.clone(), label.to_owned());
    g.on_wheel(move |notches, x| on_wheel(&a, &l, notches, x));
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::warn!("lecture en cours : {e}");
    }
}

/// Nouvel état de la lecture (le même pour toutes les barres) : la barre `label` le montre.
fn on_media(app: &AppHandle, label: &str, p: &MediaPayload) {
    NOW.with_borrow_mut(|n| {
        n.info = p.info.clone();
        n.position_ms = p.position_ms;
        n.received = Some(Instant::now());
    });
    let playing = p.info.as_ref().is_some_and(|i| i.playing);
    with_bar(label, |bar, w| {
        let g = w.global::<MediaData>();
        g.set_has_info(p.info.is_some());
        g.set_collapsed(p.collapsed || p.info.is_none());
        g.set_playing(playing);
        if let Some(i) = &p.info {
            g.set_title(i.title.as_str().into());
            g.set_artist(i.artist.as_str().into());
            g.set_app(format::app_name(&i.app_id).into());
            g.set_has_duration(i.duration_ms > 0);
        }
        g.set_progress(NOW.with_borrow(|n| n.progress(Instant::now())));
        if !playing {
            g.set_glow(glow(false, None));
        }
        // Le son n'est suivi que pendant la lecture (le hub ne sert pas les barres masquées).
        if bar.subscribed != playing {
            bar.subscribed = playing;
            app.state::<AudioHub>().subscribe(label, "media", playing, bar.graphic);
        }
    });
    sync_tick();
}

fn on_art(label: &str, art: Option<&Art>) {
    with_bar(label, |_, w| {
        let g = w.global::<MediaData>();
        g.set_has_art(art.is_some());
        if let Some(a) = art {
            g.set_art(art_image(a));
            g.set_art_seq(g.get_art_seq().wrapping_add(1));
        }
    });
}

fn on_audio(label: &str, f: &AudioFrame) {
    let playing = NOW.with_borrow(Now::playing);
    with_bar(label, |bar, w| {
        if bar.graphic {
            for (i, v) in f.bands.iter().take(BANDS).enumerate() {
                // Au centième, et seulement si la bande a bougé.
                let v = (v.clamp(0.0, 1.0) * 100.0).round() / 100.0;
                if bar.bands.row_data(i) != Some(v) {
                    bar.bands.set_row_data(i, v);
                }
            }
        } else {
            w.global::<MediaData>().set_glow(glow(playing, Some(f)));
        }
    });
}

/// Progression : avance chaque seconde pendant la lecture, s'arrête sinon.
fn sync_tick() {
    let playing = NOW.with_borrow(Now::playing);
    TICK.with(|t| {
        if !playing {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                let p = NOW.with_borrow(|n| n.progress(Instant::now()));
                for (_, w) in live_bars() {
                    w.global::<MediaData>().set_progress(p);
                }
            });
        }
    });
}

/// Molette : avancer / reculer de 10 s (comme les boutons du panneau), avec la bulle de la
/// barre ; les crans rapprochés se cumulent (« +30 s · 2:43 »).
fn on_wheel(app: &AppHandle, label: &str, notches: f32, x: f32) {
    let step = if notches > 0.0 { SEEK_MS } else { -SEEK_MS };
    report(super::media_seek_relative(app, step));
    let (at, duration) = NOW.with_borrow(|n| (n.position(Instant::now()), n.info.as_ref().map_or(0, |i| i.duration_ms)));
    with_bar(label, |bar, w| {
        bar.seek_total = if hint::shown(w) { bar.seek_total } else { 0 } + step;
        let total = bar.seek_total;
        let icon = if total >= 0 { "fastForward" } else { "rewind" };
        hint::show(w, label, x, icon, &seek_text(total, at, step, duration), None);
    });
}

/// Texte de la bulle : avance cumulée et position visée (« +20 s · 2:43 », « −10 s · 0:00 »).
fn seek_text(total_ms: i64, position_ms: u64, step_ms: i64, duration_ms: u64) -> String {
    let max = if duration_ms > 0 { duration_ms as i64 } else { i64::MAX };
    let target = (position_ms as i64 + step_ms).clamp(0, max);
    let s = (total_ms as f64 / 1000.0).round() as i64;
    format!("{}{} s · {}", if s >= 0 { '+' } else { '−' }, s.abs(), format::clock(target as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(playing: bool) -> MediaInfo {
        MediaInfo {
            app_id: "a".into(),
            title: "t".into(),
            artist: String::new(),
            playing,
            position_ms: 0,
            duration_ms: 60_000,
            timeline_at_ms: 0,
        }
    }

    #[test]
    fn position_moves_only_while_playing_and_stops_at_the_end() {
        let t0 = Instant::now();
        let n = Now { info: Some(info(true)), position_ms: 10_000, received: Some(t0) };
        assert_eq!(n.position(t0), 10_000);
        assert_eq!(n.position(t0 + Duration::from_secs(5)), 15_000);
        assert_eq!(n.position(t0 + Duration::from_secs(500)), 60_000);
        let paused = Now { info: Some(info(false)), position_ms: 10_000, received: Some(t0) };
        assert_eq!(paused.position(t0 + Duration::from_secs(90)), 10_000);
        assert!((paused.progress(t0) - 1.0 / 6.0).abs() < 1e-6);
        assert_eq!(Now::default().position(t0), 0);
    }

    #[test]
    fn glow_follows_the_sound() {
        let f = |level, beat| AudioFrame { level, bass: 0.0, bands: Vec::new(), beat };
        assert_eq!(glow(false, Some(&f(1.0, true))), 0.12);
        assert_eq!(glow(true, None), 0.12);
        assert_eq!(glow(true, Some(&f(0.5, false))), 0.65);
        assert_eq!(glow(true, Some(&f(0.5, true))), 0.8);
        assert_eq!(glow(true, Some(&f(1.0, true))), 1.0);
    }

    #[test]
    fn glow_mask_fades_from_the_centre() {
        assert!((glow_alpha(0.0, 0.0) - 0.55).abs() < 1e-6);
        assert_eq!(glow_alpha(0.72, 0.0), 0.0);
        assert_eq!(glow_alpha(1.0, 1.0), 0.0);
        assert!((glow_alpha(0.0, 0.36) - 0.275).abs() < 1e-6);
    }

    #[test]
    fn seek_bubble_text() {
        assert_eq!(seek_text(10_000, 120_000, 10_000, 200_000), "+10 s · 2:10");
        assert_eq!(seek_text(30_000, 153_000, 10_000, 200_000), "+30 s · 2:43");
        assert_eq!(seek_text(-10_000, 4_000, -10_000, 200_000), "−10 s · 0:00");
        assert_eq!(seek_text(10_000, 195_000, 10_000, 200_000), "+10 s · 3:20", "bornée à la durée");
        assert_eq!(seek_text(10_000, 5_000, 10_000, 0), "+10 s · 0:15", "durée inconnue");
    }
}
