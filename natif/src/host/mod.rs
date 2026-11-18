//! Hôte compatible : la petite part de l'API de Tauri dont se servent les modules de services
//! d'Horizon (`app/src-tauri/src`), pour les reprendre presque tels quels. Seuls les `use
//! tauri::…` deviennent `use crate::host::…` (liste des différences : `natif/README.md`).
//!
//! - [`AppHandle`] : clonable, envoyable entre fils ; états partagés ([`Manager`]), fenêtres
//!   par étiquette, fil de l'interface (`run_on_main_thread`), sortie.
//! - [`Emitter`] : événements à charge serde, livrés sur le fil de l'interface aux abonnés
//!   ([`listen`]) qui mettent à jour les propriétés et modèles Slint.

mod events;

pub use events::{Event, ListenerId, listen, scoped, unlisten, unlisten_scope, unlisten_window};

use serde::Serialize;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Default)]
struct Inner {
    states: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    /// Fenêtres ouvertes : étiquette → HWND.
    windows: Mutex<HashMap<String, isize>>,
}

/// Poignée de l'application (équivalent de `tauri::AppHandle`).
#[derive(Clone, Default)]
pub struct AppHandle(Arc<Inner>);

/// État partagé emprunté (équivalent de `tauri::State`) : se lit comme `&T`, et `inner()` le
/// prête pour toute la durée de l'emprunt de l'`AppHandle` (comme Tauri : `fn f(app:
/// &AppHandle) -> &X { &app.state::<S>().inner().x }` est permis).
pub struct State<'a, T: Send + Sync + 'static>(&'a T);

impl<'a, T: Send + Sync + 'static> State<'a, T> {
    pub fn inner(&self) -> &'a T {
        self.0
    }
}

impl<T: Send + Sync + 'static> std::ops::Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
    }
}

/// États partagés (équivalent de `tauri::Manager`).
pub trait Manager {
    /// Enregistre l'état `T` ; faux s'il y en avait déjà un (gardé, comme Tauri).
    fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool;
    /// État `T` ; panique s'il n'a pas été enregistré (comme Tauri).
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T>;
    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>>;
    /// Fenêtre ouverte d'étiquette `label` (équivalent de `get_webview_window`).
    fn get_window(&self, label: &str) -> Option<Window>;
}

impl Manager for AppHandle {
    fn manage<T: Send + Sync + 'static>(&self, state: T) -> bool {
        let mut states = self.0.states.write().unwrap_or_else(|e| e.into_inner());
        if states.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        states.insert(TypeId::of::<T>(), Arc::new(state));
        true
    }

    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state().unwrap_or_else(|| panic!("état {} non enregistré (manage)", std::any::type_name::<T>()))
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let states = self.0.states.read().unwrap_or_else(|e| e.into_inner());
        let any = states.get(&TypeId::of::<T>())?.clone();
        let arc: Arc<T> = any.downcast::<T>().ok()?;
        // SAFETY : un état enregistré n'est jamais retiré ni remplacé (`manage` garde le
        // premier) ; son `Arc` reste dans la table tant que vit `Inner`, que l'emprunt de
        // `self` (une poignée `AppHandle`) garde en vie. La valeur, sur le tas, ne bouge pas
        // quand la table grandit : la référence vaut pour toute la durée de l'emprunt.
        let value: &T = unsafe { &*Arc::as_ptr(&arc) };
        Some(State(value))
    }

    fn get_window(&self, label: &str) -> Option<Window> {
        let hwnd = *self.0.windows.lock().unwrap_or_else(|e| e.into_inner()).get(label)?;
        Some(Window { app: self.clone(), label: label.to_owned(), hwnd })
    }
}

impl AppHandle {
    pub fn new() -> AppHandle {
        AppHandle::default()
    }

    /// Exécute `f` sur le fil de l'interface (après ce qui y est déjà en file).
    pub fn run_on_main_thread(&self, f: impl FnOnce() + Send + 'static) -> anyhow::Result<()> {
        slint::invoke_from_event_loop(f).map_err(|e| anyhow::anyhow!("fil de l'interface : {e}"))
    }

    /// Arrête Horizon (fin de la boucle d'événements ; le nettoyage suit dans `run`).
    pub fn exit(&self, code: i32) {
        log::info!("arrêt demandé (code {code})");
        let _ = slint::quit_event_loop();
    }

    /// Fenêtre ouverte (appelé par l'interface à sa création).
    pub fn register_window(&self, label: &str, hwnd: isize) {
        self.0.windows.lock().unwrap_or_else(|e| e.into_inner()).insert(label.to_owned(), hwnd);
    }

    /// Fenêtre fermée : son étiquette et ses abonnements sont oubliés.
    pub fn unregister_window(&self, label: &str) {
        self.0.windows.lock().unwrap_or_else(|e| e.into_inner()).remove(label);
        // Abonnements tenus par le fil de l'interface (ailleurs : aucun, sans effet).
        unlisten_window(label);
    }
}

/// Fenêtre vue d'un autre fil : étiquette et HWND (la fenêtre Slint vit sur le fil de
/// l'interface). Équivalent réduit de `tauri::WebviewWindow`.
#[derive(Clone)]
pub struct Window {
    app: AppHandle,
    label: String,
    hwnd: isize,
}

impl Window {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn app_handle(&self) -> &AppHandle {
        &self.app
    }
}

/// Événements (équivalent de `tauri::Emitter`). La charge est gardée telle quelle (lue par
/// `Event::payload`) et convertible en JSON (`Event::json`).
pub trait Emitter {
    /// À toutes les fenêtres.
    fn emit<S: Serialize + Clone + Send + Sync + 'static>(&self, event: &str, payload: S) -> anyhow::Result<()>;
    /// À la seule fenêtre `label`.
    fn emit_to<S: Serialize + Clone + Send + Sync + 'static>(&self, label: &str, event: &str, payload: S) -> anyhow::Result<()>;
    /// Aux fenêtres dont l'étiquette passe `filter`.
    fn emit_filter<S, F>(&self, event: &str, payload: S, filter: F) -> anyhow::Result<()>
    where
        S: Serialize + Clone + Send + Sync + 'static,
        F: Fn(&str) -> bool + Send + 'static;
}

impl Emitter for AppHandle {
    fn emit<S: Serialize + Clone + Send + Sync + 'static>(&self, event: &str, payload: S) -> anyhow::Result<()> {
        events::send(event, payload, events::Target::All)
    }

    fn emit_to<S: Serialize + Clone + Send + Sync + 'static>(&self, label: &str, event: &str, payload: S) -> anyhow::Result<()> {
        events::send(event, payload, events::Target::Label(label.to_owned()))
    }

    fn emit_filter<S, F>(&self, event: &str, payload: S, filter: F) -> anyhow::Result<()>
    where
        S: Serialize + Clone + Send + Sync + 'static,
        F: Fn(&str) -> bool + Send + 'static,
    {
        events::send(event, payload, events::Target::Filter(Box::new(filter)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter(std::sync::atomic::AtomicU32);

    #[test]
    fn states_are_shared_between_clones() {
        let app = AppHandle::new();
        assert!(app.manage(Counter::default()));
        assert!(!app.manage(Counter::default()), "le premier état reste");
        let other = app.clone();
        other.state::<Counter>().inner().0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(app.state::<Counter>().inner().0.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(app.try_state::<String>().is_none());
    }

    #[test]
    fn windows_by_label() {
        let app = AppHandle::new();
        app.register_window("bar-1-0", 42);
        assert_eq!(app.get_window("bar-1-0").map(|w| w.hwnd()), Some(42));
        app.unregister_window("bar-1-0");
        assert!(app.get_window("bar-1-0").is_none());
    }
}
