//! Pulsation des barres pendant la lecture (`[features] bar_pulse`, animation `[music] pulse`) :
//! les barres suivent la basse, le niveau et chaque grosse caisse (détecteur d'Essaim,
//! `widgets/media/audio.rs`). Les animations sont dans `ui/pulse.slint` ; ici, seules les valeurs
//! du son (global `PulseState` de chaque barre) sont posées, au rythme des images de son (20 par
//! seconde) et seulement quand elles changent. Le son n'est demandé que pendant la lecture,
//! fonction active et animations permises ; le hub n'envoie rien à une barre masquée (plein
//! écran) ni pendant le mode jeu. Fil de l'interface.
//!
//! Repris de `app/src/lib/Pulse.svelte` (animation « edges »).

use super::{BarWindow, PulseState};
use crate::events::EV_OPTIONS;
use crate::host::{self, AppHandle, Manager};
use crate::widgets::media::audio::{AudioFrame, AudioHub, EV_AUDIO, Lateness};
use crate::widgets::media::{EV_MEDIA, MediaPayload, media_state};
use horizon_core::settings::Pulse;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// Retombée du flash d'une grosse caisse à chaque image de son (50 ms) : ~300 ms visibles.
const FLASH_DECAY: f32 = 0.62;
/// Sans image de son depuis ce délai (silence, pause, barre masquée), tout s'éteint.
const QUIET: Duration = Duration::from_millis(400);
/// Part de l'opacité maximale de la lueur (le flash va jusqu'à 1).
const GLOW_MAX: f32 = 0.9;
/// Écart en dessous duquel une valeur n'est pas reposée.
const STEP: f32 = 0.03;
/// Le compteur de grosses caisses repart de zéro avant cette valeur.
const BEATS: i32 = 1_000_000;

/// Valeurs à poser après une image de son ; `None` : elle n'a pas assez changé.
#[derive(Debug, Default, PartialEq)]
pub struct Shown {
    /// Basse (lueur).
    glow: Option<f32>,
    /// Grosse caisse qui retombe (« edges »).
    flash: Option<f32>,
    /// Niveau (« breath »).
    level: Option<f32>,
    /// Une grosse caisse à rejouer par l'animation (toutes sauf « edges », qui a son flash).
    beat: bool,
}

/// Constantes de temps du niveau de la respiration (secondes) : il monte en un souffle et
/// retombe lentement. Adouci ici, il n'a pas besoin d'une transition de l'interface.
const BREATH_UP: f32 = 0.12;
const BREATH_DOWN: f32 = 0.7;
/// Écart entre deux images de son.
const FRAME: f32 = 0.05;

/// Valeurs posées, niveau adouci de la respiration et retombée du flash.
#[derive(Debug, Default)]
pub struct Meter {
    bass: f32,
    level: f32,
    breath: f32,
    kick_shown: f32,
    kick: f32,
}

/// Une valeur à poser si elle a vraiment changé (un retour à zéro passe toujours).
fn put(shown: &mut f32, v: f32) -> Option<f32> {
    let cut = if v < STEP { 0.0 } else { v };
    if (cut - *shown).abs() < STEP && !(cut == 0.0 && *shown != 0.0) {
        return None;
    }
    *shown = cut;
    Some(cut)
}

impl Meter {
    /// Image de son vue par l'animation `style` : chaque animation ne reçoit que ce qu'elle
    /// dessine, le reste retombe à zéro.
    pub fn frame(&mut self, style: Pulse, level: f32, bass: f32, beat: bool) -> Shown {
        let (bass, level) = if style == Pulse::Breath { (0.0, level.clamp(0.0, 1.0)) } else { (bass, 0.0) };
        // Courbe : la lueur reste discrète sur une basse moyenne, franche sur les grosses.
        let glow = put(&mut self.bass, bass.clamp(0.0, 1.0).powf(1.5));
        let tau = if level > self.breath { BREATH_UP } else { BREATH_DOWN };
        self.breath = if style == Pulse::Breath { self.breath + (level - self.breath) * (1.0 - (-FRAME / tau).exp()) } else { 0.0 };
        let level = put(&mut self.level, self.breath);
        self.kick = match style {
            Pulse::Edges if beat => 1.0,
            Pulse::Edges => self.kick * FLASH_DECAY,
            _ => 0.0,
        };
        Shown { glow, flash: put(&mut self.kick_shown, self.kick), level, beat: beat && style != Pulse::Edges }
    }

    pub fn reset(&mut self) -> Shown {
        self.kick = 0.0;
        self.breath = 0.0;
        Shown { glow: put(&mut self.bass, 0.0), flash: put(&mut self.kick_shown, 0.0), level: put(&mut self.level, 0.0), beat: false }
    }
}

struct Bar {
    label: String,
    window: slint::Weak<BarWindow>,
    meter: Meter,
    late: Lateness,
    subscribed: bool,
    quiet: slint::Timer,
}

thread_local! {
    static BARS: RefCell<Vec<Bar>> = RefCell::default();
    static PLAYING: Cell<bool> = const { Cell::new(false) };
    /// Animation en service : `[features] bar_pulse`, animations permises, `[music] pulse`.
    static STYLE: Cell<Option<Pulse>> = const { Cell::new(None) };
    static APP: RefCell<Option<AppHandle>> = RefCell::default();
}

/// Animation choisie, si la pulsation est active et les animations permises.
fn chosen() -> Option<Pulse> {
    let s = crate::state::settings();
    (s.features.bar_pulse && !crate::motion::reduced()).then_some(s.music.pulse)
}

/// Animation dessinée par les barres : la lumière qui tourne ne l'est pas ici, mais dans
/// l'image de l'îlot de Lecture en cours.
fn style() -> Option<Pulse> {
    chosen().filter(|p| *p != Pulse::Comet)
}

/// La lumière qui tourne est en service (`widgets/media/ui.rs` la dessine).
pub fn comet() -> bool {
    chosen() == Some(Pulse::Comet)
}

/// Suit la lecture et les réglages (fil de l'interface, avant l'ouverture des barres).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    APP.set(Some(app.clone()));
    STYLE.set(style());
    PLAYING.set(media_state(app).payload.info.is_some_and(|i| i.playing));
    host::listen(EV_MEDIA, None, |e| {
        if let Some(p) = e.payload::<MediaPayload>() {
            let playing = p.info.as_ref().is_some_and(|i| i.playing);
            if PLAYING.replace(playing) != playing {
                sync_all();
            }
        }
    });
    host::listen(EV_OPTIONS, None, |_| {
        STYLE.set(style());
        sync_all();
    });
    Ok(())
}

/// Relie la barre `label`.
pub fn bind(window: &BarWindow, label: &str) {
    BARS.with_borrow_mut(|b| {
        b.retain(|x| x.label != label && x.window.upgrade().is_some());
        b.push(Bar {
            label: label.to_owned(),
            window: window.as_weak(),
            meter: Meter::default(),
            late: Lateness::default(),
            subscribed: false,
            quiet: slint::Timer::default(),
        });
    });
    let l = label.to_owned();
    host::listen(EV_AUDIO, Some(label), move |e| {
        if let Some(f) = e.payload::<AudioFrame>() {
            on_audio(&l, f);
        }
    });
    sync_all();
}

/// Donne à chaque barre son animation, l'abonne au son ou l'en désabonne selon la lecture ;
/// éteint tout sinon.
fn sync_all() {
    let style = STYLE.get();
    let want = style.is_some() && PLAYING.get();
    let Some(app) = APP.with_borrow(Clone::clone) else { return };
    let hub = app.state::<AudioHub>();
    BARS.with_borrow_mut(|bars| {
        bars.retain(|b| b.window.upgrade().is_some());
        for b in bars.iter_mut() {
            let Some(w) = b.window.upgrade() else { continue };
            let g = w.global::<PulseState>();
            let id = style.map_or("", Pulse::id);
            let changed = g.get_style().as_str() != id;
            if changed {
                g.set_style(id.into());
            }
            g.set_active(want);
            if b.subscribed != want {
                b.subscribed = want;
                hub.subscribe(&b.label, "pulse", want, false);
            }
            // Autre animation : les valeurs de l'ancienne ne passent pas à la nouvelle.
            if !want || changed {
                b.quiet.stop();
                let v = b.meter.reset();
                apply(b, v);
            }
        }
    });
}

fn apply(b: &Bar, v: Shown) {
    let Some(w) = b.window.upgrade() else { return };
    let g = w.global::<PulseState>();
    // Au centième, comme l'opacité posée par la version Tauri.
    let round = |v: f32| (v * 100.0).round() / 100.0;
    if let Some(v) = v.glow {
        g.set_glow(round(v * GLOW_MAX));
    }
    if let Some(v) = v.flash {
        g.set_flash(round(v));
    }
    if let Some(v) = v.level {
        g.set_level(round(v));
    }
    if v.beat {
        g.set_beat((g.get_beat() + 1) % BEATS);
    }
}

/// Image de son reçue par la barre `label`, pour la pulsation ou pour le fond de la lecture en
/// cours : son retard est relevé dans tous les cas (les deux lissent leurs valeurs, voir
/// [`Lateness`]), puis la pulsation en service la dessine.
fn on_audio(label: &str, f: &AudioFrame) {
    BARS.with_borrow_mut(|bars| {
        let Some(b) = bars.iter_mut().find(|b| b.label == label) else { return };
        let late = b.late.observe(f.sent_ms, horizon_core::time::now_ms());
        if let Some(w) = b.window.upgrade() {
            let g = w.global::<PulseState>();
            if g.get_late() != late {
                g.set_late(late);
            }
        }
        let Some(style) = STYLE.get() else { return };
        if !b.subscribed {
            return;
        }
        let v = b.meter.frame(style, f.level, f.bass, f.beat);
        apply(b, v);
        let l = label.to_owned();
        b.quiet.start(slint::TimerMode::SingleShot, QUIET, move || quiet(&l));
    });
}

/// Plus d'image de son : tout s'éteint.
fn quiet(label: &str) {
    BARS.with_borrow_mut(|bars| {
        if let Some(b) = bars.iter_mut().find(|b| b.label == label) {
            let v = b.meter.reset();
            apply(b, v);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_set_only_when_they_change() {
        let mut m = Meter::default();
        let v = m.frame(Pulse::Edges, 0.0, 0.5, false);
        assert!((v.glow.unwrap() - 0.5f32.powf(1.5)).abs() < 1e-6);
        assert_eq!(v.flash, None, "pas de coup, flash nul");
        assert_eq!(m.frame(Pulse::Edges, 0.0, 0.51, false).glow, None, "variation sous le seuil");
        assert_eq!(m.frame(Pulse::Edges, 0.0, 0.01, false).glow, Some(0.0), "retour à zéro toujours posé");
        assert_eq!(m.frame(Pulse::Edges, 0.0, 0.0, false).glow, None);
    }

    #[test]
    fn kick_flashes_then_decays_to_zero() {
        let mut m = Meter::default();
        let first = m.frame(Pulse::Edges, 0.0, 0.0, true);
        assert_eq!(first.flash, Some(1.0));
        assert!(!first.beat, "le flash suffit : pas de coup à rejouer");
        assert_eq!(m.frame(Pulse::Edges, 0.0, 0.0, false).flash, Some(0.62));
        let mut last = 0.62;
        let mut frames = 0;
        loop {
            frames += 1;
            match m.frame(Pulse::Edges, 0.0, 0.0, false).flash {
                Some(0.0) => break,
                Some(v) => {
                    assert!(v < last);
                    last = v;
                }
                None => {}
            }
            assert!(frames < 20);
        }
        assert!(frames <= 8, "~300 ms visibles à 20 images/s ({frames})");
    }

    #[test]
    fn each_animation_gets_only_what_it_draws() {
        let mut m = Meter::default();
        let v = m.frame(Pulse::Breath, 0.6, 0.9, true);
        assert_eq!((v.glow, v.flash, v.beat), (None, None, true), "respiration : niveau et coup");
        assert!(v.level.is_some_and(|l| l > 0.1 && l < 0.4), "le niveau monte en douceur : {:?}", v.level);
        for _ in 0..40 {
            m.frame(Pulse::Breath, 0.6, 0.0, false);
        }
        assert!((m.breath - 0.6).abs() < 0.01);
        m.frame(Pulse::Breath, 0.0, 0.0, false);
        assert!(m.breath > 0.5, "et retombe lentement");
        for style in [Pulse::Wave, Pulse::Horizon] {
            let mut m = Meter::default();
            let v = m.frame(style, 0.6, 1.0, true);
            assert_eq!((v.glow, v.flash, v.level, v.beat), (Some(1.0), None, None, true), "{style:?} : basse et coup");
            assert!(!m.frame(style, 0.6, 1.0, false).beat);
        }
    }

    #[test]
    fn reset_turns_everything_off_once() {
        let mut m = Meter::default();
        m.frame(Pulse::Edges, 0.0, 1.0, true);
        assert_eq!(m.reset(), Shown { glow: Some(0.0), flash: Some(0.0), level: None, beat: false });
        assert_eq!(m.reset(), Shown::default());
        m.frame(Pulse::Breath, 0.8, 0.0, false);
        assert_eq!(m.reset().level, Some(0.0));
    }
}
