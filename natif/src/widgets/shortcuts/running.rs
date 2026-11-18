//! Raccourcis : applis déjà ouvertes (pastille sous l'icône) et ouverture (premier plan si
//! l'appli a une fenêtre, sinon lancement ; Maj+clic = nouvelle instance).
//!
//! Pas de crochet sur chaque création de fenêtre : un relevé léger des fenêtres principales
//! toutes les 2 s, et tout de suite à chaque changement de premier plan (une appli qui
//! s'ouvre passe au premier plan). Les barres ne reçoivent la liste que si elle change.
//!
//! Repris tel quel de `app/src-tauri/src/widgets/shortcuts/running.rs` (hôte au lieu de Tauri,
//! commandes en fonctions ordinaires).

use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use crate::util::{Published, start_thread};
use horizon_core::shortcuts::{all_targets, running_targets};
use horizon_sys::foreground::{target_app_key, window_app_key};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Duration;

/// Cibles de raccourci dont l'appli a une fenêtre ouverte.
pub const EV_RUNNING: &str = "horizon://running";

const REFRESH: Duration = Duration::from_secs(2);

/// Cibles de raccourci dont l'appli a une fenêtre ouverte.
pub struct Running(Published<BTreeSet<String>>);

impl Default for Running {
    fn default() -> Self {
        Running(Published::new(EV_RUNNING, BTreeSet::new()))
    }
}

/// Caches du relevé : clé d'appli de chaque cible et de chaque fenêtre déjà vue.
#[derive(Default)]
struct Scan {
    targets: HashMap<String, Option<String>>,
    windows: HashMap<(isize, u32), Option<String>>,
}

impl Scan {
    fn run(&mut self, targets: &BTreeSet<String>) -> BTreeSet<String> {
        self.targets.retain(|t, _| targets.contains(t));
        for t in targets {
            if !self.targets.contains_key(t) {
                self.targets.insert(t.clone(), target_app_key(t));
            }
        }
        if self.targets.values().all(Option::is_none) {
            return BTreeSet::new();
        }
        let open = horizon_sys::window_focus::main_windows();
        let present: HashSet<(isize, u32)> = open.iter().copied().collect();
        self.windows.retain(|w, _| present.contains(w));
        let mut keys = HashSet::new();
        for w in open {
            let key = match self.windows.get(&w) {
                Some(k) => k.clone(),
                None => {
                    let k = window_app_key(w.0, w.1);
                    // Une fenêtre hôte d'appli UWP ne connaît son AUMID qu'une fois chargée :
                    // pas gardée tant qu'elle se présente comme l'hôte lui-même.
                    if !k.as_deref().is_some_and(|k| k.ends_with("applicationframehost.exe")) {
                        self.windows.insert(w, k.clone());
                    }
                    k
                }
            };
            keys.extend(key);
        }
        running_targets(self.targets.iter().map(|(t, k)| (t.as_str(), k.as_deref())), &keys)
    }
}

/// Relevé sur un fil dédié (les raccourcis `.lnk` et les propriétés de fenêtre sont lus par le
/// fil shell).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let fg_rx = crate::foreground::subscribe(app);
    let app = app.clone();
    start_thread("raccourcis-ouverts", move || {
        let mut scan = Scan::default();
        loop {
            crossbeam_channel::select! {
                recv(fg_rx) -> msg => if msg.is_err() { return },
                default(REFRESH) => {}
            }
            let targets = app.state::<AppState>().read(all_targets);
            let now = scan.run(&targets);
            app.state::<Running>().0.set(&app, now);
        }
    })
}

/// Cibles ouvertes (`shortcuts_running`).
pub fn shortcuts_running(app: &AppHandle) -> BTreeSet<String> {
    app.state::<Running>().0.get()
}

/// Ouvre un raccourci : l'appli déjà ouverte revient au premier plan, sinon elle est lancée.
/// `new_instance` (Maj+clic) lance toujours.
pub fn shortcuts_open(target: String, new_instance: bool) {
    // Sur le fil shell, sans attendre : la fenêtre de la barre reste libre.
    horizon_sys::shell::spawn(move || {
        let focused = !new_instance && target_app_key(&target).is_some_and(|k| horizon_sys::foreground::focus_app(&k));
        if !focused {
            horizon_sys::launch::launch(&target);
        }
    });
}
