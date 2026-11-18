//! Client WebSocket local qui se reconnecte seul (délai croissant plafonné à 30 s), pour les
//! services maison (Prisme, relais Krisp). Un service absent, ou qui ferme la connexion
//! aussitôt ouverte, ne produit ni erreur visible ni boucle serrée. À la sortie de veille, la
//! connexion (peut-être morte sans le savoir) est refaite et l'attente écourtée.
//!
//! Repris tel quel de `app/src-tauri/src/wsclient.rs`.

use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::backoff::Backoff;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

/// Ce que le client signale à son propriétaire.
pub enum WsEvent<'a> {
    Connected,
    Message(&'a str),
    Disconnected,
}

#[derive(Clone)]
pub struct WsClient {
    tx: Sender<String>,
}

impl WsClient {
    /// Envoie un message texte (perdu si le service est absent).
    pub fn send(&self, msg: String) {
        let _ = self.tx.send(msg);
    }
}

fn session(
    ws: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    outbox: &Receiver<String>,
    on_event: &mut impl FnMut(WsEvent<'_>, &WsClient),
    me: &WsClient,
) -> anyhow::Result<()> {
    let epoch = crate::resume::epoch();
    loop {
        if crate::resume::epoch() != epoch {
            anyhow::bail!("sortie de veille");
        }
        match ws.read() {
            Ok(Message::Text(t)) => on_event(WsEvent::Message(t.as_str()), me),
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }
        while let Ok(m) = outbox.try_recv() {
            ws.send(Message::text(m))?;
        }
    }
}

/// Démarre le client sur un fil dédié.
pub fn spawn(
    name: &'static str,
    url: &'static str,
    mut on_event: impl FnMut(WsEvent<'_>, &WsClient) + Send + 'static,
) -> anyhow::Result<WsClient> {
    let (tx, rx) = unbounded::<String>();
    let me = WsClient { tx };
    let client = me.clone();
    let started = crate::util::spawn_named(&format!("ws-{name}"), move || {
        let mut backoff = Backoff::new();
        loop {
            match tungstenite::connect(url) {
                Ok((mut ws, _)) => {
                    let opened = Instant::now();
                    if let MaybeTlsStream::Plain(s) = ws.get_ref() {
                        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                    }
                    // Messages en attente d'avant la connexion : périmés.
                    while rx.try_recv().is_ok() {}
                    log::info!("{name} : connecté");
                    on_event(WsEvent::Connected, &me);
                    if let Err(e) = session(&mut ws, &rx, &mut on_event, &me) {
                        log::info!("{name} : connexion perdue ({e})");
                    }
                    on_event(WsEvent::Disconnected, &me);
                    backoff.connection_closed(opened.elapsed());
                }
                Err(e) => log::debug!("{name} absent : {e}"),
            }
            if crate::resume::sleep(backoff.next_delay()) {
                backoff.reset();
            }
        }
    });
    anyhow::ensure!(started, "client {name} : fil impossible à démarrer");
    Ok(client)
}
