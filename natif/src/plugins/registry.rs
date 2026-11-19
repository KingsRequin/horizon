//! Logique du registre, sans horloge ni disque : changements du dossier des plugins pris après
//! un moment de calme (fichiers en cours d'écriture), et état de chaque plugin selon l'accord
//! donné à ses permissions.

use super::store::{Disabled, Store, StoreEntry};
use horizon_core::plugin::{Permissions, PluginManifest, parse_manifest};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Date de modification la plus récente (ms) des fichiers de chaque dossier de plugin.
pub type Snapshot = BTreeMap<String, u64>;

/// Calme demandé après la dernière modification d'un dossier avant de le relire.
pub const QUIET_MS: u64 = 300;

/// Changement d'un dossier de plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added(String),
    Modified(String),
    Removed(String),
}

/// Suit les dossiers vus.
#[derive(Debug, Default)]
pub struct Watcher {
    /// Ce qui a été pris en compte.
    committed: Snapshot,
    /// Dossier modifié, pas encore pris : (date vue, instant où elle a été vue).
    pending: BTreeMap<String, (u64, u64)>,
}

impl Watcher {
    /// Premier relevé : tout est pris aussitôt (démarrage d'Horizon).
    pub fn start(snap: Snapshot) -> (Watcher, Vec<Change>) {
        let changes = snap.keys().cloned().map(Change::Added).collect();
        (Watcher { committed: snap, pending: BTreeMap::new() }, changes)
    }

    /// Relevé suivant à `now_ms` : les changements à prendre maintenant.
    pub fn step(&mut self, snap: &Snapshot, now_ms: u64) -> Vec<Change> {
        let mut changes = Vec::new();
        // Disparus : pris aussitôt (rien à relire).
        let gone: Vec<String> = self.committed.keys().filter(|k| !snap.contains_key(*k)).cloned().collect();
        for k in gone {
            self.committed.remove(&k);
            changes.push(Change::Removed(k));
        }
        self.pending.retain(|k, _| snap.contains_key(k));
        for (k, &mtime) in snap {
            if self.committed.get(k) == Some(&mtime) {
                self.pending.remove(k);
                continue;
            }
            match self.pending.get(k) {
                Some(&(seen, since)) if seen == mtime => {
                    if now_ms.saturating_sub(since) >= QUIET_MS {
                        self.pending.remove(k);
                        let known = self.committed.insert(k.clone(), mtime).is_some();
                        changes.push(if known { Change::Modified(k.clone()) } else { Change::Added(k.clone()) });
                    }
                }
                _ => {
                    self.pending.insert(k.clone(), (mtime, now_ms));
                }
            }
        }
        changes
    }
}

/// Pourquoi un plugin n'est pas chargé, ou `Ok`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginState {
    Ok,
    /// Permissions (nouvelles) à accepter : phrases de la fenêtre d'accord.
    NeedsConsent(Vec<String>),
    Disabled(Disabled),
    /// Manifeste ou interface invalide (sans version précédente valide).
    Broken,
}

/// État d'un plugin au manifeste valide, d'après ce que l'utilisateur a accepté.
pub fn state_of(perms: &Permissions, entry: Option<&StoreEntry>) -> PluginState {
    if let Some(d) = entry.and_then(|e| e.disabled) {
        return PluginState::Disabled(d);
    }
    let accepted = entry.map(|e| e.accepted.clone()).unwrap_or_default();
    let added = perms.added_since(&accepted);
    if added.is_empty() { PluginState::Ok } else { PluginState::NeedsConsent(added.describe()) }
}

/// Un plugin connu.
#[derive(Debug, Clone)]
pub struct Entry {
    pub dir: PathBuf,
    /// Dernier manifeste valide (gardé quand une nouvelle version est cassée).
    pub manifest: Option<Arc<PluginManifest>>,
    /// Erreur de la dernière lecture (manifeste, puis interface et script aux étapes suivantes).
    pub error: Option<String>,
    pub state: PluginState,
    /// Change à chaque relecture du dossier (l'interface est recompilée).
    pub generation: u64,
}

/// Plugins connus et accords (`plugins.json`).
#[derive(Debug, Default)]
pub struct Registry {
    pub entries: BTreeMap<String, Entry>,
    pub store: Store,
    /// Compteur des relectures (`Entry::generation`).
    next: u64,
}

impl Registry {
    /// Prend en compte un changement du dossier `root`. `builtin` : widgets intégrés.
    pub fn apply(&mut self, root: &Path, change: &Change, builtin: &[&str]) {
        let name = match change {
            Change::Removed(name) => {
                self.entries.remove(name);
                return;
            }
            Change::Added(name) | Change::Modified(name) => name,
        };
        let dir = root.join(name);
        let read = std::fs::read_to_string(dir.join("plugin.json")).map_err(|e| format!("plugin.json illisible : {e}"));
        let parsed = read.and_then(|text| parse_manifest(&text, builtin).map_err(|e| e.to_string())).and_then(|m| {
            if m.id == *name { Ok(m) } else { Err(format!("plugin.json, id : « {} » doit être le nom du dossier (« {name} »)", m.id)) }
        });
        let previous = self.entries.remove(name);
        self.next += 1;
        let generation = self.next;
        let entry = match parsed {
            Ok(m) => {
                let state = state_of(&m.permissions, self.store.get(name));
                Entry { dir, manifest: Some(Arc::new(m)), error: None, state, generation }
            }
            Err(e) => match previous.and_then(|p| p.manifest.map(|m| (m, p.state))) {
                Some((m, state)) => Entry { dir, manifest: Some(m), error: Some(e), state, generation },
                None => Entry { dir, manifest: None, error: Some(e), state: PluginState::Broken, generation },
            },
        };
        self.entries.insert(name.clone(), entry);
    }

    /// Recalcule les états après un changement de `store` (accord, activation).
    pub fn refresh_states(&mut self) {
        for (name, e) in &mut self.entries {
            if let Some(m) = &e.manifest {
                e.state = state_of(&m.permissions, self.store.get(name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_dir(root: &Path, name: &str, json: &str) {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("plugin.json"), json).unwrap();
    }

    const OK: &str = r#"{ "id": "essai", "name": "Essai", "version": "1.0.0", "api": 0 }"#;

    #[test]
    fn broken_manifest_keeps_previous_version() {
        let root = std::env::temp_dir().join(format!("horizon-registry-{}", std::process::id()));
        plugin_dir(&root, "essai", OK);
        let mut r = Registry::default();
        r.apply(&root, &Change::Added("essai".into()), &[]);
        assert_eq!(r.entries["essai"].state, PluginState::Ok);
        plugin_dir(&root, "essai", "{ cassé");
        r.apply(&root, &Change::Modified("essai".into()), &[]);
        let e = &r.entries["essai"];
        assert!(e.manifest.is_some());
        assert!(e.error.as_deref().is_some_and(|m| m.starts_with("plugin.json")), "{:?}", e.error);
        assert_eq!(e.state, PluginState::Ok);
        plugin_dir(&root, "autre", "{ cassé");
        r.apply(&root, &Change::Added("autre".into()), &[]);
        assert_eq!(r.entries["autre"].state, PluginState::Broken);
        r.apply(&root, &Change::Removed("essai".into()), &[]);
        assert!(!r.entries.contains_key("essai"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn id_must_match_dir() {
        let root = std::env::temp_dir().join(format!("horizon-registry-id-{}", std::process::id()));
        plugin_dir(&root, "autre-nom", OK);
        let mut r = Registry::default();
        r.apply(&root, &Change::Added("autre-nom".into()), &[]);
        assert_eq!(r.entries["autre-nom"].state, PluginState::Broken);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn snap(items: &[(&str, u64)]) -> Snapshot {
        items.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    }

    #[test]
    fn start_takes_everything() {
        let (_, changes) = Watcher::start(snap(&[("a", 1), ("b", 2)]));
        assert_eq!(changes, [Change::Added("a".into()), Change::Added("b".into())]);
    }

    #[test]
    fn new_dir_is_added_after_quiet_period() {
        let (mut w, _) = Watcher::start(snap(&[]));
        let s = snap(&[("a", 5)]);
        assert!(w.step(&s, 1_000).is_empty());
        assert!(w.step(&s, 1_299).is_empty());
        assert_eq!(w.step(&s, 1_300), [Change::Added("a".into())]);
        assert!(w.step(&s, 2_000).is_empty());
    }

    #[test]
    fn change_during_write_waits() {
        let (mut w, _) = Watcher::start(snap(&[("a", 1)]));
        assert!(w.step(&snap(&[("a", 2)]), 1_000).is_empty());
        assert!(w.step(&snap(&[("a", 3)]), 1_200).is_empty());
        assert!(w.step(&snap(&[("a", 3)]), 1_400).is_empty());
        assert_eq!(w.step(&snap(&[("a", 3)]), 1_500), [Change::Modified("a".into())]);
    }

    #[test]
    fn removed_dir_is_removed() {
        let (mut w, _) = Watcher::start(snap(&[("a", 1)]));
        assert_eq!(w.step(&snap(&[]), 1_000), [Change::Removed("a".into())]);
        assert!(w.step(&snap(&[]), 2_000).is_empty());
    }

    fn perms() -> Permissions {
        Permissions { connect: vec!["ws://127.0.0.1:28650".into()], ..Default::default() }
    }

    #[test]
    fn first_seen_needs_consent() {
        assert_eq!(
            state_of(&perms(), None),
            PluginState::NeedsConsent(vec!["Se connecter à une application de ce PC (127.0.0.1:28650)".into()])
        );
        assert_eq!(state_of(&Permissions::default(), None), PluginState::Ok);
    }

    #[test]
    fn new_permission_needs_consent_again() {
        let entry = StoreEntry { disabled: None, accepted: Permissions::default(), library: false };
        assert!(matches!(state_of(&perms(), Some(&entry)), PluginState::NeedsConsent(_)));
    }

    #[test]
    fn same_permissions_stay_accepted() {
        let entry = StoreEntry { disabled: None, accepted: perms(), library: false };
        assert_eq!(state_of(&perms(), Some(&entry)), PluginState::Ok);
        let off = StoreEntry { disabled: Some(Disabled::User), accepted: perms(), library: false };
        assert_eq!(state_of(&perms(), Some(&off)), PluginState::Disabled(Disabled::User));
    }
}
