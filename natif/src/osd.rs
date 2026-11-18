//! Affichage du volume et du média dans la barre principale, à la place de celui de Windows
//! (réglages `[features] osd` et `osd_track`). Sources : le volume général changé ailleurs
//! qu'Horizon (touches, autre appli ; la molette d'Horizon a son propre contexte et sa bulle),
//! les touches de lecture (crochet du shell, pas de crochet clavier) et les changements de
//! morceau (widget média).
//!
//! L'affichage de Windows est masqué tant que celui d'Horizon le remplace ; il revient quand la
//! fonction est coupée, quand l'écran principal passe en plein écran (barre masquée : sinon
//! plus aucun retour) et à la fermeture d'Horizon.
//!
//! Repris de `app/src-tauri/src/osd.rs` (affichage : `ui::osd`).

use crate::events::EV_OSD;
use crate::host::{AppHandle, Emitter};
use crate::util::start_thread;
use horizon_sys::appcommand::AppCommand;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Ce que la barre principale affiche.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OsdEvent {
    Volume { level: u32, muted: bool },
    Media { action: MediaAction },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaAction {
    PlayPause,
    Next,
    Previous,
    Stop,
    /// Nouveau morceau (sans touche).
    Track,
}

/// Plein écran sur l'écran principal : sa barre est masquée.
static MAIN_FULLSCREEN: AtomicBool = AtomicBool::new(false);

/// Horizon remplace l'affichage de Windows en ce moment.
fn active() -> bool {
    crate::state::settings().features.osd && !MAIN_FULLSCREEN.load(Ordering::Relaxed)
}

/// Action affichée pour une touche multimédia ; `None` pour le volume, qui arrive par son
/// propre suivi (valeur exacte, toutes sources).
pub fn media_action(cmd: AppCommand) -> Option<MediaAction> {
    Some(match cmd {
        AppCommand::PlayPause => MediaAction::PlayPause,
        AppCommand::Next => MediaAction::Next,
        AppCommand::Previous => MediaAction::Previous,
        AppCommand::Stop => MediaAction::Stop,
        AppCommand::VolumeUp | AppCommand::VolumeDown | AppCommand::VolumeMute => return None,
    })
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    // Masquage de l'affichage de Windows : après chaque touche, l'îlot est cherché à plusieurs
    // reprises (Windows le montre quelques dizaines de millisecondes après la touche).
    let (kick, kicks) = crossbeam_channel::unbounded::<()>();
    start_thread("osd-windows", move || {
        while kicks.recv().is_ok() {
            for wait in [0, 40, 120, 300, 700] {
                std::thread::sleep(Duration::from_millis(wait));
                while kicks.try_recv().is_ok() {}
                if active() {
                    horizon_sys::osd_native::hide();
                }
            }
        }
    })?;

    let (tx, rx) = crossbeam_channel::unbounded();
    horizon_sys::appcommand::watch(tx)?;
    let (a, k) = (app.clone(), kick.clone());
    start_thread("osd-touches", move || {
        for cmd in rx {
            if !active() {
                continue;
            }
            let _ = k.send(());
            if let Some(action) = media_action(cmd) {
                let _ = a.emit(EV_OSD, OsdEvent::Media { action });
            }
        }
    })?;

    // Essai (`HORIZON_OSD_ESSAI`) : affichages factices, sans toucher au son ni à la lecture.
    if std::env::var_os("HORIZON_OSD_ESSAI").is_some() {
        let a = app.clone();
        start_thread("osd-essai", move || {
            let events = [
                OsdEvent::Volume { level: 52, muted: false },
                OsdEvent::Media { action: MediaAction::Track },
                OsdEvent::Media { action: MediaAction::Next },
                OsdEvent::Volume { level: 0, muted: true },
            ];
            for e in events {
                std::thread::sleep(Duration::from_secs(6));
                log::info!("essai de l'affichage du volume : {e:?}");
                let _ = a.emit(EV_OSD, e);
            }
        })?;
    }

    let a = app.clone();
    horizon_sys::volume::watch(move |c| {
        if active() {
            let _ = kick.send(());
            let _ = a.emit(EV_OSD, OsdEvent::Volume { level: c.level, muted: c.muted });
        }
    })
}

/// Nouveau morceau en cours de lecture (widget média).
pub fn track_changed(app: &AppHandle) {
    if active() && crate::state::settings().features.osd_track {
        let _ = app.emit(EV_OSD, OsdEvent::Media { action: MediaAction::Track });
    }
}

/// Plein écran sur l'écran principal (vu par le masquage des barres) : l'affichage de Windows
/// revient pendant ce temps.
pub fn set_main_fullscreen(on: bool) {
    if MAIN_FULLSCREEN.swap(on, Ordering::Relaxed) != on && on {
        horizon_sys::osd_native::restore();
    }
}

/// Réglages modifiés : fonction coupée → l'affichage de Windows revient.
pub fn settings_changed() {
    if !crate::state::settings().features.osd {
        horizon_sys::osd_native::restore();
    }
}

/// Fermeture d'Horizon (et panique) : l'affichage de Windows revient.
pub fn exit() {
    horizon_sys::osd_native::restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_map_to_media_actions_and_volume_is_left_to_its_watch() {
        assert_eq!(media_action(AppCommand::PlayPause), Some(MediaAction::PlayPause));
        assert_eq!(media_action(AppCommand::Next), Some(MediaAction::Next));
        assert_eq!(media_action(AppCommand::Previous), Some(MediaAction::Previous));
        assert_eq!(media_action(AppCommand::Stop), Some(MediaAction::Stop));
        for v in [AppCommand::VolumeUp, AppCommand::VolumeDown, AppCommand::VolumeMute] {
            assert_eq!(media_action(v), None);
        }
    }

    #[test]
    fn events_keep_the_tauri_shape() {
        let v = serde_json::to_value(OsdEvent::Volume { level: 52, muted: false }).unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "volume", "level": 52, "muted": false }));
        let m = serde_json::to_value(OsdEvent::Media { action: MediaAction::PlayPause }).unwrap();
        assert_eq!(m, serde_json::json!({ "kind": "media", "action": "play_pause" }));
    }
}
