//! Briques communes des modules : verrou sans empoisonnement, valeur publiée aux fenêtres,
//! réveil d'un fil, cache selon la date d'un fichier, fils nommés.
//!
//! Repris de `app/src-tauri/src/util.rs` : `use crate::host` au lieu de `use tauri`, charge de
//! `Published` envoyable entre fils, pas d'URL `data:` (les images vont à Slint telles quelles).

use serde::Serialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime};
use crate::host::{AppHandle, Emitter};

/// Prend un verrou ; un fil qui a paniqué en le tenant ne bloque pas les autres (l'état gardé
/// reste celui qu'il a laissé, toujours cohérent ici : chaque modification est complète).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Démarre un fil nommé (nom visible dans les outils de diagnostic). Un échec est journalisé :
/// la fonction ne démarre pas, Horizon continue.
pub fn spawn_named(name: &str, f: impl FnOnce() + Send + 'static) -> bool {
    match std::thread::Builder::new().name(name.into()).spawn(f) {
        Ok(_) => true,
        Err(e) => {
            log::error!("fil {name} : démarrage impossible : {e}");
            false
        }
    }
}

/// Démarre le fil d'une fonction d'Horizon ; une erreur si c'est impossible (la table de
/// démarrage la journalise et passe à la suite).
pub fn start_thread(name: &str, f: impl FnOnce() + Send + 'static) -> anyhow::Result<()> {
    std::thread::Builder::new().name(name.into()).spawn(f).map(|_| ()).map_err(|e| anyhow::anyhow!("fil {name} : {e}"))
}

/// Valeur envoyée aux fenêtres par l'événement `event` (à toutes, ou à une seule) ; seuls les
/// changements partent. La dernière valeur sert aux fenêtres qui s'ouvrent (`get`).
pub struct Published<T> {
    event: &'static str,
    /// Fenêtre destinataire ; `None` = toutes.
    target: Option<&'static str>,
    value: Mutex<T>,
}

impl<T: Clone + PartialEq + Serialize + Send + Sync + 'static> Published<T> {
    pub const fn new(event: &'static str, initial: T) -> Published<T> {
        Published { event, target: None, value: Mutex::new(initial) }
    }

    /// Valeur envoyée à la seule fenêtre `label`.
    pub const fn to(event: &'static str, label: &'static str, initial: T) -> Published<T> {
        Published { event, target: Some(label), value: Mutex::new(initial) }
    }

    pub fn get(&self) -> T {
        lock(&self.value).clone()
    }

    /// Remplace la valeur ; l'envoie si elle a changé (verrou relâché avant l'envoi). Renvoie
    /// vrai si elle a changé.
    pub fn set(&self, app: &AppHandle, next: T) -> bool {
        self.update(app, |v| *v = next)
    }

    /// Modifie la valeur en place ; l'envoie si elle a changé.
    pub fn update(&self, app: &AppHandle, f: impl FnOnce(&mut T)) -> bool {
        let changed = {
            let mut v = lock(&self.value);
            let before = v.clone();
            f(&mut v);
            (*v != before).then(|| v.clone())
        };
        match changed {
            Some(v) => {
                self.emit(app, &v);
                true
            }
            None => false,
        }
    }

    fn emit(&self, app: &AppHandle, v: &T) {
        let _ = match self.target {
            Some(label) => app.emit_to(label, self.event, v.clone()),
            None => app.emit(self.event, v.clone()),
        };
    }
}

/// Réveil d'un fil qui dort entre deux tâches périodiques (« recalcule tout de suite »).
pub struct Waker {
    tx: crossbeam_channel::Sender<()>,
    rx: crossbeam_channel::Receiver<()>,
}

impl Default for Waker {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Waker { tx, rx }
    }
}

impl Waker {
    pub fn wake(&self) {
        let _ = self.tx.send(());
    }

    /// Côté du fil réveillé (pour `select!`).
    pub fn receiver(&self) -> &crossbeam_channel::Receiver<()> {
        &self.rx
    }

    /// Dort au plus `d` ; vrai si réveillé. Les réveils accumulés entre-temps sont consommés.
    pub fn wait(&self, d: Duration) -> bool {
        let woken = self.rx.recv_timeout(d).is_ok();
        while self.rx.try_recv().is_ok() {}
        woken
    }
}

/// Contenu tiré d'un fichier, relu seulement quand sa date de modification change (fichier
/// modifiable à la main ou écrit par un autre programme).
pub struct MtimeCache<T> {
    slot: Mutex<Option<(Option<SystemTime>, T)>>,
}

impl<T> Default for MtimeCache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> MtimeCache<T> {
    pub const fn new() -> MtimeCache<T> {
        MtimeCache { slot: Mutex::new(None) }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl<T: Clone> MtimeCache<T> {
    /// Contenu du fichier : celui gardé si la date n'a pas bougé (fichier absent compris),
    /// sinon relu par `read`. Une lecture ratée (`None`) n'est pas gardée : elle sera retentée.
    pub fn get(&self, path: &Path, read: impl FnOnce() -> Option<T>) -> Option<T> {
        self.read_if_changed(path, read).or_else(|| self.last())
    }

    /// Contenu relu, seulement si la date a changé depuis la dernière lecture (sinon `None`).
    pub fn fresh(&self, path: &Path, read: impl FnOnce() -> Option<T>) -> Option<T> {
        self.read_if_changed(path, read)
    }

    /// Dernier contenu lu, sans regarder le fichier.
    pub fn last(&self) -> Option<T> {
        lock(&self.slot).as_ref().map(|(_, v)| v.clone())
    }

    /// Oublie la date vue : la prochaine demande relit le fichier.
    pub fn forget(&self) {
        if let Some((seen, _)) = lock(&self.slot).as_mut() {
            *seen = Some(SystemTime::UNIX_EPOCH);
        }
    }

    fn read_if_changed(&self, path: &Path, read: impl FnOnce() -> Option<T>) -> Option<T> {
        let mut slot = lock(&self.slot);
        let now = mtime(path);
        if slot.as_ref().is_some_and(|(seen, _)| *seen == now) {
            return None;
        }
        let v = read()?;
        // Date relue : le fichier peut avoir été créé par la lecture (réglages par défaut).
        *slot = Some((mtime(path), v.clone()));
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mtime_cache_rereads_only_after_a_change() {
        let d = Dir(std::env::temp_dir().join(format!("horizon-util-mtime-{}", std::process::id())));
        std::fs::create_dir_all(&d.0).unwrap();
        let p = d.0.join("f.txt");
        let cache: MtimeCache<String> = MtimeCache::default();
        let read = || std::fs::read_to_string(&p).ok();
        assert_eq!(cache.get(&p, read), None, "fichier absent, lecture ratée : rien de gardé");
        std::fs::write(&p, "a").unwrap();
        assert_eq!(cache.get(&p, read).as_deref(), Some("a"));
        assert_eq!(cache.fresh(&p, || panic!("pas de relecture sans changement")), None);
        assert_eq!(cache.get(&p, || panic!("pas de relecture sans changement")).as_deref(), Some("a"));
        cache.forget();
        assert_eq!(cache.fresh(&p, || Some("b".to_owned())).as_deref(), Some("b"));
        // Lecture ratée après un changement : la dernière valeur reste servie.
        cache.forget();
        assert_eq!(cache.get(&p, || None).as_deref(), Some("b"));
    }

    #[test]
    fn waker_consumes_pending_wakes() {
        let w = Waker::default();
        assert!(!w.wait(Duration::from_millis(1)));
        w.wake();
        w.wake();
        assert!(w.wait(Duration::from_millis(1)));
        assert!(!w.wait(Duration::from_millis(1)), "les réveils accumulés sont consommés");
    }

    #[test]
    fn lock_survives_poisoning() {
        let m = std::sync::Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("empoisonne");
        })
        .join();
        assert_eq!(*lock(&m), 1);
    }
}
