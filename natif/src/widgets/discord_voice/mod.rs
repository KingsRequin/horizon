//! Discord vocal : salon, participants et qui parle, micro coupé / son coupé (relais Discord).
//! Le message `voice` du plugin BetterDiscord est lu (champs dont Horizon se sert), complété
//! des commandes utilisables (`can`) et transmis aux barres ; ses autres champs passent tels
//! quels.
//!
//! Commandes supplémentaires (plugin KrispBridge 1.3) : quitter le salon, volume et sourdine
//! locale d'un participant, statut « Ne pas déranger ». Le plugin annonce ce qu'il sait faire
//! (`caps`) et si ces commandes lui parviennent (`bridged`) : le relais 28642 d'avant ne
//! transmet que les types qu'il connaît. Horizon envoie `{"type":"hello"}` quand le plugin a
//! des `caps` sans être `bridged` ; un relais qui transmet le message le fait savoir au plugin.
//!
//! Repris de `app/src-tauri/src/widgets/discord_voice.rs`. Partie interface : `ui`.

mod panel;
pub mod ui;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use crate::host::{AppHandle, Emitter, Manager};
use crate::widgets::Backend;

/// Discord vocal.
pub const EV_VOICE: &str = "horizon://voice";

pub const BACKEND: Backend = Backend {
    id: "discord_voice",
    name: "Discord vocal",
    manage: |app| crate::widgets::manage(app, VoiceState::default()),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};


/// Un `hello` sans réponse n'est renvoyé qu'après ce délai (le relais d'avant le rejette en
/// l'écrivant dans son journal).
const HELLO_EVERY: Duration = Duration::from_secs(60);

/// État vocal envoyé par le plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Voice {
    /// Discord ouvert et plugin connecté.
    #[serde(default)]
    pub available: bool,
    /// Salon en cours (`null` hors appel).
    #[serde(default)]
    pub channel: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mute: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deaf: Option<bool>,
    /// Commandes que le plugin sait faire (plugin 1.3 ; absent avant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caps: Option<BTreeMap<String, bool>>,
    /// Le relais transmet ces commandes au plugin.
    #[serde(default)]
    pub bridged: bool,
    /// Commandes utilisables maintenant (calculé ici, lu par l'interface).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub can: BTreeMap<String, bool>,
    /// Le reste du message (type, serveur, participants…), transmis tel quel.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Voice {
    fn unavailable() -> Voice {
        let mut rest = Map::new();
        rest.insert("type".into(), json!("voice"));
        Voice { rest, ..Voice::default() }
    }

    pub fn in_call(&self) -> bool {
        self.channel.as_ref().is_some_and(|c| !c.is_null())
    }

    /// Le plugin sait faire `cap` et ses commandes lui parviennent.
    fn can(&self, cap: &str) -> bool {
        self.available && self.bridged && self.caps.as_ref().is_some_and(|c| c.get(cap) == Some(&true))
    }

    /// Le plugin attend un `hello` pour savoir si le relais transmet ses nouvelles commandes.
    fn needs_hello(&self) -> bool {
        self.available && self.caps.is_some() && !self.bridged
    }

    /// Complète `can` : pour chaque capacité annoncée, si la commande est utilisable.
    fn with_can(mut self) -> Voice {
        let caps: Vec<String> = self.caps.iter().flat_map(|c| c.keys().cloned()).collect();
        self.can = caps.into_iter().map(|c| (c.clone(), self.can(&c))).collect();
        self
    }
}

struct Inner {
    voice: Voice,
    /// Statut voulu par « Ne pas déranger » (`None` = jamais demandé) et dernier envoyé au
    /// plugin connecté.
    dnd_wanted: Option<bool>,
    dnd_sent: Option<bool>,
    last_hello: Option<Instant>,
}

pub struct VoiceState(Mutex<Inner>);

impl Default for VoiceState {
    fn default() -> Self {
        VoiceState(Mutex::new(Inner { voice: Voice::unavailable(), dnd_wanted: None, dnd_sent: None, last_hello: None }))
    }
}

fn lock(app: &AppHandle) -> std::sync::MutexGuard<'_, Inner> {
    crate::util::lock(&app.state::<VoiceState>().inner().0)
}

/// Message de statut à envoyer pour rattraper ce qui est voulu, s'il y a lieu.
fn status_message(v: &Voice, wanted: Option<bool>, sent: Option<bool>) -> Option<Value> {
    let w = wanted?;
    (v.can("status") && sent != Some(w))
        .then(|| json!({ "type": "status_set", "status": if w { "dnd" } else { "online" } }))
}

/// Envoie le statut voulu s'il n'est pas encore parvenu au plugin.
fn sync_status(app: &AppHandle, i: &mut Inner) {
    if let Some(m) = status_message(&i.voice, i.dnd_wanted, i.dnd_sent) {
        crate::relay::send(app, m);
        i.dnd_sent = i.dnd_wanted;
    }
}

/// Message `voice` du relais (illisible : Discord vu comme indisponible).
pub fn on_voice(app: &AppHandle, raw: Value) {
    let v = serde_json::from_value::<Voice>(raw)
        .inspect_err(|e| log::warn!("état vocal illisible : {e}"))
        .unwrap_or_else(|_| Voice::unavailable())
        .with_can();
    let changed = {
        let mut i = lock(app);
        let changed = i.voice != v;
        i.voice = v.clone();
        // Plugin parti ou relais qui ne transmet plus : le statut sera renvoyé.
        if !v.bridged {
            i.dnd_sent = None;
        }
        if v.needs_hello() && i.last_hello.is_none_or(|t| t.elapsed() >= HELLO_EVERY) {
            i.last_hello = Some(Instant::now());
            crate::relay::send(app, json!({ "type": "hello" }));
        }
        sync_status(app, &mut i);
        changed
    };
    if changed {
        let _ = app.emit(EV_VOICE, v);
    }
}

/// « Ne pas déranger » : Discord en `dnd`, ou retour au statut d'avant (le plugin le garde).
pub fn set_dnd(app: &AppHandle, on: bool) {
    let mut i = lock(app);
    i.dnd_wanted = Some(on);
    sync_status(app, &mut i);
}

pub fn voice_state(app: AppHandle) -> Voice {
    lock(&app).voice.clone()
}

/// En appel (palette) : micro coupé, son coupé, « Quitter le salon » possible.
pub fn call_status(app: &AppHandle) -> Option<(bool, bool, bool)> {
    let v = voice_state(app.clone());
    (v.available && v.in_call()).then(|| (v.mute.unwrap_or(false), v.deaf.unwrap_or(false), v.can("leave")))
}

/// Message `voice_set` qui inverse le micro (`mute`) ou le son (`deaf`) de l'état `v`.
fn toggle_message(v: &Voice, what: &str) -> Result<Value, String> {
    if !v.available || !v.in_call() {
        return Err("pas en vocal sur Discord".into());
    }
    let now = match what {
        "mute" => v.mute,
        "deaf" => v.deaf,
        _ => return Err(format!("commande inconnue : {what}")),
    };
    let mut msg = json!({ "type": "voice_set" });
    msg[what] = json!(!now.unwrap_or(false));
    Ok(msg)
}

/// Bascule le micro (`what` = "mute") ou le son (`what` = "deaf") dans Discord ; l'état
/// affiché ne change qu'au retour du plugin.
pub fn voice_toggle(app: AppHandle, what: String) -> Result<(), String> {
    crate::relay::send(&app, toggle_message(&voice_state(app.clone()), &what)?);
    Ok(())
}

/// Commande d'appel `msg`, si le plugin sait faire `cap` et qu'on est dans un salon.
fn call_message(v: &Voice, cap: &str, msg: Value) -> Result<Value, String> {
    if !v.in_call() {
        return Err("pas en vocal sur Discord".into());
    }
    if !v.can(cap) {
        return Err("commande non prise en charge par le plugin ou le relais Discord".into());
    }
    Ok(msg)
}

pub fn voice_leave(app: AppHandle) -> Result<(), String> {
    crate::relay::send(&app, call_message(&voice_state(app.clone()), "leave", json!({ "type": "voice_leave" }))?);
    Ok(())
}

/// Volume local d'un participant (0 à 200, 100 = normal, comme Discord).
pub fn voice_user_volume(app: AppHandle, id: String, volume: f64) -> Result<(), String> {
    let msg = json!({ "type": "user_volume", "id": id, "volume": volume.clamp(0.0, 200.0).round() });
    crate::relay::send(&app, call_message(&voice_state(app.clone()), "user_volume", msg)?);
    Ok(())
}

/// Sourdine locale d'un participant.
pub fn voice_user_mute(app: AppHandle, id: String, mute: bool) -> Result<(), String> {
    let msg = json!({ "type": "user_mute", "id": id, "mute": mute });
    crate::relay::send(&app, call_message(&voice_state(app.clone()), "user_volume", msg)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(j: Value) -> Voice {
        serde_json::from_value(j).unwrap()
    }

    fn bridged(caps: Value) -> Voice {
        voice(json!({ "type": "voice", "available": true, "channel": { "id": "1", "name": "Général" },
            "caps": caps, "bridged": true }))
    }

    #[test]
    fn plugin_message_round_trips() {
        let j = json!({ "type": "voice", "available": true, "channel": { "id": "1", "name": "Général" },
            "guild": { "id": "2", "name": "Les potes" }, "users": [{ "id": "a" }], "mute": false });
        assert_eq!(serde_json::to_value(voice(j.clone())).unwrap(), {
            let mut out = j;
            out["bridged"] = json!(false);
            out
        });
        assert_eq!(serde_json::to_value(Voice::unavailable()).unwrap(), json!({ "type": "voice", "available": false, "channel": null, "bridged": false }));
    }

    #[test]
    fn toggle_inverts_current_state() {
        let v = voice(json!({ "type": "voice", "available": true, "channel": { "id": "1", "name": "Général" }, "mute": true, "deaf": false }));
        assert_eq!(toggle_message(&v, "mute").unwrap(), json!({ "type": "voice_set", "mute": false }));
        assert_eq!(toggle_message(&v, "deaf").unwrap(), json!({ "type": "voice_set", "deaf": true }));
        assert!(toggle_message(&v, "kick").is_err());
    }

    #[test]
    fn toggle_refused_outside_a_call() {
        assert!(toggle_message(&voice(json!({ "available": false })), "mute").is_err());
        assert!(toggle_message(&voice(json!({ "available": true, "channel": null })), "mute").is_err());
    }

    #[test]
    fn new_commands_need_caps_and_a_forwarding_relay() {
        let leave = json!({ "type": "voice_leave" });
        assert!(call_message(&bridged(json!({ "leave": true })), "leave", leave.clone()).is_ok());
        assert!(call_message(&bridged(json!({ "leave": false })), "leave", leave.clone()).is_err());
        let mut old_relay = bridged(json!({ "leave": true }));
        old_relay.bridged = false;
        assert!(call_message(&old_relay, "leave", leave.clone()).is_err());
        let mut no_call = bridged(json!({ "leave": true }));
        no_call.channel = None;
        assert!(call_message(&no_call, "leave", leave).is_err());
    }

    #[test]
    fn usable_commands_are_computed_here() {
        let v = bridged(json!({ "leave": true, "status": false })).with_can();
        assert_eq!(serde_json::to_value(&v.can).unwrap(), json!({ "leave": true, "status": false }));
        let mut old_relay = bridged(json!({ "leave": true }));
        old_relay.bridged = false;
        assert_eq!(serde_json::to_value(old_relay.with_can().can).unwrap(), json!({ "leave": false }));
        let off = serde_json::to_value(voice(json!({ "available": false })).with_can()).unwrap();
        assert!(off.get("can").is_none());
    }

    #[test]
    fn hello_only_for_an_unbridged_plugin_with_caps() {
        let mut v = bridged(json!({ "status": true }));
        assert!(!v.needs_hello());
        v.bridged = false;
        assert!(v.needs_hello());
        // Plugin 1.2 (sans caps) ou Discord fermé : rien à demander.
        assert!(!voice(json!({ "available": true, "channel": null })).needs_hello());
        assert!(!voice(json!({ "available": false })).needs_hello());
    }

    #[test]
    fn dnd_status_is_sent_once_when_possible() {
        let v = bridged(json!({ "status": true }));
        let dnd = json!({ "type": "status_set", "status": "dnd" });
        assert_eq!(status_message(&v, Some(true), None), Some(dnd));
        assert_eq!(status_message(&v, Some(true), Some(true)), None);
        assert_eq!(
            status_message(&v, Some(false), Some(true)),
            Some(json!({ "type": "status_set", "status": "online" }))
        );
        // Jamais demandé, ou plugin sans la capacité : rien.
        assert_eq!(status_message(&v, None, None), None);
        assert_eq!(status_message(&bridged(json!({})), Some(true), None), None);
    }
}
