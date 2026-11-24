//! Lecture en cours dans les barres (global `MediaData` de `ui/widgets/media/widget.slint`) :
//! état et pochette reçus du service, position qui avance chaque seconde pendant la lecture,
//! abonnement au son pour ce que les réglages `[music]` demandent, commandes et molette ±10 s
//! avec la bulle de la barre. Fil de l'interface.
//!
//! Ce qui suit le son : le spectre et les capsules sont des rectangles de l'interface, qui
//! reçoivent leurs valeurs ; le halo, le ruban, l'aurore, les points, la progression qui ondule
//! et la lumière qui tourne (pulsation « comet ») sont une image de l'îlot
//! (`horizon_core::music::canvas`), redessinée à chaque image de son.

use super::audio::{AudioFrame, AudioHub, BANDS, EV_AUDIO};
use super::{Art, EV_MEDIA, EV_MEDIA_ART, MediaPayload, SEEK_MS};
use crate::events::EV_OPTIONS;
use crate::host::{self, AppHandle, Manager};
use crate::ui::{BarWindow, MediaData, format, hint};
use horizon_core::layout::{BarLayout, BarStyle};
use horizon_core::media::MediaInfo;
use horizon_core::music::canvas::{Canvas, Palette, Scene, Sound};
use horizon_core::music::{CAPSULES, capsules};
use horizon_core::settings::{Backdrop, Progress};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Écart entre deux images de son (secondes).
const FRAME: f32 = 0.05;

/// Ce que les réglages demandent au widget d'une barre.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    /// Fond qui suit le son (jamais `Auto` : déjà traduit d'après le style de la barre).
    backdrop: Backdrop,
    /// La progression ondule (`[music] progress = "wave"`).
    wave: bool,
    /// Lumière qui tourne autour de l'îlot (`[music] pulse = "comet"`, pulsation active).
    comet: bool,
}

impl Look {
    fn read(graphic: bool) -> Look {
        let music = crate::state::settings().music;
        Look { backdrop: music.backdrop.resolved(graphic), wave: music.progress == Progress::Wave, comet: crate::ui::pulse::comet() }
    }

    /// Quelque chose suit le son.
    fn follows_sound(&self) -> bool {
        self.backdrop != Backdrop::Off || self.wave || self.comet
    }
}

/// Barre qui montre le widget.
struct Bar {
    label: String,
    window: slint::Weak<BarWindow>,
    graphic: bool,
    look: Look,
    bands: Rc<VecModel<f32>>,
    capsules: Rc<VecModel<f32>>,
    /// Son adouci pour l'image de l'îlot, et cette image (refaite quand l'îlot change de taille).
    sound: Sound,
    canvas: Option<Canvas>,
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
    /// Dernière pochette convertie (partagée par les barres).
    static ART: RefCell<Option<(std::sync::Arc<Vec<u8>>, slint::Image)>> = RefCell::default();
    /// Les images des îlots suivent déjà la couleur d'accent.
    static ACCENT_FOLLOWED: Cell<bool> = const { Cell::new(false) };
}

/// Les images des îlots sont redessinées quand l'accent change (à chaque pas de son fondu).
fn follow_accent() {
    if !ACCENT_FOLLOWED.replace(true) {
        crate::ui::accent::register(|_| {
            repaint_all();
            true
        });
    }
}

/// Redessine l'image de l'îlot d'une barre (fond en image, progression qui ondule, lumière qui
/// tourne) à la taille que l'interface annonce, ou la retire s'il n'y a rien à dessiner.
fn repaint(bar: &mut Bar, w: &BarWindow) {
    let g = w.global::<MediaData>();
    let (playing, progress) = NOW.with_borrow(|n| {
        let known = n.info.as_ref().is_some_and(|i| i.duration_ms > 0);
        (n.playing(), (known && bar.look.wave).then(|| n.progress(Instant::now())))
    });
    let scene = Scene { backdrop: bar.look.backdrop, progress, comet: bar.look.comet, playing };
    let scale = w.window().scale_factor();
    let (width, height) = ((g.get_island_w() * scale).round() as u32, (g.get_island_h() * scale).round() as u32);
    if scene.is_empty() || width == 0 || height == 0 {
        if g.get_canvas_on() {
            g.set_canvas_on(false);
        }
        return;
    }
    let radius = g.get_island_radius();
    if !bar.canvas.as_ref().is_some_and(|c| c.fits(width, height, scale, radius)) {
        bar.canvas = Some(Canvas::new(width, height, scale, radius));
    }
    let Some(canvas) = bar.canvas.as_mut() else { return };
    let pixels = canvas.paint(&scene, &bar.sound, &Palette::from_accent(crate::ui::accent::shown()));
    let buf = SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(pixels, width, height);
    g.set_canvas(slint::Image::from_rgba8_premultiplied(buf));
    if !g.get_canvas_on() {
        g.set_canvas_on(true);
    }
}

fn repaint_all() {
    BARS.with_borrow_mut(|bars| {
        for bar in bars.iter_mut() {
            if let Some(w) = bar.window.upgrade() {
                repaint(bar, &w);
            }
        }
    });
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

/// Identifiant du fond pour l'interface (`MediaData.backdrop`) ; vide : aucun.
fn backdrop_id(b: Backdrop) -> &'static str {
    if b == Backdrop::Off { "" } else { b.id() }
}

/// Valeur d'une bande ou d'une capsule, au centième, posée seulement si elle a bougé.
fn put(model: &VecModel<f32>, i: usize, v: f32) {
    let v = (v.clamp(0.0, 1.0) * 100.0).round() / 100.0;
    if model.row_data(i) != Some(v) {
        model.set_row_data(i, v);
    }
}

/// Pose dans l'interface ce que les réglages demandent au widget.
fn show_look(g: &MediaData, look: Look) {
    g.set_backdrop(backdrop_id(look.backdrop).into());
    g.set_progress_wave(look.wave);
}

pub(super) fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    follow_accent();
    let g = window.global::<MediaData>();
    let bands = Rc::new(VecModel::from(vec![0.0f32; BANDS]));
    g.set_bands(ModelRc::from(bands.clone()));
    let caps = Rc::new(VecModel::from(vec![0.0f32; CAPSULES]));
    g.set_capsules(ModelRc::from(caps.clone()));
    let graphic = bar.style == BarStyle::Graphic;
    let look = Look::read(graphic);
    show_look(&g, look);
    BARS.with_borrow_mut(|b| {
        b.retain(|x| x.label != label);
        b.push(Bar {
            label: label.to_owned(),
            window: window.as_weak(),
            graphic,
            look,
            bands,
            capsules: caps,
            sound: Sound::default(),
            canvas: None,
            subscribed: false,
            seek_total: 0,
        })
    });
    // L'îlot a changé de taille (autre titre, barre resserrée) : son image est à refaire. Si
    // l'interface l'annonce pendant qu'une barre est déjà en train d'être mise à jour, la mise à
    // jour en cours redessine de toute façon.
    let l = label.to_owned();
    g.on_resized(move || {
        BARS.with(|bars| {
            let Ok(mut bars) = bars.try_borrow_mut() else { return };
            if let Some(bar) = bars.iter_mut().find(|b| b.label == l)
                && let Some(w) = bar.window.upgrade()
            {
                repaint(bar, &w);
            }
        });
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
    let (a, l) = (app.clone(), label.to_owned());
    host::listen(EV_OPTIONS, Some(label), move |_| on_options(&a, &l));
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
            bar.sound.rest();
        }
        follow_sound(app, bar, playing);
        repaint(bar, w);
    });
    sync_tick();
}

/// Le son n'est suivi que pendant la lecture, et seulement si quelque chose le montre (le hub
/// ne sert pas les barres masquées) ; les bandes ne sont demandées que pour un fond qui les
/// dessine.
fn follow_sound(app: &AppHandle, bar: &mut Bar, playing: bool) {
    let want = playing && bar.look.follows_sound();
    if bar.subscribed != want {
        bar.subscribed = want;
        app.state::<AudioHub>().subscribe(&bar.label, "media", want, bar.look.backdrop.wants_bands());
    }
}

/// Réglages modifiés : ce qu'ils demandent à la barre `label` est relu, et son abonnement au
/// son refait si cela a changé (un autre fond ne demande pas la même chose au hub).
fn on_options(app: &AppHandle, label: &str) {
    let playing = NOW.with_borrow(Now::playing);
    with_bar(label, |bar, w| {
        let look = Look::read(bar.graphic);
        if look == bar.look {
            return;
        }
        bar.look = look;
        show_look(&w.global::<MediaData>(), look);
        if bar.subscribed {
            bar.subscribed = false;
            app.state::<AudioHub>().subscribe(label, "media", false, false);
        }
        follow_sound(app, bar, playing);
        repaint(bar, w);
    });
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
    if !playing {
        return;
    }
    with_bar(label, |bar, w| {
        match bar.look.backdrop {
            Backdrop::Spectrum => f.bands.iter().take(BANDS).enumerate().for_each(|(i, v)| put(&bar.bands, i, *v)),
            Backdrop::Capsules => capsules(&f.bands).iter().enumerate().for_each(|(i, v)| put(&bar.capsules, i, *v)),
            _ => {}
        }
        bar.sound.feed(f.level, f.bass, &f.bands, f.beat, FRAME);
        repaint(bar, w);
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
                // La progression qui ondule est dans l'image de l'îlot : elle avance avec lui,
                // même quand aucune image de son n'arrive (silence).
                BARS.with_borrow_mut(|bars| {
                    for bar in bars.iter_mut().filter(|b| b.look.wave) {
                        if let Some(w) = bar.window.upgrade() {
                            repaint(bar, &w);
                        }
                    }
                });
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
    fn seek_bubble_text() {
        assert_eq!(seek_text(10_000, 120_000, 10_000, 200_000), "+10 s · 2:10");
        assert_eq!(seek_text(30_000, 153_000, 10_000, 200_000), "+30 s · 2:43");
        assert_eq!(seek_text(-10_000, 4_000, -10_000, 200_000), "−10 s · 0:00");
        assert_eq!(seek_text(10_000, 195_000, 10_000, 200_000), "+10 s · 3:20", "bornée à la durée");
        assert_eq!(seek_text(10_000, 5_000, 10_000, 0), "+10 s · 0:15", "durée inconnue");
    }
}
