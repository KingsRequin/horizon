//! `%APPDATA%\Horizon\plugins.json` : accord donné aux permissions de chaque plugin et
//! désactivation (par l'utilisateur ou par le budget), écrit atomiquement.

use horizon_core::plugin::Permissions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Pourquoi un plugin est désactivé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disabled {
    User,
    TooHeavy,
    Crashing,
}

/// Ce qui est gardé pour un plugin.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreEntry {
    pub disabled: Option<Disabled>,
    /// Permissions acceptées par l'utilisateur.
    pub accepted: Permissions,
    /// Installé depuis la bibliothèque (mis à jour par elle) ; sinon plugin local.
    pub library: bool,
}

/// Contenu du fichier.
pub type Store = BTreeMap<String, StoreEntry>;

/// Chemin du fichier.
pub fn path() -> PathBuf {
    crate::state::app_dir().join("plugins.json")
}

/// Lit le fichier (absent ou illisible : vide, signalé au journal).
pub fn load(path: &Path) -> Store {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("{} illisible : {e}", path.display());
            Store::new()
        }),
        Err(_) => Store::new(),
    }
}

/// Écrit le fichier (fichier temporaire puis renommage).
pub fn save(path: &Path, store: &Store) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(store)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("horizon-plugins-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("plugins.json");
        let mut s = Store::new();
        s.insert("prisme".into(), StoreEntry { disabled: Some(Disabled::TooHeavy), accepted: Permissions::default(), library: true });
        save(&p, &s).unwrap();
        assert_eq!(load(&p), s);
        std::fs::write(&p, "pas du json").unwrap();
        assert!(load(&p).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
