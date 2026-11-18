//! Connexion WebSocket de la fenêtre au moteur (`ws://127.0.0.1:28650`), le temps que la fenêtre
//! est ouverte, comme `daemon.svelte.ts` : reconnexion toutes les 1,5 s, messages lus et
//! décodés sur un fil à part, remis au fil de l'interface (`deliver`). Le fil s'arrête quand le
//! `Link` est lâché (fenêtre fermée) : la connexion se ferme, le moteur coupe sa
//! prévisualisation.

use super::types::{DaemonState, DeviceStatus, Preview};
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const URL: &str = "ws://127.0.0.1:28650";
/// Attente avant de retenter (celle de `daemon.svelte.ts`).
const RETRY: Duration = Duration::from_millis(1500);

/// Ce que le fil remet à l'interface.
#[derive(Debug)]
pub enum Incoming {
    Connected,
    Disconnected,
    State(Box<DaemonState>, Vec<DeviceStatus>),
    Preview(Box<Preview>),
    Reply { id: u64, result: Result<Value, String> },
}

/// Décode un message du moteur (`None` : inconnu ou illisible).
pub fn parse(text: &str) -> Option<Incoming> {
    let mut v: Value = serde_json::from_str(text).ok()?;
    match v.get("event").and_then(Value::as_str) {
        Some("state") => {
            let data = v.get_mut("data")?;
            let daemon = serde_json::from_value(data.get_mut("daemon")?.take()).ok()?;
            let devices = data.get_mut("devices").map(Value::take).and_then(|d| serde_json::from_value(d).ok());
            Some(Incoming::State(Box::new(daemon), devices.unwrap_or_default()))
        }
        Some("preview") => serde_json::from_value(v.get_mut("data")?.take()).ok().map(|p| Incoming::Preview(Box::new(p))),
        Some(_) => None,
        None => {
            let id = v.get("id")?.as_u64()?;
            let result = if v.get("ok").and_then(Value::as_bool) == Some(true) {
                Ok(v.get_mut("data").map(Value::take).unwrap_or(Value::Null))
            } else {
                Err(v.get("error").and_then(Value::as_str).unwrap_or("erreur du moteur").to_owned())
            };
            Some(Incoming::Reply { id, result })
        }
    }
}

/// Lien en cours ; lâché, son fil s'arrête.
pub struct Link {
    tx: Sender<String>,
    stop: Arc<AtomicBool>,
}

impl Link {
    /// Démarre le fil ; `deliver` est appelé sur ce fil (à lui de passer au fil de l'interface).
    pub fn spawn(deliver: impl Fn(Incoming) + Send + 'static) -> Link {
        let (tx, rx) = unbounded::<String>();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let started = crate::util::spawn_named("prisme-fenetre", move || run(&rx, &flag, &deliver));
        if !started {
            log::error!("fenêtre Prisme : fil de connexion impossible à démarrer");
        }
        Link { tx, stop }
    }

    /// Envoie un message texte (perdu hors connexion).
    pub fn send(&self, msg: String) {
        let _ = self.tx.send(msg);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(outbox: &Receiver<String>, stop: &AtomicBool, deliver: &dyn Fn(Incoming)) {
    while !stop.load(Ordering::Relaxed) {
        if let Ok((mut ws, _)) = tungstenite::connect(URL) {
            if let MaybeTlsStream::Plain(s) = ws.get_ref() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
            }
            // Messages d'avant la connexion : périmés.
            while outbox.try_recv().is_ok() {}
            deliver(Incoming::Connected);
            if let Err(e) = session(&mut ws, outbox, stop, deliver) {
                log::debug!("fenêtre Prisme : connexion perdue ({e})");
            }
            let _ = ws.close(None);
            let _ = ws.flush();
            deliver(Incoming::Disconnected);
        }
        // Attente découpée : la fermeture de la fenêtre arrête le fil sans délai.
        let mut waited = Duration::ZERO;
        while waited < RETRY && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            waited += Duration::from_millis(100);
        }
    }
}

fn session(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    outbox: &Receiver<String>,
    stop: &AtomicBool,
    deliver: &dyn Fn(Incoming),
) -> anyhow::Result<()> {
    while !stop.load(Ordering::Relaxed) {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Some(m) = parse(t.as_str()) {
                    deliver(m);
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e.into()),
        }
        while let Ok(m) = outbox.try_recv() {
            ws.send(Message::text(m))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_state_preview_and_replies() {
        let m = r#"{"event":"state","data":{"daemon":{"scene":"music","override_scene":null,"brightness":0.8,
            "scenes":["idle","music"],"screens":[{"name":"\\\\.\\DISPLAY1","x":0,"y":0,"w":1,"h":0.5625,"primary":true}],
            "config_rev":4},"devices":[{"id":"serial-1","label":"","name":"Bande","driver":"serial","enabled":true,
            "connected":true,"error":null,"leds":60,"zones":[["Bande",60]],"fps":59.6}]}}"#;
        let Some(Incoming::State(s, d)) = parse(m) else { panic!("état attendu") };
        assert_eq!((s.scene.as_str(), s.config_rev, s.screens.len()), ("music", 4, 1));
        assert_eq!(d[0].zones, [("Bande".to_owned(), 60)]);
        let p = r#"{"event":"preview","data":{"positions":[[0.1,0.2]],"screens":[],"active_screen":null,
            "colors":[[255,0,10]],"devices":[["serial-1",0,1]]}}"#;
        let Some(Incoming::Preview(p)) = parse(p) else { panic!("aperçu attendu") };
        assert_eq!((p.colors[0], p.devices[0].2), ([255, 0, 10], 1));
        assert!(matches!(parse(r#"{"id":3,"ok":true,"data":{"rev":9}}"#), Some(Incoming::Reply { id: 3, result: Ok(_) })));
        let Some(Incoming::Reply { result: Err(e), .. }) = parse(r#"{"id":4,"ok":false,"error":"config invalide"}"#)
        else {
            panic!("erreur attendue")
        };
        assert_eq!(e, "config invalide");
        assert!(parse(r#"{"id":null,"ok":true,"data":null}"#).is_none());
        assert!(parse("pas du json").is_none());
    }
}
