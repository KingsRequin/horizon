//! Permissions du bac à sable d'un plugin (`permissions` de `plugin.json`) : tout ce que son
//! script peut joindre doit y figurer.

use serde::{Deserialize, Serialize};
use std::path::Path;
use url::Url;

/// Ce qu'un plugin a déclaré pouvoir faire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Permissions {
    /// Connexions locales (`ws://` ou `http://` vers `127.0.0.1`/`localhost`), préfixes d'URL.
    pub connect: Vec<String>,
    /// Fichiers lisibles (chemins exacts, `%LOCALAPPDATA%` et `%APPDATA%` permis).
    pub files: Vec<String>,
    /// Adresses web (`https://`), préfixes d'URL.
    pub http: Vec<String>,
    /// Événements d'Horizon reçus (voir [`super::EVENTS`]).
    pub events: Vec<String>,
}

/// Hôtes permis pour `connect`.
const LOCAL_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];

/// URL analysée (WHATWG : `..`, `%2e`, casse de l'hôte déjà résolus) ; refusée si elle porte
/// un nom d'utilisateur ou un mot de passe, ou n'a pas d'hôte.
fn parse(url: &str) -> Option<Url> {
    let u = Url::parse(url).ok()?;
    (u.username().is_empty() && u.password().is_none() && u.host_str().is_some()).then_some(u)
}

/// Segments du chemin, sans le dernier s'il est vide (`/v1/` → `["v1"]`).
fn segments(u: &Url) -> Vec<&str> {
    let mut s: Vec<&str> = u.path_segments().map(Iterator::collect).unwrap_or_default();
    while s.last() == Some(&"") {
        s.pop();
    }
    s
}

/// `url` est couverte par l'entrée déclarée `entry` : même schéma, hôte et port, et chemin qui
/// commence par celui de l'entrée, segment par segment.
fn covers(entry: &Url, url: &Url) -> bool {
    entry.scheme() == url.scheme()
        && entry.host_str() == url.host_str()
        && entry.port_or_known_default() == url.port_or_known_default()
        && segments(url).starts_with(&segments(entry))
}

/// Connexion locale permise (`ws://`/`http://` vers un hôte local).
fn local(u: &Url) -> bool {
    matches!(u.scheme(), "ws" | "http") && u.host_str().is_some_and(|h| LOCAL_HOSTS.contains(&h))
}

/// Entrée `connect` valide : `ws://`/`http://` vers un hôte local.
pub(super) fn valid_connect(entry: &str) -> Result<(), String> {
    match parse(entry) {
        Some(u) if local(&u) => Ok(()),
        Some(u) if matches!(u.scheme(), "ws" | "http") => {
            Err(format!("« {entry} » : seules les connexions vers 127.0.0.1 ou localhost sont permises"))
        }
        _ => Err(format!("« {entry} » : adresse ws:// ou http:// attendue")),
    }
}

/// Entrée `http` valide : `https://` avec un hôte.
pub(super) fn valid_http(entry: &str) -> Result<(), String> {
    match parse(entry) {
        Some(u) if u.scheme() == "https" => Ok(()),
        _ => Err(format!("« {entry} » : adresse https:// attendue")),
    }
}

/// Entrée `files` valide : chemin absolu (ou commençant par une variable permise), sans `..`.
pub(super) fn valid_file(entry: &str) -> Result<(), String> {
    let p = entry.replace('/', "\\");
    if p.split('\\').any(|c| c == "..") {
        return Err(format!("« {entry} » : « .. » interdit"));
    }
    let starts_var = p.starts_with("%LOCALAPPDATA%\\") || p.starts_with("%APPDATA%\\");
    let absolute = p.len() > 3 && p.as_bytes()[1] == b':' && p.as_bytes()[2] == b'\\';
    if starts_var || absolute { Ok(()) } else { Err(format!("« {entry} » : chemin absolu, %LOCALAPPDATA%\\… ou %APPDATA%\\… attendu")) }
}

/// Chemin normalisé pour la comparaison (barres inverses, minuscules), `None` s'il contient
/// `..` ou un préfixe `\\?\` / `\\.\` (contournement de la comparaison).
fn normalize(path: &str) -> Option<String> {
    let p = path.replace('/', "\\");
    if p.starts_with("\\\\") || p.split('\\').any(|c| c == "..") {
        return None;
    }
    Some(p.to_lowercase())
}

/// Variables `%LOCALAPPDATA%` et `%APPDATA%` développées en tête de chemin.
fn expand(entry: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let p = entry.replace('/', "\\");
    for var in ["LOCALAPPDATA", "APPDATA"] {
        if let Some(rest) = p.strip_prefix(&format!("%{var}%")) {
            return Some(format!("{}{rest}", env(var)?.trim_end_matches('\\')));
        }
    }
    Some(p)
}

/// Nom en clair d'un événement (fenêtre d'accord).
fn event_label(name: &str) -> &str {
    match name {
        "accent" => "couleur de la pochette",
        "media" => "musique en cours",
        "dnd" => "« Ne pas déranger »",
        "alert" => "alertes d'Horizon",
        "timer" => "fin du minuteur",
        "presence" => "verrouillage et veille",
        "foreground" => "application au premier plan",
        other => other,
    }
}

impl Permissions {
    /// Le script peut ouvrir cette connexion locale : l'URL normalisée à utiliser (celle qui a
    /// été vérifiée), `None` sinon.
    pub fn allows_connect(&self, url: &str) -> Option<String> {
        let u = parse(url).filter(local)?;
        self.connect.iter().filter_map(|e| parse(e)).any(|e| covers(&e, &u)).then(|| u.to_string())
    }

    /// Le script peut joindre cette adresse web : l'URL normalisée à utiliser, `None` sinon.
    pub fn allows_http(&self, url: &str) -> Option<String> {
        let u = parse(url).filter(|u| u.scheme() == "https")?;
        self.http.iter().filter_map(|e| parse(e)).any(|e| covers(&e, &u)).then(|| u.to_string())
    }

    /// Le script peut lire ce fichier. `env` donne les variables d'environnement.
    pub fn allows_file(&self, path: &Path, env: &dyn Fn(&str) -> Option<String>) -> bool {
        let Some(wanted) = path.to_str().and_then(normalize) else { return false };
        self.files.iter().filter_map(|e| expand(e, env)).filter_map(|e| normalize(&e)).any(|e| e == wanted)
    }

    /// Le plugin reçoit cet événement.
    pub fn allows_event(&self, name: &str) -> bool {
        self.events.iter().any(|e| e == name)
    }

    /// Ce que `self` déclare en plus de `old` (un nouvel accord est nécessaire s'il y en a).
    pub fn added_since(&self, old: &Permissions) -> Permissions {
        let new = |a: &[String], b: &[String]| a.iter().filter(|x| !b.contains(x)).cloned().collect();
        Permissions {
            connect: new(&self.connect, &old.connect),
            files: new(&self.files, &old.files),
            http: new(&self.http, &old.http),
            events: new(&self.events, &old.events),
        }
    }

    /// Rien de déclaré.
    pub fn is_empty(&self) -> bool {
        self.connect.is_empty() && self.files.is_empty() && self.http.is_empty() && self.events.is_empty()
    }

    /// Phrases pour la fenêtre d'accord, une par permission.
    pub fn describe(&self) -> Vec<String> {
        let local = |e: &String| e.split_once("://").map_or(e.as_str(), |(_, r)| r).trim_end_matches('/').to_owned();
        self.connect
            .iter()
            .map(|e| format!("Se connecter à une application de ce PC ({})", local(e)))
            .chain(self.files.iter().map(|e| format!("Lire le fichier {e}")))
            .chain(self.http.iter().map(|e| format!("Accéder à {e}")))
            .chain(self.events.iter().map(|e| format!("Recevoir : {}", event_label(e))))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn perms() -> Permissions {
        Permissions {
            connect: vec!["ws://127.0.0.1:28650".into()],
            files: vec!["%LOCALAPPDATA%/KingsHub/phone.json".into()],
            http: vec!["https://api.example.com/v1/".into()],
            events: vec!["dnd".into()],
        }
    }

    fn env(var: &str) -> Option<String> {
        (var == "LOCALAPPDATA").then(|| r"C:\Users\u\AppData\Local".to_owned())
    }

    #[test]
    fn connect_prefix() {
        let p = perms();
        assert_eq!(p.allows_connect("ws://127.0.0.1:28650").as_deref(), Some("ws://127.0.0.1:28650/"));
        assert!(p.allows_connect("ws://127.0.0.1:28650/x").is_some());
        assert!(p.allows_connect("ws://127.0.0.1:28651").is_none());
        let short = Permissions { connect: vec!["ws://127.0.0.1:2865".into()], ..Default::default() };
        assert!(short.allows_connect("ws://127.0.0.1:28650").is_none());
    }

    #[test]
    fn connect_rejects_userinfo_tricks() {
        let p = perms();
        for url in [
            "ws://127.0.0.1:28650@evil.com/",
            "ws://user@127.0.0.1:28650/",
            "ws://evil.com#@127.0.0.1:28650",
            "ws://127.0.0.1.evil.com:28650/",
        ] {
            assert!(p.allows_connect(url).is_none(), "{url}");
        }
        // `\` vaut `/` (WHATWG) : l'URL utilisée reste sur la machine.
        assert_eq!(p.allows_connect(r"ws://127.0.0.1:28650\@evil.com/").as_deref(), Some("ws://127.0.0.1:28650/@evil.com/"));
    }

    #[test]
    fn http_requires_https() {
        let p = perms();
        assert!(p.allows_http("https://api.example.com/v1/forecast?x=1").is_some());
        assert!(p.allows_http("https://API.example.com/v1/").is_some());
        assert!(p.allows_http("http://api.example.com/v1/forecast").is_none());
        assert!(p.allows_http("https://api.example.com.evil.net/v1/").is_none());
        assert!(p.allows_http("https://other.com/v1/").is_none());
        assert!(p.allows_http("https://api.example.com/v1x").is_none());
    }

    #[test]
    fn http_rejects_path_escape() {
        let p = perms();
        for url in [
            "https://api.example.com/v1/../admin",
            "https://api.example.com/v1/%2e%2e/admin",
            "https://api.example.com/v1/..%2fadmin/../../admin",
            "https://u:p@api.example.com/v1/",
        ] {
            assert!(p.allows_http(url).is_none(), "{url}");
        }
    }

    #[test]
    fn file_exact_and_no_escape() {
        let p = perms();
        let base = r"C:\Users\u\AppData\Local\KingsHub";
        assert!(p.allows_file(&PathBuf::from(format!(r"{base}\PHONE.json")), &env));
        assert!(p.allows_file(&PathBuf::from("C:/Users/u/AppData/Local/KingsHub/phone.json"), &env));
        assert!(!p.allows_file(&PathBuf::from(format!(r"{base}\..\KingsHub\phone.json")), &env));
        assert!(!p.allows_file(&PathBuf::from(format!(r"{base}\phone.json.bak")), &env));
        assert!(!p.allows_file(&PathBuf::from(format!(r"\\?\{base}\phone.json")), &env));
    }

    #[test]
    fn added_since_lists_only_new() {
        let old = perms();
        let mut new = perms();
        new.events.push("accent".into());
        let added = new.added_since(&old);
        assert_eq!(added, Permissions { events: vec!["accent".into()], ..Default::default() });
        assert!(old.added_since(&old).is_empty());
    }

    #[test]
    fn describe_in_french() {
        let d = perms().describe();
        assert_eq!(d[0], "Se connecter à une application de ce PC (127.0.0.1:28650)");
        assert_eq!(d[3], "Recevoir : « Ne pas déranger »");
    }
}
