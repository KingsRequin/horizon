//! Mode jeu léger : une appli en plein écran sur l'écran principal (vue par le masquage, à
//! chaque relecture de l'ordre des fenêtres). Pendant ce temps : capture du son et spectre
//! coupés, mesures système toutes les 6 s au lieu de chaque seconde (sauf panneau ouvert),
//! import de l'historique Vivaldi retardé. Réglage `[features] game_mode`.
//!
//! Repris de `app/src-tauri/src/game.rs`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use crate::host::{AppHandle, Manager};

static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Début du mode jeu en cours (ms Unix), pour la page « État » ; 0 = inactif.
static SINCE: AtomicU64 = AtomicU64::new(0);

/// Le mode jeu est en cours.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// Début du mode jeu en cours.
pub fn since_ms() -> Option<u64> {
    Some(SINCE.load(Ordering::Relaxed)).filter(|&t| t > 0 && active())
}

/// Plein écran sur l'écran principal (ou non) : entre ou sort du mode jeu, si le réglage le
/// permet. Les fils concernés sont réveillés.
pub fn update(app: &AppHandle, fullscreen_on_main: bool) {
    let on = fullscreen_on_main && crate::state::settings().features.game_mode;
    if ACTIVE.swap(on, Ordering::Relaxed) == on {
        return;
    }
    SINCE.store(if on { horizon_core::time::now_ms() } else { 0 }, Ordering::Relaxed);
    log::info!("{}", if on { "mode jeu : spectre coupé, mesures espacées" } else { "fin du mode jeu" });
    app.state::<crate::widgets::media::audio::AudioHub>().wake();
}
