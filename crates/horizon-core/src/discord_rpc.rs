//! RPC local de Discord (logique pure) : messages envoyés, salon vocal suivi, état présenté au
//! widget Discord vocal (même forme que le message `voice` du relais d'avant), et traduction
//! des commandes du widget en commandes RPC.
//!
//! Le client (connexion, autorisation) est dans `natif/src/discord.rs`. Par défaut, Horizon se
//! présente comme StreamKit (l'appli de Discord faite pour les overlays) : identifiant et
//! origine de StreamKit, jeton obtenu par l'échange de StreamKit.

use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Application StreamKit de Discord.
pub const STREAMKIT_ID: &str = "207646673902501888";
pub const STREAMKIT_ORIGIN: &str = "https://streamkit.discord.com";
/// Échange du code d'autorisation contre un jeton (StreamKit).
pub const STREAMKIT_TOKEN_URL: &str = "https://streamkit.discord.com/overlay/token";
/// Ports essayés du RPC local de Discord.
pub const PORTS: std::ops::RangeInclusive<u16> = 6463..=6472;
/// Droits demandés à l'autorisation.
pub const SCOPES: [&str; 1] = ["rpc"];

/// Adresse du RPC sur `port` pour l'application `client_id`.
pub fn url(port: u16, client_id: &str) -> String {
    format!("ws://127.0.0.1:{port}/?v=1&client_id={client_id}&encoding=json")
}

/// Commande RPC.
pub fn command(cmd: &str, args: Value, nonce: u64) -> Value {
    json!({ "cmd": cmd, "args": args, "nonce": nonce.to_string() })
}

/// Abonnement à un événement.
pub fn subscribe(evt: &str, args: Value, nonce: u64, on: bool) -> Value {
    json!({ "cmd": if on { "SUBSCRIBE" } else { "UNSUBSCRIBE" }, "evt": evt, "args": args, "nonce": nonce.to_string() })
}

/// Événements suivis pour un salon vocal.
pub const CHANNEL_EVENTS: [&str; 5] = ["VOICE_STATE_CREATE", "VOICE_STATE_UPDATE", "VOICE_STATE_DELETE", "SPEAKING_START", "SPEAKING_STOP"];

#[derive(Debug, Clone, PartialEq)]
struct User {
    id: String,
    name: String,
    avatar: Option<String>,
    /// Micro et son coupés (par la personne elle-même ou par le serveur).
    mute: bool,
    deaf: bool,
    /// Réglages locaux (pour moi seulement) : volume 0 à 200, sourdine.
    volume: f64,
    local_mute: bool,
}

/// Salon vocal en cours et réglages, tels que suivis par le RPC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceModel {
    /// Connecté et authentifié.
    pub ready: bool,
    /// Les commandes (micro, son, volumes, quitter) sont permises à cette application.
    pub write: bool,
    /// Moi (identifiant).
    pub me: Option<String>,
    channel: Option<(String, String)>,
    guild: Option<(String, String)>,
    users: Vec<User>,
    speaking: BTreeSet<String>,
    pub mute: Option<bool>,
    pub deaf: Option<bool>,
    /// Suppression de bruit de Discord (Krisp).
    pub noise: Option<bool>,
}

fn user_of(v: &Value) -> Option<User> {
    let u = &v["user"];
    let id = u["id"].as_str()?.to_owned();
    let name = v["nick"]
        .as_str()
        .or_else(|| u["global_name"].as_str())
        .or_else(|| u["username"].as_str())
        .unwrap_or("?")
        .to_owned();
    let avatar = u["avatar"].as_str().map(|h| format!("https://cdn.discordapp.com/avatars/{id}/{h}.png?size=64"));
    let s = &v["voice_state"];
    let flag = |k: &str| s[k].as_bool().unwrap_or(false);
    Some(User {
        id,
        name,
        avatar,
        mute: flag("mute") || flag("self_mute"),
        deaf: flag("deaf") || flag("self_deaf"),
        volume: v["volume"].as_f64().unwrap_or(100.0),
        local_mute: v["mute"].as_bool().unwrap_or(false),
    })
}

impl VoiceModel {
    /// Identifiant du salon suivi.
    pub fn channel_id(&self) -> Option<&str> {
        self.channel.as_ref().map(|c| c.0.as_str())
    }

    /// Serveur du salon (pour demander son nom).
    pub fn guild_id(&self) -> Option<&str> {
        self.guild.as_ref().map(|g| g.0.as_str())
    }

    /// Réponse de `GET_SELECTED_VOICE_CHANNEL` (ou `GET_CHANNEL`) : `null` hors appel.
    pub fn set_channel(&mut self, data: &Value) {
        self.speaking.clear();
        let (Some(id), Some(name)) = (data["id"].as_str(), data["name"].as_str()) else {
            self.channel = None;
            self.guild = None;
            self.users.clear();
            return;
        };
        self.channel = Some((id.to_owned(), name.to_owned()));
        self.guild = data["guild_id"].as_str().map(|g| (g.to_owned(), String::new()));
        self.users = data["voice_states"].as_array().map(|a| a.iter().filter_map(user_of).collect()).unwrap_or_default();
    }

    /// Nom du serveur (réponse de `GET_GUILD`).
    pub fn set_guild_name(&mut self, id: &str, name: &str) {
        if let Some(g) = self.guild.as_mut().filter(|g| g.0 == id) {
            g.1 = name.to_owned();
        }
    }

    /// Événement `VOICE_STATE_CREATE`, `_UPDATE` ou `_DELETE`.
    pub fn voice_state(&mut self, evt: &str, data: &Value) {
        let Some(u) = user_of(data) else { return };
        let at = self.users.iter().position(|x| x.id == u.id);
        match (evt, at) {
            ("VOICE_STATE_DELETE", Some(i)) => {
                self.speaking.remove(&u.id);
                self.users.remove(i);
            }
            ("VOICE_STATE_DELETE", None) => {}
            (_, Some(i)) => self.users[i] = u,
            (_, None) => self.users.push(u),
        }
    }

    /// `SPEAKING_START` / `SPEAKING_STOP`.
    pub fn speaking(&mut self, user_id: &str, on: bool) {
        if on {
            self.speaking.insert(user_id.to_owned());
        } else {
            self.speaking.remove(user_id);
        }
    }

    /// `GET_VOICE_SETTINGS` ou `VOICE_SETTINGS_UPDATE`.
    pub fn settings(&mut self, data: &Value) {
        if let Some(m) = data["mute"].as_bool() {
            self.mute = Some(m);
        }
        if let Some(d) = data["deaf"].as_bool() {
            self.deaf = Some(d);
        }
        if let Some(n) = data["noise_suppression"].as_bool() {
            self.noise = Some(n);
        }
    }

    /// Discord fermé ou connexion perdue.
    pub fn reset(&mut self) {
        *self = VoiceModel::default();
    }

    /// État pour le widget (forme du message `voice` du relais d'avant).
    pub fn to_voice(&self) -> Value {
        let available = self.ready;
        let channel = self.channel.as_ref().map(|(id, name)| json!({ "id": id, "name": name }));
        let users: Vec<Value> = self
            .users
            .iter()
            .map(|u| {
                json!({
                    "id": u.id,
                    "name": u.name,
                    "avatar": u.avatar,
                    "speaking": self.speaking.contains(&u.id),
                    "mute": u.mute,
                    "deaf": u.deaf,
                    "volume": u.volume,
                    "local_mute": u.local_mute,
                    "self": self.me.as_deref() == Some(u.id.as_str()),
                })
            })
            .collect();
        let mut v = json!({
            "type": "voice",
            "available": available,
            "channel": channel,
            "users": users,
            "caps": { "leave": self.write, "user_volume": self.write, "voice": self.write, "status": false },
            "bridged": true,
            "source": "rpc",
        });
        if let Some(m) = self.mute {
            v["mute"] = json!(m);
        }
        if let Some(d) = self.deaf {
            v["deaf"] = json!(d);
        }
        if let Some((id, name)) = &self.guild {
            v["guild"] = json!({ "id": id, "name": name });
        }
        v
    }
}

/// Commande RPC (`cmd`, `args`) pour un message du widget (format du relais d'avant) ; `None` :
/// pas d'équivalent (statut « Ne pas déranger »).
pub fn translate(msg: &Value) -> Option<(&'static str, Value)> {
    match msg["type"].as_str()? {
        "voice_set" => {
            let mut args = json!({});
            for k in ["mute", "deaf"] {
                if let Some(b) = msg[k].as_bool() {
                    args[k] = json!(b);
                }
            }
            Some(("SET_VOICE_SETTINGS", args))
        }
        "voice_leave" => Some(("SELECT_VOICE_CHANNEL", json!({ "channel_id": null, "force": true }))),
        "user_volume" => Some(("SET_USER_VOICE_SETTINGS", json!({ "user_id": msg["id"], "volume": msg["volume"] }))),
        "user_mute" => Some(("SET_USER_VOICE_SETTINGS", json!({ "user_id": msg["id"], "mute": msg["mute"] }))),
        // Krisp : la suppression de bruit de Discord.
        "set" => msg["krisp"].as_bool().map(|k| ("SET_VOICE_SETTINGS", json!({ "noise_suppression": k }))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: &str, name: &str, self_mute: bool) -> Value {
        json!({ "nick": null, "mute": false, "volume": 100,
            "voice_state": { "mute": false, "deaf": false, "self_mute": self_mute, "self_deaf": false },
            "user": { "id": id, "username": name, "global_name": null, "avatar": "abc" } })
    }

    #[test]
    fn channel_users_and_speaking() {
        let mut m = VoiceModel { ready: true, me: Some("1".into()), ..VoiceModel::default() };
        m.set_channel(&json!({ "id": "c1", "name": "Général", "guild_id": "g1", "voice_states": [state("1", "moi", false), state("2", "léo", true)] }));
        m.set_guild_name("g1", "Les potes");
        m.speaking("2", true);
        m.settings(&json!({ "mute": false, "deaf": false, "noise_suppression": true }));
        let v = m.to_voice();
        assert_eq!(v["channel"], json!({ "id": "c1", "name": "Général" }));
        assert_eq!(v["guild"]["name"], "Les potes");
        assert_eq!(v["users"][0]["self"], true);
        assert_eq!(v["users"][1]["speaking"], true);
        assert_eq!(v["users"][1]["mute"], true);
        assert_eq!(v["users"][1]["avatar"], "https://cdn.discordapp.com/avatars/2/abc.png?size=64");
        assert_eq!(m.noise, Some(true));
        m.voice_state("VOICE_STATE_DELETE", &state("2", "léo", true));
        assert_eq!(m.to_voice()["users"].as_array().unwrap().len(), 1);
        m.voice_state("VOICE_STATE_CREATE", &state("3", "sam", false));
        assert_eq!(m.to_voice()["users"][1]["name"], "sam");
    }

    #[test]
    fn leaving_the_channel() {
        let mut m = VoiceModel { ready: true, ..VoiceModel::default() };
        m.set_channel(&json!({ "id": "c1", "name": "Général", "voice_states": [state("1", "moi", false)] }));
        m.set_channel(&Value::Null);
        let v = m.to_voice();
        assert_eq!(v["channel"], Value::Null);
        assert_eq!(v["users"], json!([]));
        assert_eq!(v["available"], true);
    }

    #[test]
    fn read_only_without_write() {
        let v = VoiceModel { ready: true, write: false, ..VoiceModel::default() }.to_voice();
        assert_eq!(v["caps"]["leave"], false);
        assert_eq!(v["caps"]["status"], false);
        assert_eq!(VoiceModel::default().to_voice()["available"], false);
    }

    #[test]
    fn widget_commands_become_rpc_commands() {
        assert_eq!(translate(&json!({ "type": "voice_set", "mute": true })), Some(("SET_VOICE_SETTINGS", json!({ "mute": true }))));
        assert_eq!(translate(&json!({ "type": "voice_leave" })).unwrap().0, "SELECT_VOICE_CHANNEL");
        assert_eq!(
            translate(&json!({ "type": "user_volume", "id": "2", "volume": 150 })),
            Some(("SET_USER_VOICE_SETTINGS", json!({ "user_id": "2", "volume": 150 })))
        );
        assert_eq!(translate(&json!({ "type": "set", "krisp": false })), Some(("SET_VOICE_SETTINGS", json!({ "noise_suppression": false }))));
        assert_eq!(translate(&json!({ "type": "status_set", "status": "dnd" })), None);
    }

    #[test]
    fn messages() {
        assert_eq!(url(6463, STREAMKIT_ID), "ws://127.0.0.1:6463/?v=1&client_id=207646673902501888&encoding=json");
        assert_eq!(command("AUTHENTICATE", json!({ "access_token": "t" }), 3)["nonce"], "3");
        assert_eq!(subscribe("SPEAKING_START", json!({ "channel_id": "c" }), 4, false)["cmd"], "UNSUBSCRIBE");
    }
}
