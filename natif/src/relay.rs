//! Connexion unique au relais Discord local (`ws://127.0.0.1:28642`, hébergé par le plugin
//! OpenDeck Discord, alimenté par le plugin BetterDiscord KrispBridge) :
//!  * `{"type":"state","krisp":bool|null}` → voyant Krisp (`mic`) ;
//!  * `{"type":"voice",…}` → widget Discord vocal (`discord`).
//!
//! Envois : `{"type":"set","krisp":bool}` et `{"type":"voice_set","mute"?:bool,"deaf"?:bool}` ;
//! avec le plugin KrispBridge 1.3 et un relais qui transmet les types qu'il ne connaît pas :
//! `hello`, `voice_leave`, `user_volume`, `user_mute`, `status_set` (voir `discord.rs`).
//!
//! Repris de `app/src-tauri/src/relay.rs` (`use crate::host` au lieu de `tauri`).

use crate::wsclient::{WsClient, WsEvent};
use std::sync::OnceLock;
use crate::host::{AppHandle, Manager};

const URL: &str = "ws://127.0.0.1:28642";

/// Client du relais (posé au démarrage).
#[derive(Default)]
pub struct Relay(OnceLock<WsClient>);

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    let client = crate::wsclient::spawn("relais-discord", URL, move |ev, _| match ev {
        // Le RPC de Discord alimente déjà le widget : le relais se tait.
        _ if a.state::<crate::discord::Discord>().active() => {}
        WsEvent::Message(m) => {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(m) else { return };
            match v["type"].as_str() {
                Some("state") => crate::widgets::mic_krisp::on_krisp(&a, v["krisp"].as_bool()),
                Some("voice") => crate::widgets::discord_voice::on_voice(&a, v),
                _ => {}
            }
        }
        WsEvent::Disconnected => {
            crate::widgets::mic_krisp::on_krisp(&a, None);
            crate::widgets::discord_voice::on_voice(&a, serde_json::json!({ "type": "voice", "available": false }));
        }
        WsEvent::Connected => {}
    })?;
    app.state::<Relay>().0.set(client).map_err(|_| anyhow::anyhow!("relais déjà démarré"))
}

/// Envoie un message au relais (perdu s'il est absent).
pub fn send(app: &AppHandle, msg: serde_json::Value) {
    if let Some(c) = app.state::<Relay>().0.get() {
        c.send(msg.to_string());
    }
}
