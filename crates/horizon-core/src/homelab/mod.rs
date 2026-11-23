//! Widget « Serveurs » (homelab) : hôtes lus dans `~/.ssh/config`, réglages du widget
//! (renommer, masquer, ajouter, choisir le test) et état de chaque hôte avec anti-rebond.
//!
//! Seuls `Host`, `HostName`, `Port` et `User` sont lus : les clés (`IdentityFile`…) et le
//! reste de la configuration ne sont jamais conservés (`ssh_config`).

mod ssh_config;

pub use ssh_config::{SshHost, parse_ssh_config};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Port testé par défaut (SSH).
pub(crate) const DEFAULT_PORT: u16 = 22;
/// Latence (ms) au-delà de laquelle un hôte joignable est « lent » (pastille orange).
pub(crate) const SLOW_MS: u32 = 300;
/// Échecs consécutifs avant de déclarer un hôte hors ligne (un paquet perdu ne suffit pas).
pub(crate) const FAILS_TO_DOWN: u8 = 2;

/// Test d'un hôte : connexion TCP (port SSH par défaut) ou requête HTTP(S).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    #[default]
    Tcp,
    Http,
}

/// Réglage d'un hôte dans le widget. Sans `address`, il complète l'hôte `id` de
/// `~/.ssh/config` ; avec, c'est un hôte ajouté à la main.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HostSetting {
    pub id: String,
    pub address: Option<String>,
    pub label: Option<String>,
    pub hidden: bool,
    pub check: CheckKind,
    pub port: Option<u16>,
    /// Interface web : ouverte depuis le panneau, et testée si `check` = HTTP.
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ServersSettings {
    pub hosts: Vec<HostSetting>,
}

impl ServersSettings {
    /// Réglages d'un widget ; absents ou illisibles = aucun réglage (tous les hôtes de ssh).
    pub fn from_value(v: &Value) -> ServersSettings {
        serde_json::from_value(v.clone()).unwrap_or_default()
    }
}

/// Façon de joindre un hôte. Deux hôtes au même test ne sont sondés qu'une fois.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Probe {
    Tcp { host: String, port: u16 },
    Http { url: String },
}

impl Probe {
    /// Texte court pour l'interface (« TCP 22 », « HTTP »).
    pub fn label(&self) -> String {
        match self {
            Probe::Tcp { port, .. } => format!("TCP {port}"),
            Probe::Http { url } if url.starts_with("https://") => "HTTPS".into(),
            Probe::Http { .. } => "HTTP".into(),
        }
    }
}

/// Hôte à surveiller, réglages appliqués.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Target {
    pub id: String,
    pub label: String,
    pub address: String,
    /// Argument de `ssh` (alias de `~/.ssh/config`, ou adresse d'un hôte ajouté à la main) ;
    /// `None` si ce nom ne peut pas être passé sans risque à `ssh`.
    pub ssh: Option<String>,
    pub url: Option<String>,
    pub probe: Probe,
    pub hidden: bool,
    /// Ajouté à la main (sinon lu dans `~/.ssh/config`).
    pub manual: bool,
}

/// Adresse web acceptée (http ou https seulement).
pub(crate) fn web_url(url: Option<&str>) -> Option<String> {
    let u = url?.trim();
    (u.starts_with("https://") || u.starts_with("http://")).then(|| u.to_owned())
}

pub use crate::plugin::safe_ssh_target;

fn target(id: &str, address: &str, ssh_port: Option<u16>, s: Option<&HostSetting>, manual: bool) -> Target {
    let url = web_url(s.and_then(|s| s.url.as_deref()));
    let port = s.and_then(|s| s.port).or(ssh_port).unwrap_or(DEFAULT_PORT);
    let probe = match (s.map(|s| s.check), &url) {
        (Some(CheckKind::Http), Some(u)) => Probe::Http { url: u.clone() },
        // HTTP sans adresse web : retour au test TCP.
        _ => Probe::Tcp { host: address.to_owned(), port },
    };
    let ssh_name = if manual { address } else { id };
    let label = s.and_then(|s| s.label.as_deref()).map(str::trim).filter(|l| !l.is_empty());
    Target {
        id: id.to_owned(),
        label: label.unwrap_or(id).to_owned(),
        address: address.to_owned(),
        ssh: safe_ssh_target(ssh_name).then(|| ssh_name.to_owned()),
        url,
        probe,
        hidden: s.is_some_and(|s| s.hidden),
        manual,
    }
}

/// Hôtes du widget : ceux de `~/.ssh/config` dans l'ordre du fichier (avec leurs réglages),
/// puis les hôtes ajoutés à la main. Les hôtes masqués sont gardés (`hidden`) pour les réglages.
pub fn targets(ssh: &[SshHost], settings: &ServersSettings) -> Vec<Target> {
    let setting = |id: &str| settings.hosts.iter().find(|h| h.address.is_none() && h.id == id);
    let mut out: Vec<Target> = ssh
        .iter()
        .map(|h| target(&h.alias, h.hostname.as_deref().unwrap_or(&h.alias), h.port, setting(&h.alias), false))
        .collect();
    for m in &settings.hosts {
        let Some(addr) = m.address.as_deref().map(str::trim).filter(|a| !a.is_empty()) else { continue };
        if m.id.is_empty() || out.iter().any(|t| t.id == m.id) {
            continue;
        }
        out.push(target(&m.id, addr, None, Some(m), true));
    }
    out
}

/// État affiché d'un hôte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Pas encore de verdict (démarrage, ou un seul échec).
    Unknown,
    Up,
    Slow,
    Down,
}

/// Changement à annoncer dans l'îlot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    WentDown,
    CameBack,
}

/// Suivi d'un hôte : un succès suffit à le dire en ligne, il faut `FAILS_TO_DOWN` échecs
/// d'affilée pour le dire hors ligne.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Health {
    /// Dernier verdict (`None` = pas encore).
    up: Option<bool>,
    fails: u8,
    latency_ms: Option<u32>,
    /// Instant du dernier changement de verdict (ms depuis l'époque Unix).
    since_ms: Option<i64>,
    /// La chute a été annoncée : le retour le sera aussi.
    announced: bool,
}

impl Health {
    /// Résultat d'un sondage (`Some(latence)` ou `None` = injoignable). Renvoie le changement
    /// à annoncer : chute d'un hôte qui était en ligne, retour d'un hôte dont la chute a été
    /// annoncée. Un hôte déjà absent au démarrage n'est pas annoncé.
    pub fn observe(&mut self, result: Option<u32>, now_ms: i64) -> Option<Change> {
        match result {
            Some(ms) => {
                self.fails = 0;
                self.latency_ms = Some(ms);
                if self.up == Some(true) {
                    return None;
                }
                let was_down = self.up == Some(false);
                self.up = Some(true);
                self.since_ms = Some(now_ms);
                let back = was_down && self.announced;
                self.announced = false;
                back.then_some(Change::CameBack)
            }
            None => {
                self.fails = self.fails.saturating_add(1);
                if self.fails < FAILS_TO_DOWN || self.up == Some(false) {
                    return None;
                }
                let was_up = self.up == Some(true);
                self.up = Some(false);
                self.since_ms = Some(now_ms);
                self.latency_ms = None;
                self.announced = was_up;
                was_up.then_some(Change::WentDown)
            }
        }
    }

    pub fn status(&self) -> Status {
        match self.up {
            None => Status::Unknown,
            Some(false) => Status::Down,
            Some(true) if self.latency_ms.is_some_and(|l| l >= SLOW_MS) => Status::Slow,
            Some(true) => Status::Up,
        }
    }

    pub fn latency_ms(&self) -> Option<u32> {
        self.latency_ms
    }

    pub fn since_ms(&self) -> Option<i64> {
        self.since_ms
    }
}

#[cfg(test)]
mod tests;
