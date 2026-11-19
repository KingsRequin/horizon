//! Logique du registre, sans horloge ni disque : changements du dossier des plugins pris après
//! un moment de calme (fichiers en cours d'écriture), et état de chaque plugin selon l'accord
//! donné à ses permissions.

use super::store::{Disabled, StoreEntry};
use horizon_core::plugin::Permissions;
use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;

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
        let entry = StoreEntry { disabled: None, accepted: Permissions::default() };
        assert!(matches!(state_of(&perms(), Some(&entry)), PluginState::NeedsConsent(_)));
    }

    #[test]
    fn same_permissions_stay_accepted() {
        let entry = StoreEntry { disabled: None, accepted: perms() };
        assert_eq!(state_of(&perms(), Some(&entry)), PluginState::Ok);
        let off = StoreEntry { disabled: Some(Disabled::User), accepted: perms() };
        assert_eq!(state_of(&perms(), Some(&off)), PluginState::Disabled(Disabled::User));
    }
}
