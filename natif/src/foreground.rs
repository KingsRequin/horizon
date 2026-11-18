//! Premier plan : un seul crochet (`horizon_sys::foreground`), dont chaque changement est
//! diffusé aux modules abonnés (suggestions, profils, applis ouvertes des raccourcis).
//!
//! Repris tel quel de `app/src-tauri/src/foreground.rs` (seul l'import de l'hôte change).

use crate::util::{lock, start_thread};
use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_sys::foreground::Foreground;
use std::sync::Mutex;
use crate::host::{AppHandle, Manager};

#[derive(Default)]
pub struct ForegroundHub(Mutex<Vec<Sender<Option<Foreground>>>>);

/// Abonnement aux changements de premier plan (à prendre avant `start` pour recevoir l'état
/// du démarrage).
pub fn subscribe(app: &AppHandle) -> Receiver<Option<Foreground>> {
    let (tx, rx) = unbounded();
    lock(&app.state::<ForegroundHub>().0).push(tx);
    rx
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = unbounded();
    horizon_sys::foreground::watch(tx, std::process::id())?;
    let app = app.clone();
    start_thread("premier-plan", move || {
        for fg in rx {
            let subscribers = lock(&app.state::<ForegroundHub>().inner().0).clone();
            for s in subscribers {
                let _ = s.send(fg.clone());
            }
        }
    })
}
