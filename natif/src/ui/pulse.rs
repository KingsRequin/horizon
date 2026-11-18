//! Pulsation des barres pendant la lecture (`[features] bar_pulse`) : les îlots (ou la bande du
//! style graphique) s'éclairent de l'accent avec la basse, et flashent à chaque grosse caisse
//! (détecteur d'Essaim, `widgets/media/audio.rs`). Les couches sont dans `ui/pulse.slint` ; ici,
//! seules leurs intensités (global `PulseState` de chaque barre) sont posées, au rythme des
//! images de son (20 par seconde) et seulement quand elles changent : ni transition ni animation.
//! Le son n'est demandé que pendant la lecture, fonction active et animations permises ; le hub
//! n'envoie rien à une barre masquée (plein écran) ni pendant le mode jeu. Fil de l'interface.
//!
//! Repris de `app/src/lib/Pulse.svelte`.

use super::{BarWindow, PulseState};
use crate::events::EV_OPTIONS;
use crate::host::{self, AppHandle, Manager};
use crate::widgets::media::audio::{AudioFrame, AudioHub, EV_AUDIO};
use crate::widgets::media::{EV_MEDIA, MediaPayload, media_state};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// Retombée du flash d'une grosse caisse à chaque image de son (50 ms) : ~300 ms visibles.
const FLASH_DECAY: f32 = 0.62;
/// Sans image de son depuis ce délai (silence, pause, barre masquée), la lueur s'éteint.
const QUIET: Duration = Duration::from_millis(400);
/// Part de l'opacité maximale de la lueur (le flash va jusqu'à 1).
const GLOW_MAX: f32 = 0.9;
/// Écart en dessous duquel une valeur n'est pas reposée.
const STEP: f32 = 0.03;

/// Valeurs posées (lueur de la basse, flash du kick) et retombée du flash.
#[derive(Debug, Default)]
pub struct Meter {
    bass: f32,
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
    /// Image de son : (lueur, flash) à poser, `None` s'ils n'ont pas assez changé.
    pub fn frame(&mut self, bass: f32, beat: bool) -> (Option<f32>, Option<f32>) {
        // Courbe : la lueur reste discrète sur une basse moyenne, franche sur les grosses.
        let glow = put(&mut self.bass, bass.clamp(0.0, 1.0).powf(1.5));
        self.kick = if beat { 1.0 } else { self.kick * FLASH_DECAY };
        (glow, put(&mut self.kick_shown, self.kick))
    }

    pub fn reset(&mut self) -> (Option<f32>, Option<f32>) {
        self.kick = 0.0;
        (put(&mut self.bass, 0.0), put(&mut self.kick_shown, 0.0))
    }
}

struct Bar {
    label: String,
    window: slint::Weak<BarWindow>,
    meter: Meter,
    subscribed: bool,
    quiet: slint::Timer,
}

thread_local! {
    static BARS: RefCell<Vec<Bar>> = RefCell::default();
    static PLAYING: Cell<bool> = const { Cell::new(false) };
    /// `[features] bar_pulse` et animations permises.
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static APP: RefCell<Option<AppHandle>> = RefCell::default();
}

fn enabled() -> bool {
    crate::state::settings().features.bar_pulse && !crate::motion::reduced()
}

/// Suit la lecture et les réglages (fil de l'interface, avant l'ouverture des barres).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    APP.set(Some(app.clone()));
    ENABLED.set(enabled());
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
        ENABLED.set(enabled());
        sync_all();
    });
    Ok(())
}

/// Relie la barre `label`.
pub fn bind(window: &BarWindow, label: &str) {
    BARS.with_borrow_mut(|b| {
        b.retain(|x| x.label != label && x.window.upgrade().is_some());
        b.push(Bar { label: label.to_owned(), window: window.as_weak(), meter: Meter::default(), subscribed: false, quiet: slint::Timer::default() });
    });
    let l = label.to_owned();
    host::listen(EV_AUDIO, Some(label), move |e| {
        if let Some(f) = e.payload::<AudioFrame>() {
            on_audio(&l, f);
        }
    });
    sync_all();
}

/// Abonne ou désabonne chaque barre selon la lecture et le réglage ; éteint les couches sinon.
fn sync_all() {
    let want = ENABLED.get() && PLAYING.get();
    let Some(app) = APP.with_borrow(Clone::clone) else { return };
    let hub = app.state::<AudioHub>();
    BARS.with_borrow_mut(|bars| {
        bars.retain(|b| b.window.upgrade().is_some());
        for b in bars.iter_mut() {
            if b.subscribed != want {
                b.subscribed = want;
                hub.subscribe(&b.label, "pulse", want, false);
            }
            if !want {
                b.quiet.stop();
                let v = b.meter.reset();
                apply(b, v);
            }
        }
    });
}

fn apply(b: &Bar, (glow, flash): (Option<f32>, Option<f32>)) {
    let Some(w) = b.window.upgrade() else { return };
    let g = w.global::<PulseState>();
    // Au centième, comme l'opacité posée par la version Tauri.
    let round = |v: f32| (v * 100.0).round() / 100.0;
    if let Some(v) = glow {
        g.set_glow(round(v * GLOW_MAX));
    }
    if let Some(v) = flash {
        g.set_flash(round(v));
    }
}

fn on_audio(label: &str, f: &AudioFrame) {
    BARS.with_borrow_mut(|bars| {
        let Some(b) = bars.iter_mut().find(|b| b.label == label) else { return };
        if !b.subscribed {
            return;
        }
        let v = b.meter.frame(f.bass, f.beat);
        apply(b, v);
        let l = label.to_owned();
        b.quiet.start(slint::TimerMode::SingleShot, QUIET, move || quiet(&l));
    });
}

/// Plus d'image de son : les couches s'éteignent.
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
        let (g, k) = m.frame(0.5, false);
        assert!((g.unwrap() - 0.5f32.powf(1.5)).abs() < 1e-6);
        assert_eq!(k, None, "pas de coup, flash nul");
        assert_eq!(m.frame(0.51, false).0, None, "variation sous le seuil");
        assert_eq!(m.frame(0.01, false).0, Some(0.0), "retour à zéro toujours posé");
        assert_eq!(m.frame(0.0, false).0, None);
    }

    #[test]
    fn kick_flashes_then_decays_to_zero() {
        let mut m = Meter::default();
        assert_eq!(m.frame(0.0, true).1, Some(1.0));
        assert_eq!(m.frame(0.0, false).1, Some(0.62));
        let mut last = 0.62;
        let mut frames = 0;
        loop {
            frames += 1;
            match m.frame(0.0, false).1 {
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
    fn reset_turns_both_layers_off_once() {
        let mut m = Meter::default();
        m.frame(1.0, true);
        assert_eq!(m.reset(), (Some(0.0), Some(0.0)));
        assert_eq!(m.reset(), (None, None));
    }
}
