//! Lecture en cours dans les barres (global `MediaData` de `ui/widgets/media/widget.slint`) :
//! état et pochette reçus du service, position qui avance chaque seconde pendant la lecture,
//! abonnement au son pour ce que la barre demande (`BarLayout::music`), commandes et molette
//! ±10 s avec la bulle de la barre. Fil de l'interface.
//!
//! Ce qui suit le son : le spectre et les capsules sont des rectangles de l'interface, qui
//! reçoivent leurs valeurs ; le halo, le ruban, l'aurore, les points, la progression qui ondule
//! et la lumière qui tourne (pulsation « comet ») sont une image de l'îlot
//! (`horizon_core::music::canvas`), redessinée à chaque image de son. En pause, dans un silence
//! ou quand le hub cesse d'envoyer (mode jeu, absence), tout retombe en douceur, au rythme des
//! images de son, puis plus rien ne tourne.

use super::audio::{AudioFrame, AudioHub, BANDS, EV_AUDIO};
use super::{Art, EV_MEDIA, EV_MEDIA_ART, MediaPayload, SEEK_MS};
use crate::events::EV_OPTIONS;
use crate::host::{self, AppHandle, Manager};
use crate::ui::{BarWindow, MediaData, format, hint};
use horizon_core::layout::{BarLayout, BarMusic, BarStyle};
use horizon_core::media::MediaInfo;
use horizon_core::music::canvas::{Canvas, Palette, Scene, Sound};
use horizon_core::music::{CAPSULES, capsules};
use horizon_core::settings::{Backdrop, Progress, Pulse};
use slint::{ComponentHandle, Model, ModelRc, SharedPixelBuffer, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Écart entre deux images de son (secondes).
const FRAME: f32 = 0.05;
/// Sans image de son depuis ce délai, ce qui suit le son retombe (comme la pulsation).
const QUIET: Duration = Duration::from_millis(400);
/// Une image de son reçue depuis moins que ce délai redessine de toute façon l'îlot.
const FRESH: Duration = Duration::from_millis(150);

/// Ce que la barre demande au widget.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    backdrop: Backdrop,
    /// La progression ondule.
    wave: bool,
    /// Lumière qui tourne autour de l'îlot (pulsation « comet », barre en îlots).
    comet: bool,
}

impl Look {
    /// Choix de la barre, rien si Windows demande moins d'animations.
    fn of(music: BarMusic, graphic: bool) -> Look {
        let m = music.effective(crate::motion::reduced());
        Look { backdrop: m.backdrop, wave: m.progress == Progress::Wave, comet: m.pulse == Pulse::Comet && !graphic }
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
    music: BarMusic,
    look: Look,
    bands: Rc<VecModel<f32>>,
    capsules: Rc<VecModel<f32>>,
    /// Son adouci pour l'image de l'îlot, et cette image (refaite quand l'îlot change de taille).
    sound: Sound,
    canvas: Option<Canvas>,
    /// Dernière image de son reçue.
    last_frame: Option<Instant>,
    /// Plus d'image de son depuis `QUIET` : la retombée commence.
    quiet: slint::Timer,
    /// Retombée en cours (une image de silence toutes les 50 ms).
    fade: Rc<slint::Timer>,
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
    static APP: RefCell<Option<AppHandle>> = RefCell::default();
}

/// Les images des îlots sont redessinées quand l'accent change (à chaque pas de son fondu),
/// sauf celles qui le sont déjà par les images de son, et celles des barres masquées.
fn follow_accent(app: &AppHandle) {
    if !ACCENT_FOLLOWED.replace(true) {
        let app = app.clone();
        crate::ui::accent::register(move |_| {
            let hub = app.state::<AudioHub>();
            BARS.with_borrow_mut(|bars| {
                for bar in bars.iter_mut() {
                    let fresh = bar.last_frame.is_some_and(|t| t.elapsed() < FRESH);
                    if let (false, false, Some(w)) = (fresh, hub.is_hidden(&bar.label), bar.window.upgrade()) {
                        repaint(bar, &w);
                    }
                }
            });
            true
        });
    }
}

/// Redessine l'image de l'îlot d'une barre (fond en image, progression qui ondule, lumière qui
/// tourne) à la taille que l'interface annonce, ou la retire s'il n'y a rien à dessiner.
fn repaint(bar: &mut Bar, w: &BarWindow) {
    let g = w.global::<MediaData>();
    let (progress, track) = NOW.with_borrow(|n| {
        let known = n.info.as_ref().is_some_and(|i| i.duration_ms > 0);
        ((known && bar.look.wave).then(|| n.progress(Instant::now())), known && !bar.look.wave)
    });
    let scene = Scene { backdrop: bar.look.backdrop, progress, comet: bar.look.comet, track };
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

/// Une image de silence : le son adouci, les bandes et les capsules retombent, l'image est
/// redessinée ; vrai quand tout est retombé.
fn fade_step(bar: &mut Bar, w: &BarWindow) -> bool {
    bar.sound.fade(FRAME);
    let mut still = false;
    for model in [&bar.bands, &bar.capsules] {
        for i in 0..model.row_count() {
            let v = model.row_data(i).unwrap_or(0.0) * 0.6;
            put(model, i, if v < 0.02 { 0.0 } else { v });
            still |= model.row_data(i).unwrap_or(0.0) > 0.0;
        }
    }
    repaint(bar, w);
    let done = bar.sound.is_quiet() && !still;
    if done {
        bar.sound.rest();
        w.global::<MediaData>().set_live(NOW.with_borrow(Now::playing));
    }
    done
}

/// Plus de son (pause, silence, hub muet) : tout retombe en douceur, puis s'arrête.
fn fade_out(bar: &mut Bar) {
    bar.quiet.stop();
    if bar.fade.running() {
        return;
    }
    let label = bar.label.clone();
    bar.fade.start(slint::TimerMode::Repeated, Duration::from_secs_f32(FRAME), move || {
        with_bar(&label, |bar, w| {
            if fade_step(bar, w) {
                bar.fade.stop();
            }
        });
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

/// Le widget a quitté la barre `label` : plus d'abonnement au son, plus rien n'y tourne.
pub(super) fn unbind(app: &AppHandle, label: &str) {
    let gone = BARS.with_borrow_mut(|bars| {
        let i = bars.iter().position(|b| b.label == label)?;
        let b = bars.remove(i);
        b.quiet.stop();
        b.fade.stop();
        Some(b.subscribed)
    });
    if gone == Some(true) {
        app.state::<AudioHub>().subscribe(label, "media", false, false);
    }
}

pub(super) fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    APP.set(Some(app.clone()));
    follow_accent(app);
    let g = window.global::<MediaData>();
    let bands = Rc::new(VecModel::from(vec![0.0f32; BANDS]));
    g.set_bands(ModelRc::from(bands.clone()));
    let caps = Rc::new(VecModel::from(vec![0.0f32; CAPSULES]));
    g.set_capsules(ModelRc::from(caps.clone()));
    let graphic = bar.style == BarStyle::Graphic;
    let look = Look::of(bar.music, graphic);
    show_look(&g, look);
    // Relié de nouveau (autre réglage, autre style) : l'ancien abonnement est remplacé.
    let was_subscribed = BARS.with_borrow_mut(|b| {
        let old = b.iter().position(|x| x.label == label).map(|i| b.remove(i));
        old.is_some_and(|o| {
            o.quiet.stop();
            o.fade.stop();
            o.subscribed
        })
    });
    if was_subscribed {
        app.state::<AudioHub>().subscribe(label, "media", false, false);
    }
    BARS.with_borrow_mut(|b| {
        b.push(Bar {
            label: label.to_owned(),
            window: window.as_weak(),
            graphic,
            music: bar.music,
            look,
            bands,
            capsules: caps,
            sound: Sound::default(),
            canvas: None,
            last_frame: None,
            quiet: slint::Timer::default(),
            fade: Rc::default(),
            subscribed: false,
            seek_total: 0,
        })
    });
    // L'îlot a changé de taille (autre titre, barre resserrée) : son image est à refaire. Une
    // annonce de l'interface arrive à un tour de boucle suivant, jamais pendant une mise à jour
    // de `BARS` : l'emprunt raté ne peut être qu'une précaution.
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
        follow_sound(app, bar, playing);
        if playing {
            g.set_live(true);
        } else {
            fade_out(bar);
        }
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

/// Réglages relus (« Effets d'animation » de Windows a pu changer) : ce que la barre `label`
/// montre est revu, et son abonnement au son refait si cela a changé (les choix de la barre,
/// eux, arrivent par une nouvelle liaison du widget).
fn on_options(app: &AppHandle, label: &str) {
    let playing = NOW.with_borrow(Now::playing);
    with_bar(label, |bar, w| {
        let look = Look::of(bar.music, bar.graphic);
        if look == bar.look {
            return;
        }
        bar.fade.stop();
        bar.sound.rest();
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
        if !bar.subscribed {
            return;
        }
        bar.fade.stop();
        bar.last_frame = Some(Instant::now());
        match bar.look.backdrop {
            Backdrop::Spectrum => f.bands.iter().take(BANDS).enumerate().for_each(|(i, v)| put(&bar.bands, i, *v)),
            Backdrop::Capsules => capsules(&f.bands).iter().enumerate().for_each(|(i, v)| put(&bar.capsules, i, *v)),
            _ => {}
        }
        bar.sound.feed(f.level, f.bass, &f.bands, f.beat, FRAME);
        repaint(bar, w);
        // Le hub se tait (silence, mode jeu, absence) : tout retombera.
        let l = label.to_owned();
        bar.quiet.start(slint::TimerMode::SingleShot, QUIET, move || {
            with_bar(&l, |bar, _| fade_out(bar));
        });
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
                // même quand aucune image de son n'arrive (silence), sauf sur une barre masquée.
                let app = APP.with_borrow(Clone::clone);
                BARS.with_borrow_mut(|bars| {
                    for bar in bars.iter_mut().filter(|b| b.look.wave && !b.fade.running()) {
                        let fresh = bar.last_frame.is_some_and(|t| t.elapsed() < Duration::from_millis(900));
                        let hidden = app.as_ref().is_some_and(|a| a.state::<AudioHub>().is_hidden(&bar.label));
                        if let (false, false, Some(w)) = (fresh, hidden, bar.window.upgrade()) {
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
