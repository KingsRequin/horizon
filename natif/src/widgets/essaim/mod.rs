//! Essaim, curseur animé réactif à la musique (appli à part, `ws://127.0.0.1:28670`, même
//! forme que Prisme) : l'état qu'il pousse (`{"event":"state"}`, à chaque changement et au moins
//! toutes les 2 s) est relayé aux fenêtres ; pause (`set_paused`), réglage en direct
//! (`set_config` : Essaim l'écrit dans son `config.toml` et le recharge dans la seconde) et
//! ouverture de ce fichier (`open_config`) sont commandés depuis le widget.
//!
//! Repris de `app/src-tauri/src/widgets/essaim.rs`. Partie interface : `ui`.

mod panel;
pub mod ui;

use crate::util::Published;
use crate::widgets::Backend;
use crate::wsclient::{WsClient, WsEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use crate::host::{AppHandle, Manager};

/// État d'Essaim.
pub const EV_ESSAIM: &str = "horizon://essaim";

pub const BACKEND: Backend = Backend {
    id: "essaim",
    name: "Essaim",
    manage: |app| crate::widgets::manage(app, EssaimState::default()),
    start: Some(start),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};


const URL: &str = "ws://127.0.0.1:28670";
const ABSENT: &str = "Essaim n’est pas lancé";

/// État d'Essaim pour le widget ; `connected: false` = Essaim absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Essaim {
    /// Rempli ici (absent des messages d'Essaim).
    pub connected: bool,
    /// Mis en pause par l'utilisateur.
    pub paused: bool,
    /// En veille : appli en plein écran sur l'écran de la souris.
    pub fullscreen: bool,
    /// Ni en pause ni en veille.
    pub active: bool,
    /// Dessin par la carte graphique (sinon par le processeur).
    pub gpu: bool,
    /// Couleur synchronisée sur Prisme, connecté.
    pub prisme: bool,
    /// Réglages (`config.toml`, par section), transmis tels quels ; `null` hors connexion.
    pub config: Value,
}

impl Essaim {
    /// État tiré d'un événement `state` d'Essaim.
    fn from_event(msg: &str) -> Option<Essaim> {
        let mut v: Value = serde_json::from_str(msg).ok()?;
        if v["event"] != "state" {
            return None;
        }
        let e = Essaim::deserialize(v["data"].take()).ok()?;
        Some(Essaim { connected: true, ..e })
    }
}

/// Lien avec Essaim ; le client est posé au démarrage (`start`).
pub struct EssaimState {
    client: OnceLock<WsClient>,
    state: Published<Essaim>,
    next_id: AtomicU64,
}

impl Default for EssaimState {
    fn default() -> Self {
        EssaimState { client: OnceLock::new(), state: Published::new(EV_ESSAIM, Essaim::default()), next_id: AtomicU64::new(0) }
    }
}

impl EssaimState {
    /// Envoie une commande à Essaim (erreur s'il est absent ; sa réponse arrive par le journal en
    /// cas de refus, l'effet par l'état poussé).
    fn command(&self, cmd: &str, args: Value) -> Result<(), String> {
        let client = self.client.get().filter(|_| self.state.get().connected).ok_or(ABSENT)?;
        client.send(request(self.next_id.fetch_add(1, Ordering::Relaxed) + 1, cmd, args));
        Ok(())
    }
}

fn request(id: u64, cmd: &str, mut args: Value) -> String {
    args["id"] = json!(id);
    args["cmd"] = json!(cmd);
    args.to_string()
}

/// Connexion à Essaim (se refait seule) ; son état part aux fenêtres.
fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    let client = crate::wsclient::spawn("essaim", URL, move |ev, _| {
        let state = &a.state::<EssaimState>().state;
        match ev {
            // Essaim pousse son état dès la connexion.
            WsEvent::Connected => {}
            WsEvent::Message(m) => {
                if let Some(e) = Essaim::from_event(m) {
                    state.set(&a, e);
                } else if m.contains("\"ok\":false") {
                    log::warn!("essaim : {m}");
                }
            }
            WsEvent::Disconnected => {
                state.set(&a, Essaim::default());
            }
        }
    })?;
    app.state::<EssaimState>().client.set(client).map_err(|_| anyhow::anyhow!("Essaim déjà démarré"))
}

pub fn essaim_state(app: AppHandle) -> Essaim {
    app.state::<EssaimState>().state.get()
}

pub fn essaim_set_paused(app: AppHandle, paused: bool) -> Result<(), String> {
    app.state::<EssaimState>().command("set_paused", json!({ "paused": paused }))
}

/// Un réglage de `config.toml` (`[section] key = value`, valeur du même type que dans `config`).
pub fn essaim_set_config(app: AppHandle, section: String, key: String, value: Value) -> Result<(), String> {
    if !(value.is_boolean() || value.is_number()) {
        return Err(format!("[{section}] {key} : valeur ni nombre ni case"));
    }
    app.state::<EssaimState>().command("set_config", json!({ "section": section, "key": key, "value": value }))
}

/// Ouvre `config.toml` d'Essaim dans l'éditeur.
pub fn essaim_open_config(app: AppHandle) -> Result<(), String> {
    app.state::<EssaimState>().command("open_config", json!({}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_essaim_state_event() {
        let m = r#"{"event":"state","data":{"active":false,"config":{"orbite":{"rayon":22.0}},
            "fullscreen":false,"gpu":true,"paused":true,"prisme":true}}"#;
        let e = Essaim::from_event(m).unwrap();
        assert!(e.connected && e.paused && e.gpu && e.prisme);
        assert!(!e.active && !e.fullscreen);
        assert_eq!(e.config["orbite"]["rayon"], json!(22.0));
    }

    #[test]
    fn tolerates_missing_fields_and_ignores_replies() {
        let e = Essaim::from_event(r#"{"event":"state","data":{"paused":false}}"#).unwrap();
        assert!(e.connected);
        assert_eq!(e.config, Value::Null);
        assert_eq!(Essaim::from_event(r#"{"id":3,"ok":true,"data":null}"#), None);
        assert_eq!(Essaim::from_event(r#"{"event":"autre","data":{}}"#), None);
        assert_eq!(Essaim::from_event("pas du json"), None);
    }

    #[test]
    fn absent_essaim_serializes_as_disconnected() {
        let v = serde_json::to_value(Essaim::default()).unwrap();
        assert_eq!(v["connected"], json!(false));
        assert_eq!(v["config"], Value::Null);
    }

    #[test]
    fn requests_carry_id_and_command() {
        let m: Value = serde_json::from_str(&request(4, "set_config", json!({ "section": "orbite", "key": "rayon", "value": 24 }))).unwrap();
        assert_eq!(m, json!({ "id": 4, "cmd": "set_config", "section": "orbite", "key": "rayon", "value": 24 }));
    }
}
