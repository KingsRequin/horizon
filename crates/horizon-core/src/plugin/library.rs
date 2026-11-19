//! Bibliothèque de plugins (logique pure) : index publié par le dépôt `KingsRequin/horizon-plugins`
//! (`index.json`), paquets (un fichier JSON par version, qui contient les fichiers du plugin) et
//! état de chaque plugin pour la page Plugins.
//!
//! Un paquet : `{ "id", "version", "files": { "plugin.json": "…", "assets/a.png": "base64:…" } }`.
//! Son empreinte SHA-256 est dans l'index ; ses chemins sont vérifiés avant toute écriture.

use super::Permissions;
use crate::update::Version;
use serde::Deserialize;

/// Dépôt de la bibliothèque (adresse brute de GitHub) : index et paquets n'en viennent pas
/// d'ailleurs.
pub const BASE: &str = "https://raw.githubusercontent.com/KingsRequin/horizon-plugins/main/";
pub const INDEX: &str = "https://raw.githubusercontent.com/KingsRequin/horizon-plugins/main/index.json";
/// Tailles maximales : index, paquet, contenu décodé d'un plugin, nombre de fichiers.
pub const MAX_INDEX: u64 = 2 * 1024 * 1024;
pub const MAX_BUNDLE: u64 = 24 * 1024 * 1024;
const MAX_FILES_BYTES: usize = 20 * 1024 * 1024;
const MAX_FILES: usize = 500;

/// Un plugin de l'index.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: u32,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Taille du paquet (octets) et son empreinte.
    pub size: u64,
    pub sha256: String,
    /// Adresse du paquet (sous [`BASE`]).
    pub url: String,
}

#[derive(Deserialize)]
struct RawIndex {
    plugins: Vec<serde_json::Value>,
}

/// Lit l'index : les entrées lisibles, d'une API acceptée par cet Horizon, dont le paquet est
/// dans le dépôt de la bibliothèque. Les autres sont écartées (une entrée cassée ne cache pas
/// les autres).
pub fn parse_index(text: &str) -> anyhow::Result<Vec<Entry>> {
    let raw: RawIndex = serde_json::from_str(text)?;
    Ok(raw
        .plugins
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Entry>(v).ok())
        .filter(|e| (super::API_MIN..=super::API_MAX).contains(&e.api))
        .filter(|e| e.url.starts_with(BASE) && !e.url.contains(".."))
        .filter(|e| (1..=40).contains(&e.id.len()) && e.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'))
        .collect())
}

/// Chemin de fichier permis dans un paquet : relatif, sans `..`, et parmi les fichiers d'un plugin.
fn allowed_path(p: &str) -> bool {
    let ok_chars = p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'));
    let parts: Vec<&str> = p.split('/').collect();
    let clean = ok_chars && !p.starts_with('/') && parts.iter().all(|c| !c.is_empty() && *c != "." && *c != "..");
    let kind = matches!(p, "plugin.json" | "main.js" | "README.md" | "LICENSE" | "PLUGINS.md") || p.ends_with(".slint") || p.starts_with("assets/");
    clean && kind && parts.len() <= 5
}

/// Décode le base64 standard (sans espaces).
fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        let take = match chunk.len() {
            4 => 3,
            3 => 2,
            2 => 1,
            _ => return None,
        };
        out.extend_from_slice(&n.to_be_bytes()[1..=take]);
    }
    Some(out)
}

/// Fichiers d'un paquet (chemin relatif, contenu) vérifiés pour le plugin `id` : chemins permis,
/// `plugin.json` présent et du même `id`, tailles bornées.
pub fn parse_bundle(bytes: &[u8], id: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    #[derive(Deserialize)]
    struct Raw {
        id: String,
        files: std::collections::BTreeMap<String, String>,
    }
    let raw: Raw = serde_json::from_slice(bytes)?;
    anyhow::ensure!(raw.id == id, "paquet d'un autre plugin ({})", raw.id);
    anyhow::ensure!(raw.files.len() <= MAX_FILES, "trop de fichiers");
    let mut out = Vec::new();
    let mut total = 0usize;
    for (path, content) in raw.files {
        anyhow::ensure!(allowed_path(&path), "chemin refusé : {path}");
        let data = match content.strip_prefix("base64:") {
            Some(b) => base64(b).ok_or_else(|| anyhow::anyhow!("{path} : base64 illisible"))?,
            None => content.into_bytes(),
        };
        total += data.len();
        anyhow::ensure!(total <= MAX_FILES_BYTES, "plugin trop gros");
        out.push((path, data));
    }
    let manifest = out.iter().find(|(p, _)| p == "plugin.json").ok_or_else(|| anyhow::anyhow!("plugin.json absent"))?;
    let m: serde_json::Value = serde_json::from_slice(&manifest.1)?;
    anyhow::ensure!(m["id"] == id, "plugin.json d'un autre plugin");
    Ok(out)
}

/// Une version de l'index est plus récente que celle installée.
pub fn newer(available: &str, installed: &str) -> bool {
    match (Version::parse(available), Version::parse(installed)) {
        (Some(a), Some(i)) => a > i,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, url: &str, api: u32) -> serde_json::Value {
        serde_json::json!({ "id": id, "name": id, "version": "1.0.0", "api": api, "size": 10, "sha256": "ab", "url": url })
    }

    #[test]
    fn index_keeps_only_safe_entries() {
        let good = format!("{BASE}dist/meteo-1.0.0.json");
        let text = serde_json::json!({ "plugins": [
            entry("meteo", &good, 0),
            entry("ailleurs", "https://exemple.com/x.json", 0),
            entry("futur", &good, 99),
            entry("Majuscule", &good, 0),
            entry("remonte", &format!("{BASE}../x.json"), 0),
            { "id": "cassé" },
        ] })
        .to_string();
        let list = parse_index(&text).unwrap();
        assert_eq!(list.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["meteo"]);
    }

    #[test]
    fn bundle_paths_are_checked() {
        let ok = serde_json::json!({ "id": "meteo", "files": {
            "plugin.json": "{\"id\":\"meteo\"}", "widget.slint": "x", "assets/a.png": "base64:AAEC" } });
        let files = parse_bundle(ok.to_string().as_bytes(), "meteo").unwrap();
        assert_eq!(files.iter().find(|f| f.0 == "assets/a.png").unwrap().1, vec![0u8, 1, 2]);
        for bad in ["../evil.slint", "/abs.slint", "a/../b.slint", "C:x.slint", "autre.exe", "assets\\x.png", "sous//x.slint"] {
            let b = serde_json::json!({ "id": "meteo", "files": { "plugin.json": "{\"id\":\"meteo\"}", bad: "x" } });
            assert!(parse_bundle(b.to_string().as_bytes(), "meteo").is_err(), "{bad}");
        }
        let other = serde_json::json!({ "id": "meteo", "files": { "plugin.json": "{\"id\":\"autre\"}" } });
        assert!(parse_bundle(other.to_string().as_bytes(), "meteo").is_err());
        assert!(parse_bundle(ok.to_string().as_bytes(), "autre").is_err());
    }

    #[test]
    fn base64_and_versions() {
        assert_eq!(base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64("aGVsbG8h").unwrap(), b"hello!");
        assert!(base64("a").is_none());
        assert!(newer("1.2.0", "1.1.9") && !newer("1.0.0", "1.0.0") && !newer("x", "1.0.0"));
    }
}
