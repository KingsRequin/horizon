//! Lien avec Prisme (`ws://127.0.0.1:28650`) :
//!  * la couleur de la pochette en cours est envoyée (`set_accent`), Prisme en teinte sa scène
//!    musique ; `null` quand rien ne joue ou à la fermeture ;
//!  * widget Éclairage : l'état poussé par Prisme (`{"event":"state"}`, au moins toutes les 2 s)
//!    est relayé aux barres ; scène forcée (`set_override`, `"off"` = éteint, `null` = règles
//!    automatiques) et luminosité (`set_brightness`) sont commandées depuis le widget ;
//!  * signal « Claude attend » ou fin du minuteur : une scène forcée un instant, puis la
//!    précédente ; « Ne pas déranger » : scène `focus` tenue pendant toute sa durée
//!    (voir `horizon_core::light_signal`).
//!
//! Repris de `app/src-tauri/src/prisme.rs` (`use crate::host` au lieu de `tauri`). Widget :
//! `ui::lighting`.

use crate::events::EV_LIGHTING;
use crate::util::{lock, spawn_named};
use crate::wsclient::{WsClient, WsEvent};
use horizon_core::accent::AccentSender;
use horizon_core::light_signal::{LightSignal, scene_name};
use std::collections::BTreeMap;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use crate::host::{AppHandle, Emitter, Manager};

const URL: &str = "ws://127.0.0.1:28650";
/// Scène de Prisme qui éteint les LED.
const OFF: &str = "off";

/// État de l'éclairage pour le widget ; `connected: false` = Prisme absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Lighting {
    pub connected: bool,
    /// Scène appliquée en ce moment.
    pub scene: String,
    /// Scène forcée par l'utilisateur ; `None` = règles automatiques.
    pub override_scene: Option<String>,
    pub brightness: f32,
    pub scenes: Vec<String>,
    /// Nom affiché de chaque scène (celles de la liste et la scène en cours).
    pub names: BTreeMap<String, String>,
}

impl Lighting {
    /// État tiré d'un événement `state` de Prisme.
    fn from_event(msg: &str) -> Option<Lighting> {
        let v: Value = serde_json::from_str(msg).ok()?;
        if v["event"] != "state" {
            return None;
        }
        let d = &v["data"]["daemon"];
        let scene = d["scene"].as_str().unwrap_or_default().to_owned();
        let scenes: Vec<String> = d["scenes"]
            .as_array()
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let names = scenes.iter().chain([&scene]).map(|s| (s.clone(), scene_name(s))).collect();
        Some(Lighting {
            connected: true,
            scene,
            override_scene: d["override_scene"].as_str().map(str::to_owned),
            brightness: d["brightness"].as_f64().unwrap_or(1.0) as f32,
            scenes,
            names,
        })
    }
}

struct Inner {
    /// Couleur voulue (dernière demandée par Horizon).
    wanted: Option<[u8; 3]>,
    sender: AccentSender,
    next_id: u64,
    lighting: Lighting,
    signal: LightSignal,
    /// Maintien de la scène `focus` (« Ne pas déranger ») : voulu, et numéro du maintien en
    /// place.
    hold: LightSignal,
    hold_wanted: bool,
    hold_on: Option<u64>,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            wanted: None,
            sender: AccentSender::default(),
            next_id: 0,
            lighting: Lighting::default(),
            signal: LightSignal::default(),
            hold: LightSignal::focus(),
            hold_wanted: false,
            hold_on: None,
        }
    }
}

/// Met le maintien de `focus` en accord avec ce qui est voulu ; renvoie la commande à envoyer.
/// Un signal en cours garde la main : c'est la scène qu'il rétablira qui change.
fn sync_hold(i: &mut Inner) -> Option<String> {
    if !i.lighting.connected {
        return None;
    }
    let pending = i.signal.pending();
    let effective = pending.clone().unwrap_or_else(|| i.lighting.override_scene.clone());
    let target = match (i.hold_wanted, i.hold_on) {
        (true, None) => {
            let (scene, generation) = i.hold.start(effective.as_deref(), &i.lighting.scenes)?;
            i.hold_on = Some(generation);
            Some(scene)
        }
        (false, Some(generation)) => {
            i.hold_on = None;
            i.hold.finish(generation, effective.as_deref())?
        }
        _ => return None,
    };
    if pending.is_some() {
        i.signal.set_pending(target);
        return None;
    }
    Some(request(i, "set_override", json!({ "scene": target })))
}

/// Lien avec Prisme ; le client est posé au démarrage (`start`).
#[derive(Default)]
pub struct Prisme {
    client: OnceLock<WsClient>,
    inner: Arc<Mutex<Inner>>,
}

fn request(inner: &mut Inner, cmd: &str, mut args: Value) -> String {
    inner.next_id += 1;
    args["id"] = json!(inner.next_id);
    args["cmd"] = json!(cmd);
    args.to_string()
}

/// Connexion à Prisme (se refait seule) ; l'état de l'éclairage part aux barres.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let p = app.state::<Prisme>();
    let (i, a) = (p.inner.clone(), app.clone());
    let client = crate::wsclient::spawn("prisme", URL, move |ev, client| match ev {
        WsEvent::Connected => {
            // Prisme vient de (re)démarrer : on lui redonne la couleur courante.
            let mut g = lock(&i);
            g.sender.reset();
            let wanted = g.wanted;
            if let Some(v) = g.sender.next(wanted) {
                let m = request(&mut g, "set_accent", json!({ "rgb": v }));
                client.send(m);
            }
        }
        WsEvent::Message(m) => {
            if let Some(l) = Lighting::from_event(m) {
                let mut g = lock(&i);
                if g.lighting != l {
                    g.lighting = l.clone();
                    if let Some(m) = sync_hold(&mut g) {
                        client.send(m);
                    }
                    drop(g);
                    let _ = a.emit(EV_LIGHTING, l);
                }
            } else if m.contains("\"ok\":false") {
                log::warn!("prisme : {m}");
            }
        }
        WsEvent::Disconnected => {
            let mut g = lock(&i);
            // Prisme relancé : le maintien sera remis à la reconnexion.
            g.hold = LightSignal::focus();
            g.hold_on = None;
            if g.lighting.connected {
                g.lighting = Lighting::default();
                drop(g);
                let _ = a.emit(EV_LIGHTING, Lighting::default());
            }
        }
    })?;
    p.client.set(client).map_err(|_| anyhow::anyhow!("Prisme déjà démarré"))
}

impl Prisme {
    /// Envoie un message (perdu avant le démarrage ou hors connexion).
    fn send(&self, m: String) {
        if let Some(c) = self.client.get() {
            c.send(m);
        }
    }

    /// Demande une couleur (`None` = palette d'origine) ; n'envoie que les changements.
    /// Hors connexion, la couleur est redonnée à la reconnexion.
    pub fn set_accent(&self, rgb: Option<[u8; 3]>) {
        let mut g = lock(&self.inner);
        g.wanted = rgb;
        if let Some(v) = g.sender.next(rgb) {
            let m = request(&mut g, "set_accent", json!({ "rgb": v }));
            self.send(m);
        }
    }

    fn command(&self, cmd: &str, args: Value) -> Result<(), String> {
        let mut g = lock(&self.inner);
        if !g.lighting.connected {
            return Err("Prisme n’est pas lancé".into());
        }
        let m = request(&mut g, cmd, args);
        self.send(m);
        Ok(())
    }

    pub fn lighting(&self) -> Lighting {
        lock(&self.inner).lighting.clone()
    }

    /// Début d'un signal : scène forcée envoyée ; renvoie le numéro du signal.
    fn signal_start(&self) -> Option<u64> {
        let mut g = lock(&self.inner);
        if !g.lighting.connected {
            return None;
        }
        let i = &mut *g;
        let (scene, generation) = i.signal.start(i.lighting.override_scene.as_deref(), &i.lighting.scenes)?;
        let m = request(&mut g, "set_override", json!({ "scene": scene }));
        self.send(m);
        Some(generation)
    }

    /// Fin du signal `generation` : la scène d'avant revient, sauf si l'utilisateur en a
    /// choisi une autre entre-temps.
    fn signal_finish(&self, generation: u64) {
        let mut g = lock(&self.inner);
        let i = &mut *g;
        if let Some(previous) = i.signal.finish(generation, i.lighting.override_scene.as_deref()) {
            let m = request(&mut g, "set_override", json!({ "scene": previous }));
            self.send(m);
        }
    }

    /// Tient (ou rend) la scène `focus`, maintenant ou dès que Prisme répond.
    pub fn set_focus_hold(&self, on: bool) {
        let mut g = lock(&self.inner);
        g.hold_wanted = on;
        if let Some(m) = sync_hold(&mut g) {
            self.send(m);
        }
    }

    /// Fermeture d'Horizon pendant un signal : la scène d'avant revient tout de suite.
    pub fn end_signal(&self) {
        let generation = lock(&self.inner).signal.generation();
        self.signal_finish(generation);
    }
}

/// Bref signal lumineux (scène `claude`, sinon extinction) pendant `duration`.
pub fn signal(app: &AppHandle, duration: Duration) {
    let Some(generation) = app.state::<Prisme>().signal_start() else { return };
    let app = app.clone();
    spawn_named("prisme-signal", move || {
        std::thread::sleep(duration);
        app.state::<Prisme>().signal_finish(generation);
    });
}

pub fn lighting_state(app: AppHandle) -> Lighting {
    app.state::<Prisme>().lighting()
}

/// Force une scène ; `None` = retour aux règles automatiques.
pub fn lighting_set_scene(app: AppHandle, scene: Option<String>) -> Result<(), String> {
    app.state::<Prisme>().command("set_override", json!({ "scene": scene }))
}

/// Éteint (scène `off`) ou rallume (règles automatiques).
pub fn lighting_toggle(app: AppHandle) -> Result<(), String> {
    let p = app.state::<Prisme>();
    let off = p.lighting().override_scene.as_deref() == Some(OFF);
    p.command("set_override", json!({ "scene": if off { None } else { Some(OFF) } }))
}

/// Luminosité de 0 à 1 (Prisme l'enregistre dans sa config : à n'envoyer qu'au relâchement).
pub fn lighting_set_brightness(app: AppHandle, value: f32) -> Result<(), String> {
    app.state::<Prisme>().command("set_brightness", json!({ "value": value.clamp(0.0, 1.0) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prisme_state_event() {
        let m = r#"{"event":"state","data":{"daemon":{"scene":"music","effect":"spectrum",
            "override_scene":null,"brightness":0.8,"scenes":["idle","music","off","video"]},"devices":[]}}"#;
        let l = Lighting::from_event(m).unwrap();
        assert!(l.connected);
        assert_eq!(l.scene, "music");
        assert_eq!(l.override_scene, None);
        assert!((l.brightness - 0.8).abs() < 1e-6);
        assert_eq!(l.scenes, ["idle", "music", "off", "video"]);
        assert_eq!(l.names["music"], "Musique");
        let forced = m.replace(r#""override_scene":null"#, r#""override_scene":"off""#);
        assert_eq!(Lighting::from_event(&forced).unwrap().override_scene.as_deref(), Some("off"));
    }

    #[test]
    fn ignores_replies_and_other_events() {
        assert_eq!(Lighting::from_event(r#"{"id":3,"ok":true,"data":null}"#), None);
        assert_eq!(Lighting::from_event(r#"{"event":"preview","data":[]}"#), None);
        assert_eq!(Lighting::from_event("pas du json"), None);
    }

    fn connected(scenes: &[&str], current: Option<&str>) -> Inner {
        Inner {
            lighting: Lighting {
                connected: true,
                scene: "music".into(),
                override_scene: current.map(str::to_owned),
                brightness: 1.0,
                scenes: scenes.iter().map(|s| s.to_string()).collect(),
                names: BTreeMap::new(),
            },
            ..Inner::default()
        }
    }

    fn scene_of(m: Option<String>) -> Value {
        serde_json::from_str::<Value>(&m.expect("commande attendue")).unwrap()["scene"].clone()
    }

    #[test]
    fn focus_hold_starts_and_restores() {
        let mut i = connected(&["music", "focus", "claude"], Some("music"));
        i.hold_wanted = true;
        assert_eq!(scene_of(sync_hold(&mut i)), json!("focus"));
        // Déjà en place : rien de plus.
        i.lighting.override_scene = Some("focus".into());
        assert_eq!(sync_hold(&mut i), None);
        i.hold_wanted = false;
        assert_eq!(scene_of(sync_hold(&mut i)), json!("music"));
    }

    #[test]
    fn focus_hold_waits_for_prisme_and_needs_the_scene() {
        let mut i = Inner { hold_wanted: true, ..Inner::default() };
        assert_eq!(sync_hold(&mut i), None);
        let mut i = connected(&["music"], None);
        i.hold_wanted = true;
        assert_eq!(sync_hold(&mut i), None);
        assert_eq!(i.hold_on, None);
    }

    #[test]
    fn focus_hold_during_a_signal_changes_what_the_signal_restores() {
        let mut i = connected(&["music", "focus", "claude"], None);
        let (_, g) = i.signal.start(None, &i.lighting.scenes.clone()).unwrap();
        i.lighting.override_scene = Some("claude".into());
        i.hold_wanted = true;
        // Pas d'envoi : le signal rétablira « focus ».
        assert_eq!(sync_hold(&mut i), None);
        assert_eq!(i.signal.pending(), Some(Some("focus".into())));
        assert_eq!(i.signal.finish(g, Some("claude")), Some(Some("focus".into())));
    }

    #[test]
    fn requests_carry_id_and_command() {
        let mut i = Inner::default();
        let m: Value = serde_json::from_str(&request(&mut i, "set_override", json!({ "scene": null }))).unwrap();
        assert_eq!(m, json!({ "id": 1, "cmd": "set_override", "scene": null }));
    }
}
