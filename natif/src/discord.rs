//! Discord vocal par le RPC local de Discord (`ws://127.0.0.1:6463…6472`). Par défaut, Horizon
//! se présente comme StreamKit (l'application de Discord pour les overlays : origine et
//! identifiant de StreamKit, jeton par son échange) ; `[discord] client_id` et `client_secret`
//! de `settings.toml` font utiliser ta propre application.
//!
//! Discord demande une fois l'autorisation (fenêtre dans Discord) ; le jeton est gardé dans le
//! Gestionnaire d'identifiants de Windows. Ensuite : salon en cours, participants, qui parle,
//! micro et son ; commandes (micro, son, quitter, volume d'un participant, Krisp par la
//! suppression de bruit de Discord) si l'application y a droit, sinon lecture seule.
//!
//! L'autorisation n'est demandée que si un widget Discord vocal ou Micro est sur une barre.
//! Tant que le RPC n'est pas actif, le relais BetterDiscord d'avant (édition personnelle) peut
//! encore alimenter le widget (`relay.rs`).

use crate::host::{AppHandle, Manager};
use crate::util::lock;
use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::discord_rpc::{self as rpc, VoiceModel};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

/// Délai d'ouverture d'un port, et attente entre deux tours de ports.
const CONNECT: Duration = Duration::from_millis(300);
const RETRY: Duration = Duration::from_secs(10);
/// Délai pour que l'utilisateur réponde à la demande d'autorisation dans Discord.
const AUTHORIZE_WAIT: Duration = Duration::from_secs(120);

/// État du RPC (géré par l'application).
#[derive(Default)]
pub struct Discord {
    tx: Mutex<Option<Sender<Value>>>,
    active: AtomicBool,
    /// Ce que montre la page État.
    status: Mutex<String>,
}

impl Discord {
    /// Le RPC alimente le widget (connecté et authentifié).
    pub fn active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> String {
        lock(&self.status).clone()
    }

    fn set_status(&self, s: impl Into<String>) {
        *lock(&self.status) = s.into();
    }
}

/// Commande du widget (format du relais d'avant) : par le RPC s'il est actif, sinon par le
/// relais (édition personnelle).
pub fn send(app: &AppHandle, msg: Value) {
    let d = app.state::<Discord>();
    if d.active() {
        if let Some(tx) = lock(&d.tx).as_ref() {
            let _ = tx.send(msg);
        }
    } else {
        crate::relay::send(app, msg);
    }
}

/// Un widget qui se sert de Discord est sur une barre.
fn wanted(app: &AppHandle) -> bool {
    app.state::<crate::state::AppState>()
        .read(|l| l.widgets().any(|w| matches!(w.kind.as_str(), "discord_voice" | "mic_krisp")))
}

/// Application utilisée : (identifiant, origine, secret de ta propre application).
fn app_identity() -> (String, String, Option<String>) {
    let s = crate::state::settings();
    match s.discord.own_app() {
        Some((id, secret)) => (id.to_owned(), "http://localhost".to_owned(), Some(secret.to_owned())),
        None => (rpc::STREAMKIT_ID.to_owned(), rpc::STREAMKIT_ORIGIN.to_owned(), None),
    }
}

fn token_target(client_id: &str) -> String {
    format!("Horizon/discord/{client_id}")
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    crate::util::start_thread("discord-rpc", move || {
        // Refus d'autorisation : plus redemandé avant le prochain démarrage.
        let mut refused = false;
        loop {
            if wanted(&a) {
                let (client_id, origin, secret) = app_identity();
                match connect(&client_id, &origin) {
                    Some(ws) => {
                        let (tx, rx) = unbounded();
                        *lock(&a.state::<Discord>().tx) = Some(tx);
                        let r = Session { app: &a, ws, rx, client_id, secret, nonce: 0, pending: HashMap::new(), model: VoiceModel::default() }
                            .run(&mut refused);
                        if let Err(e) = r {
                            log::info!("Discord RPC : {e:#}");
                        }
                        lost(&a);
                    }
                    None => a.state::<Discord>().set_status("Discord fermé"),
                }
            } else {
                a.state::<Discord>().set_status("aucun widget Discord sur les barres");
            }
            crate::resume::sleep(RETRY);
        }
    })
}

/// Connexion perdue : le widget redevient « Discord indisponible » (s'il était alimenté ici).
fn lost(app: &AppHandle) {
    let d = app.state::<Discord>();
    *lock(&d.tx) = None;
    if d.active.swap(false, Ordering::SeqCst) {
        crate::widgets::discord_voice::on_voice(app, json!({ "type": "voice", "available": false }));
        crate::widgets::mic_krisp::on_krisp(app, None);
    }
}

/// Ouvre le RPC sur le premier port qui répond.
fn connect(client_id: &str, origin: &str) -> Option<WebSocket<TcpStream>> {
    for port in rpc::PORTS {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let Ok(stream) = TcpStream::connect_timeout(&addr, CONNECT) else { continue };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let Ok(mut req) = rpc::url(port, client_id).into_client_request() else { continue };
        if let Ok(o) = origin.parse() {
            req.headers_mut().insert("Origin", o);
        }
        if let Ok((ws, _)) = tungstenite::client(req, stream) {
            let _ = ws.get_ref().set_read_timeout(Some(Duration::from_millis(100)));
            return Some(ws);
        }
    }
    None
}

/// Requête en attente de sa réponse.
#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Authorize,
    Authenticate,
    Channel,
    Guild,
    Settings,
    /// Commande du widget (une réponse d'erreur : pas le droit, lecture seule).
    Command,
    Other,
}

struct Session<'a> {
    app: &'a AppHandle,
    ws: WebSocket<TcpStream>,
    rx: Receiver<Value>,
    client_id: String,
    secret: Option<String>,
    nonce: u64,
    pending: HashMap<String, Pending>,
    model: VoiceModel,
}

impl Session<'_> {
    fn send(&mut self, msg: Value, kind: Pending) -> anyhow::Result<()> {
        if let Some(n) = msg["nonce"].as_str() {
            self.pending.insert(n.to_owned(), kind);
        }
        self.ws.send(Message::text(msg.to_string()))?;
        Ok(())
    }

    fn command(&mut self, cmd: &str, args: Value, kind: Pending) -> anyhow::Result<()> {
        self.nonce += 1;
        self.send(rpc::command(cmd, args, self.nonce), kind)
    }

    fn subscribe(&mut self, evt: &str, args: Value, on: bool) -> anyhow::Result<()> {
        self.nonce += 1;
        self.send(rpc::subscribe(evt, args, self.nonce, on), Pending::Other)
    }

    fn publish(&self) {
        crate::widgets::discord_voice::on_voice(self.app, self.model.to_voice());
        crate::widgets::mic_krisp::on_krisp(self.app, self.model.noise);
    }

    /// Suivi d'un autre salon (ou d'aucun).
    fn follow(&mut self, old: Option<String>, new: Option<String>) -> anyhow::Result<()> {
        if old == new {
            return Ok(());
        }
        if let Some(c) = old {
            for evt in rpc::CHANNEL_EVENTS {
                self.subscribe(evt, json!({ "channel_id": c }), false)?;
            }
        }
        if let Some(c) = new {
            for evt in rpc::CHANNEL_EVENTS {
                self.subscribe(evt, json!({ "channel_id": c }), true)?;
            }
        }
        Ok(())
    }

    fn run(mut self, refused: &mut bool) -> anyhow::Result<()> {
        let d = self.app.state::<Discord>();
        d.set_status("connexion…");
        let mut asked_at: Option<Instant> = None;
        loop {
            while let Ok(m) = self.rx.try_recv() {
                if let Some((cmd, args)) = rpc::translate(&m) {
                    self.command(cmd, args, Pending::Command)?;
                }
            }
            if asked_at.is_some_and(|t| t.elapsed() > AUTHORIZE_WAIT) {
                anyhow::bail!("autorisation sans réponse");
            }
            let text = match self.ws.read() {
                Ok(Message::Text(t)) => t.to_string(),
                Ok(Message::Close(_)) => anyhow::bail!("fermé par Discord"),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(e) => return Err(e.into()),
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
            let kind = v["nonce"].as_str().and_then(|n| self.pending.remove(n));
            let (cmd, evt, data) = (v["cmd"].as_str().unwrap_or_default(), v["evt"].as_str(), &v["data"]);
            if evt == Some("ERROR") {
                let message = data["message"].as_str().unwrap_or_default().to_owned();
                match kind {
                    Some(Pending::Authenticate) => {
                        // Jeton périmé ou retiré : on redemande.
                        horizon_sys::credentials::delete(&token_target(&self.client_id));
                        anyhow::bail!("jeton refusé ({message})");
                    }
                    Some(Pending::Authorize) => {
                        *refused = true;
                        d.set_status(format!("autorisation refusée dans Discord ({message})"));
                        anyhow::bail!("autorisation refusée");
                    }
                    Some(Pending::Command) => {
                        if self.model.write {
                            log::info!("Discord RPC : commandes refusées ({message}), lecture seule");
                            self.model.write = false;
                            self.publish();
                        }
                    }
                    _ => log::debug!("Discord RPC : {cmd} : {message}"),
                }
                continue;
            }
            match (cmd, evt, kind) {
                ("DISPATCH", Some("READY"), _) => {
                    self.model.me = data["user"]["id"].as_str().map(str::to_owned);
                    match horizon_sys::credentials::read(&token_target(&self.client_id)) {
                        Some(token) => self.command("AUTHENTICATE", json!({ "access_token": token }), Pending::Authenticate)?,
                        None if *refused => {
                            d.set_status("autorisation refusée (redemandée au prochain démarrage d'Horizon)");
                            anyhow::bail!("autorisation refusée");
                        }
                        None => {
                            d.set_status("autorisation demandée dans Discord");
                            asked_at = Some(Instant::now());
                            let args = json!({ "client_id": self.client_id, "scopes": rpc::SCOPES });
                            self.command("AUTHORIZE", args, Pending::Authorize)?;
                        }
                    }
                }
                (_, _, Some(Pending::Authorize)) => {
                    asked_at = None;
                    let code = data["code"].as_str().unwrap_or_default().to_owned();
                    let token = exchange(&self.client_id, self.secret.as_deref(), &code).map_err(|e| anyhow::anyhow!("échange du code : {e}"))?;
                    let _ = horizon_sys::credentials::write(&token_target(&self.client_id), &token);
                    self.command("AUTHENTICATE", json!({ "access_token": token }), Pending::Authenticate)?;
                }
                (_, _, Some(Pending::Authenticate)) => {
                    self.model.ready = true;
                    self.model.write = true;
                    d.active.store(true, Ordering::SeqCst);
                    let who = if self.secret.is_some() { "ton application" } else { "StreamKit" };
                    d.set_status(format!("connecté ({who})"));
                    log::info!("Discord RPC : connecté ({who})");
                    self.command("GET_SELECTED_VOICE_CHANNEL", json!({}), Pending::Channel)?;
                    self.command("GET_VOICE_SETTINGS", json!({}), Pending::Settings)?;
                    self.subscribe("VOICE_CHANNEL_SELECT", json!({}), true)?;
                    self.subscribe("VOICE_SETTINGS_UPDATE", json!({}), true)?;
                    self.publish();
                }
                (_, _, Some(Pending::Channel)) => {
                    let old = self.model.channel_id().map(str::to_owned);
                    self.model.set_channel(data);
                    let new = self.model.channel_id().map(str::to_owned);
                    self.follow(old, new)?;
                    if let Some(g) = self.model.guild_id().map(str::to_owned) {
                        self.command("GET_GUILD", json!({ "guild_id": g }), Pending::Guild)?;
                    }
                    self.publish();
                }
                (_, _, Some(Pending::Guild)) => {
                    if let (Some(id), Some(name)) = (data["id"].as_str(), data["name"].as_str()) {
                        self.model.set_guild_name(id, name);
                        self.publish();
                    }
                }
                (_, _, Some(Pending::Settings)) => {
                    self.model.settings(data);
                    self.publish();
                }
                ("DISPATCH", Some("VOICE_CHANNEL_SELECT"), _) => match data["channel_id"].as_str() {
                    Some(c) => self.command("GET_CHANNEL", json!({ "channel_id": c }), Pending::Channel)?,
                    None => {
                        let old = self.model.channel_id().map(str::to_owned);
                        self.model.set_channel(&Value::Null);
                        self.follow(old, None)?;
                        self.publish();
                    }
                },
                ("DISPATCH", Some("VOICE_SETTINGS_UPDATE"), _) => {
                    self.model.settings(data);
                    self.publish();
                }
                ("DISPATCH", Some(e @ ("VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" | "VOICE_STATE_DELETE")), _) => {
                    self.model.voice_state(e, data);
                    self.publish();
                }
                ("DISPATCH", Some(e @ ("SPEAKING_START" | "SPEAKING_STOP")), _) => {
                    if let Some(u) = data["user_id"].as_str() {
                        self.model.speaking(u, e == "SPEAKING_START");
                        self.publish();
                    }
                }
                _ => {}
            }
        }
    }
}

/// Code d'autorisation → jeton : échange de StreamKit, ou OAuth2 de ta propre application.
fn exchange(client_id: &str, secret: Option<&str>, code: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let mut response = match secret {
        None => agent.post(rpc::STREAMKIT_TOKEN_URL).header("Content-Type", "application/json").send(json!({ "code": code }).to_string()),
        Some(secret) => {
            let form = [("client_id", client_id), ("client_secret", secret), ("grant_type", "authorization_code"), ("code", code)];
            agent.post("https://discord.com/api/oauth2/token").send_form(form)
        }
    }
    .map_err(|e| e.to_string())?;
    let text = response.body_mut().with_config().limit(64 * 1024).read_to_string().map_err(|e| e.to_string())?;
    let body: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    body["access_token"].as_str().map(str::to_owned).ok_or_else(|| "réponse sans jeton".to_owned())
}
