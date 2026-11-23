//! Joue l'appli Horizon Mobile face au module « Téléphone » d'Horizon (`docs/PHONE.md`) :
//! pour les tests du serveur et pour le contrôle en vrai, sans téléphone.
//!
//! C'est une seconde écriture du protocole, volontairement indépendante de `horizon-core` :
//! si elle et le serveur se comprennent, c'est que le document dit vrai.

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};
use serde_json::{Value, json};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tungstenite::{Message, WebSocket};

/// Délai de connexion, puis d'attente d'une réponse.
const TIMEOUT: Duration = Duration::from_secs(5);
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

type Socket = WebSocket<StreamOwned<ClientConnection, TcpStream>>;

/// Ce que porte le QR code d'appairage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub hosts: Vec<String>,
    pub name: String,
    pub port: u16,
    pub fingerprint: String,
    pub token: String,
}

/// Lit `horizon://pair?v=1&h=…&n=…&p=…&fp=…&t=…`.
pub fn parse_link(text: &str) -> anyhow::Result<Link> {
    let url = url::Url::parse(text).context("lien illisible")?;
    if url.scheme() != "horizon" || url.host_str() != Some("pair") {
        bail!("ce n'est pas un lien d'appairage d'Horizon");
    }
    let get = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()).ok_or_else(|| anyhow!("champ « {key} » absent"));
    if get("v")? != "1" {
        bail!("version de lien inconnue");
    }
    let hosts: Vec<String> = get("h")?.split(',').filter(|h| !h.is_empty()).map(str::to_owned).collect();
    if hosts.is_empty() {
        bail!("aucune adresse dans le lien");
    }
    Ok(Link { hosts, name: get("n")?, port: get("p")?.parse().context("port illisible")?, fingerprint: get("fp")?, token: get("t")? })
}

/// Empreinte de la clé publique d'un certificat : SHA-256 du SPKI, base64url sans remplissage.
pub fn fingerprint(cert: &CertificateDer<'_>) -> anyhow::Result<String> {
    let parsed = webpki::EndEntityCert::try_from(cert).map_err(|e| anyhow!("certificat illisible : {e}"))?;
    let digest = ring::digest::digest(&ring::digest::SHA256, parsed.subject_public_key_info().as_ref());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

/// N'accepte que le certificat dont la clé publique a l'empreinte attendue : ni autorité,
/// ni nom d'hôte.
#[derive(Debug)]
struct Pinned {
    fingerprint: String,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match fingerprint(end_entity) {
            Ok(found) if found == self.fingerprint => Ok(ServerCertVerified::assertion()),
            Ok(_) => Err(rustls::Error::General("ce n'est pas le PC attendu (empreinte différente)".into())),
            Err(e) => Err(rustls::Error::General(e.to_string())),
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Ouvre TLS puis WebSocket vers `addr`, pour le PC d'empreinte `fingerprint`.
fn connect(addr: SocketAddr, fingerprint: &str) -> anyhow::Result<Socket> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { fingerprint: fingerprint.to_owned(), provider }))
        .with_no_client_auth();
    let tcp = TcpStream::connect_timeout(&addr, TIMEOUT).with_context(|| format!("{addr} injoignable"))?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    tcp.set_nodelay(true)?;
    let name = ServerName::try_from("horizon.local").expect("nom d'hôte valide");
    let tls = StreamOwned::new(ClientConnection::new(Arc::new(config), name)?, tcp);
    let (ws, _) = tungstenite::client(format!("ws://{addr}/"), tls).map_err(|e| anyhow!("ouverture refusée par {addr} : {e}"))?;
    Ok(ws)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Identifiant d'appareil : aléatoire, à la façon de celui que l'appli crée à l'installation.
fn new_device_id() -> String {
    let mut bytes = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).expect("générateur aléatoire du système");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Une session ouverte avec Horizon.
pub struct Client {
    ws: Socket,
    name: String,
    pub device_id: String,
    pub device_token: String,
    /// Adresse qui a répondu.
    pub addr: SocketAddr,
}

impl std::fmt::Debug for Client {
    /// Sans le jeton : cette valeur finit dans des messages d'échec de test.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("device_id", &self.device_id).field("addr", &self.addr).finish_non_exhaustive()
    }
}

impl Client {
    /// Appaire ce « téléphone » avec le PC du lien `link` (le texte du QR code).
    pub fn pair(link: &str, name: &str) -> anyhow::Result<Client> {
        let link = parse_link(link)?;
        let mut last = anyhow!("aucune adresse");
        for host in &link.hosts {
            let Some(addr) = (host.as_str(), link.port).to_socket_addrs().ok().and_then(|mut a| a.next()) else { continue };
            match Self::pair_at(addr, &link, name) {
                Ok(client) => return Ok(client),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn pair_at(addr: SocketAddr, link: &Link, name: &str) -> anyhow::Result<Client> {
        let mut ws = connect(addr, &link.fingerprint)?;
        let device_id = new_device_id();
        let request = json!({ "type": "pair.request", "token": link.token, "deviceId": device_id, "deviceName": name, "appVersion": APP_VERSION });
        ws.send(Message::text(request.to_string()))?;
        let reply = read_json(&mut ws)?;
        match reply["type"].as_str() {
            Some("pair.accepted") => {
                let device_token = reply["deviceToken"].as_str().context("pair.accepted sans deviceToken")?.to_owned();
                Ok(Client { ws, name: name.to_owned(), device_id, device_token, addr })
            }
            Some("pair.rejected") => bail!("appairage refusé : {}", reply["reason"].as_str().unwrap_or("?")),
            _ => bail!("réponse inattendue : {reply}"),
        }
    }

    /// Rouvre une session avec le jeton reçu à l'appairage.
    pub fn hello(addr: SocketAddr, fingerprint: &str, device_id: &str, device_token: &str) -> anyhow::Result<Client> {
        let mut ws = connect(addr, fingerprint)?;
        let hello = json!({ "type": "hello", "deviceId": device_id, "deviceToken": device_token, "protocol": 1, "appVersion": APP_VERSION });
        ws.send(Message::text(hello.to_string()))?;
        let reply = read_json(&mut ws)?;
        match reply["type"].as_str() {
            Some("hello.ok") => Ok(Client { ws, name: "Faux téléphone".to_owned(), device_id: device_id.to_owned(), device_token: device_token.to_owned(), addr }),
            Some("hello.rejected") => bail!("session refusée : {}", reply["reason"].as_str().unwrap_or("?")),
            _ => bail!("réponse inattendue : {reply}"),
        }
    }

    /// Envoie la batterie du « téléphone ».
    pub fn send_battery(&mut self, percent: u8, charging: bool) -> anyhow::Result<()> {
        let device = json!({ "id": "phone", "kind": "phone", "name": self.name, "percent": percent, "charging": charging, "atMs": now_ms() });
        self.send_raw(&json!({ "type": "phone.battery", "devices": [device] }).to_string())
    }

    /// Envoie une trame de texte telle quelle (messages tordus des tests).
    pub fn send_raw(&mut self, text: &str) -> anyhow::Result<()> {
        self.ws.send(Message::text(text)).map_err(|e| anyhow!("envoi impossible : {e}"))
    }

    /// Prochain message du PC ; erreur si rien n'arrive en 5 s ou si la connexion est fermée.
    pub fn next(&mut self) -> anyhow::Result<String> {
        loop {
            match self.ws.read().map_err(|e| anyhow!("lecture impossible : {e}"))? {
                Message::Text(text) => return Ok(text.to_string()),
                Message::Close(_) => bail!("connexion fermée par le PC"),
                _ => {}
            }
        }
    }
}

fn read_json(ws: &mut Socket) -> anyhow::Result<Value> {
    loop {
        match ws.read().map_err(|e| anyhow!("pas de réponse : {e}"))? {
            Message::Text(text) => return serde_json::from_str(&text).context("réponse illisible"),
            Message::Close(_) => bail!("connexion fermée par le PC"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_link_is_read() {
        let link = parse_link("horizon://pair?v=1&h=192.168.1.49,10.0.0.5&n=PC%20%C3%89t%C3%A9&p=28680&fp=AbC-_9&t=tok").unwrap();
        assert_eq!(link, Link { hosts: vec!["192.168.1.49".into(), "10.0.0.5".into()], name: "PC Été".into(), port: 28680, fingerprint: "AbC-_9".into(), token: "tok".into() });
        for bad in ["https://exemple.fr", "horizon://pair?v=2&h=1.2.3.4&n=a&p=1&fp=f&t=t", "horizon://pair?v=1&h=&n=a&p=1&fp=f&t=t", "horizon://pair?v=1&h=1.2.3.4&n=a&p=x&fp=f&t=t", "horizon://autre?v=1"] {
            assert!(parse_link(bad).is_err(), "{bad}");
        }
    }
}
