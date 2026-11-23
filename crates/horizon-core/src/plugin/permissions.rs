//! Permissions du bac à sable d'un plugin (`permissions` de `plugin.json`) : tout ce que son
//! script peut joindre doit y figurer.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use url::Url;

/// Ce qu'un plugin a déclaré pouvoir faire.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Permissions {
    /// Connexions locales (`ws://` vers `127.0.0.1`/`localhost`), préfixes d'URL.
    pub connect: Vec<String>,
    /// Fichiers lisibles (chemins exacts ; variables de [`FILE_VARS`] permises en tête).
    pub files: Vec<String>,
    /// Adresses web (`https://`), préfixes d'URL.
    pub http: Vec<String>,
    /// Événements d'Horizon reçus (voir [`super::EVENTS`]).
    pub events: Vec<String>,
    /// Actions sur le système (voir [`super::SYSTEM`]).
    pub system: Vec<String>,
}

/// Hôtes permis pour `connect`.
const LOCAL_HOSTS: [&str; 2] = ["127.0.0.1", "localhost"];

/// Variables d'environnement permises en tête d'un chemin de `files`.
const FILE_VARS: [&str; 3] = ["LOCALAPPDATA", "APPDATA", "USERPROFILE"];

/// URL analysée (WHATWG : `..`, casse de l'hôte déjà résolus) ; refusée si elle porte un nom
/// d'utilisateur ou un mot de passe, n'a pas d'hôte, ou si son chemin contient un point ou un
/// séparateur encodé (`%2e`, `%2f`, `%5c` : un serveur qui les décode sortirait du préfixe).
fn parse(url: &str) -> Option<Url> {
    let u = Url::parse(url).ok()?;
    let path = u.path().to_ascii_lowercase();
    let encoded = ["%2e", "%2f", "%5c"].iter().any(|e| path.contains(e));
    (u.username().is_empty() && u.password().is_none() && u.host_str().is_some() && !encoded).then_some(u)
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

/// Connexion locale permise (`ws://` vers un hôte local).
fn local(u: &Url) -> bool {
    u.scheme() == "ws" && u.host_str().is_some_and(|h| LOCAL_HOSTS.contains(&h))
}

/// Entrée `connect` valide : `ws://` vers un hôte local.
pub(super) fn valid_connect(entry: &str) -> Result<(), String> {
    match parse(entry) {
        Some(u) if local(&u) => Ok(()),
        Some(u) if u.scheme() == "ws" => Err(format!("« {entry} » : seules les connexions vers 127.0.0.1 ou localhost sont permises")),
        _ => Err(format!("« {entry} » : adresse ws:// attendue")),
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
    let starts_var = FILE_VARS.iter().any(|v| p.strip_prefix(&format!("%{v}%")).is_some_and(|rest| rest.starts_with('\\')));
    let absolute = p.len() > 3 && p.as_bytes()[1] == b':' && p.as_bytes()[2] == b'\\';
    if starts_var || absolute {
        Ok(())
    } else {
        Err(format!("« {entry} » : chemin absolu, %LOCALAPPDATA%\\…, %APPDATA%\\… ou %USERPROFILE%\\… attendu"))
    }
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

/// Variable de [`FILE_VARS`] développée en tête de chemin.
fn expand(entry: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let p = entry.replace('/', "\\");
    for var in FILE_VARS {
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

    /// Le script peut ouvrir ce lien dans le navigateur : un `https://` déclaré dans `http`, ou
    /// tout lien `http://` / `https://` s'il a l'action `open`. L'URL normalisée, `None` sinon.
    pub fn allows_open(&self, url: &str) -> Option<String> {
        if self.allows_system("open") {
            return parse(url).filter(|u| matches!(u.scheme(), "http" | "https")).map(|u| u.to_string());
        }
        self.allows_http(url)
    }

    /// Le plugin a déclaré cette action sur le système.
    pub fn allows_system(&self, name: &str) -> bool {
        self.system.iter().any(|e| e == name)
    }

    /// Le script peut lire ce fichier (variables de [`FILE_VARS`] permises, comme dans le
    /// manifeste) : le chemin réel à lire, `None` sinon. `env` donne les variables.
    pub fn allows_file(&self, path: &str, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
        let real = expand(path, env)?;
        let wanted = normalize(&real)?;
        self.files.iter().filter_map(|e| expand(e, env)).filter_map(|e| normalize(&e)).any(|e| e == wanted).then(|| PathBuf::from(real))
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
            system: new(&self.system, &old.system),
        }
    }

    /// Rien de déclaré.
    pub fn is_empty(&self) -> bool {
        self.connect.is_empty() && self.files.is_empty() && self.http.is_empty() && self.events.is_empty() && self.system.is_empty()
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
            .chain(self.system.iter().map(|e| super::system::label(e).to_owned()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms() -> Permissions {
        Permissions {
            connect: vec!["ws://127.0.0.1:28650".into()],
            files: vec!["%LOCALAPPDATA%/KingsHub/phone.json".into()],
            http: vec!["https://api.example.com/v1/".into()],
            events: vec!["dnd".into()],
            system: vec![],
        }
    }

    fn env(var: &str) -> Option<String> {
        match var {
            "LOCALAPPDATA" => Some(r"C:\Users\u\AppData\Local".to_owned()),
            "USERPROFILE" => Some(r"C:\Users\u".to_owned()),
            _ => None,
        }
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
    fn connect_is_websocket_only() {
        assert!(valid_connect("http://127.0.0.1:8080").is_err());
        let p = Permissions { connect: vec!["ws://127.0.0.1:8080".into()], ..Default::default() };
        assert!(p.allows_connect("http://127.0.0.1:8080").is_none());
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
            "https://api.example.com/v1/..%2f..%2fadmin",
            "https://api.example.com/v1/%2E%2E%5Cadmin",
        ] {
            assert!(p.allows_http(url).is_none(), "{url}");
        }
    }

    #[test]
    fn file_exact_and_no_escape() {
        let p = perms();
        let base = r"C:\Users\u\AppData\Local\KingsHub";
        assert!(p.allows_file(&format!(r"{base}\PHONE.json"), &env).is_some());
        assert!(p.allows_file("C:/Users/u/AppData/Local/KingsHub/phone.json", &env).is_some());
        // Le script peut aussi écrire le chemin comme dans le manifeste.
        assert_eq!(
            p.allows_file("%LOCALAPPDATA%/KingsHub/phone.json", &env),
            Some(PathBuf::from(r"C:\Users\u\AppData\Local\KingsHub\phone.json"))
        );
        assert!(p.allows_file(&format!(r"{base}\..\KingsHub\phone.json"), &env).is_none());
        assert!(p.allows_file(&format!(r"{base}\phone.json.bak"), &env).is_none());
        assert!(p.allows_file(&format!(r"\\?\{base}\phone.json"), &env).is_none());
        assert!(p.allows_file("%APPDATA%/KingsHub/phone.json", &env).is_none());
    }

    #[test]
    fn file_under_the_user_profile() {
        assert!(valid_file("%USERPROFILE%/.ssh/config").is_ok());
        assert!(valid_file("%USERPROFILE%").is_err());
        assert!(valid_file("%USERPROFILES%/x").is_err());
        assert!(valid_file("%TEMP%/x").is_err());
        let p = Permissions { files: vec!["%USERPROFILE%/.ssh/config".into()], ..Default::default() };
        assert_eq!(p.allows_file("%USERPROFILE%/.ssh/config", &env), Some(PathBuf::from(r"C:\Users\u\.ssh\config")));
        assert!(p.allows_file(r"C:\Users\u\.ssh\config", &env).is_some());
        assert!(p.allows_file("%USERPROFILE%/.ssh/id_ed25519", &env).is_none());
    }

    #[test]
    fn open_follows_http_unless_declared() {
        let p = perms();
        assert!(p.allows_open("https://api.example.com/v1/doc").is_some());
        assert!(p.allows_open("http://192.168.1.20:8080/").is_none());
        let open = Permissions { system: vec!["open".into()], ..Default::default() };
        assert_eq!(open.allows_open("http://192.168.1.20:8080").as_deref(), Some("http://192.168.1.20:8080/"));
        assert!(open.allows_open("https://nas.local/").is_some());
        for bad in ["file:///C:/Windows/notepad.exe", "ms-settings:", "https://u:p@nas.local/", "calc.exe"] {
            assert!(open.allows_open(bad).is_none(), "{bad}");
        }
        assert!(open.allows_system("open") && !open.allows_system("ssh"));
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
        let system = Permissions { system: vec!["probe".into(), "ssh".into()], ..Default::default() };
        assert_eq!(system.describe(), ["Tester si des machines répondent (ce PC, réseau local, internet)", "Ouvrir un terminal SSH"]);
        assert!(!system.is_empty() && !system.added_since(&Permissions::default()).is_empty());
    }
}
