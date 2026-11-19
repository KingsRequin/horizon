//! Absence de l'utilisateur (session verrouillée, écran éteint) : les fils d'affichage
//! s'arrêtent d'eux-mêmes (`horizon_sys::presence::wait_while_away` : mesures système,
//! masquage, micro, notifications, serveurs). Ici, le journal et le réveil de ceux qui dorment
//! sur autre chose (son en cours).
//!
//! Repris de `app/src-tauri/src/presence.rs`.

use crate::util::start_thread;
use crate::host::{AppHandle, Manager};

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    horizon_sys::presence::watch(tx)?;
    let app = app.clone();
    start_thread("presence-hub", move || {
        for away in rx {
            log::info!("{}", if away { "absence (session verrouillée ou écran éteint) : pause" } else { "retour : reprise" });
            crate::plugins::events::emit(&app, "presence", serde_json::json!({ "away": away }));
            app.state::<crate::widgets::media::audio::AudioHub>().wake();
            app.state::<crate::autohide::Autohide>().wake();
        }
    })
}
