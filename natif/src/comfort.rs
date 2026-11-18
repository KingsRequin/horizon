//! Petits conforts : molette = volume général, mode nuit des barres non cliquables (et
//! « Ne pas déranger » la nuit, si c'est demandé).
//!
//! Repris de `app/src-tauri/src/comfort.rs`.

use crate::events::EV_NIGHT;
use crate::state::settings;
use crate::util::{Published, Waker, start_thread};
use horizon_core::night::{is_night, parse_hhmm};
use serde::Serialize;
use std::time::Duration;
use crate::host::{AppHandle, Manager};

/// Points de volume par cran de molette.
const VOLUME_STEP: i32 = 2;
/// Pas de la relecture de l'heure et des réglages.
const PERIOD: Duration = Duration::from_secs(30);

pub struct Night(Published<NightState>, Waker);

impl Default for Night {
    fn default() -> Self {
        Night(Published::new(EV_NIGHT, NightState { night: false, opacity: settings().night.opacity }), Waker::default())
    }
}

impl Night {
    /// Relit tout de suite l'heure et les réglages (réglages modifiés).
    pub fn wake(&self) {
        self.1.wake();
    }
}

#[derive(Clone, PartialEq, Serialize)]
pub struct NightState {
    pub night: bool,
    pub opacity: f32,
}

/// État d'après l'heure locale et `settings.toml`, et « Ne pas déranger » de nuit voulu.
fn current() -> (NightState, bool) {
    let s = settings().night;
    let night = match (parse_hhmm(&s.start), parse_hhmm(&s.end)) {
        (Some(a), Some(b)) => is_night(horizon_core::time::local_minutes(), a, b),
        _ => false,
    };
    (NightState { night, opacity: s.opacity }, night && s.dnd)
}

/// Envoie l'état aux barres quand il change (passage jour / nuit, luminosité modifiée).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("night", move || {
        let mut dnd_applied: Option<bool> = None;
        loop {
            let (state, dnd) = current();
            app.state::<Night>().0.set(&app, state);
            if dnd_applied != Some(dnd) {
                dnd_applied = Some(dnd);
                crate::dnd::set_night(&app, dnd);
            }
            app.state::<Night>().1.wait(PERIOD);
        }
    })
}

/// Commande `night_state`.
pub fn night_state(app: AppHandle) -> NightState {
    app.state::<Night>().0.get()
}

/// Molette sur la barre : `up` = plus fort. Renvoie le nouveau volume (0–100).
pub fn volume_step(up: bool) -> Result<u32, String> {
    horizon_sys::volume::step(if up { VOLUME_STEP } else { -VOLUME_STEP }).map_err(|e| e.to_string())
}
