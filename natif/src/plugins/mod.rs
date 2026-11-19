//! Plugins : dossiers `%APPDATA%\Horizon\plugins\<id>\` chargés à chaud (widget et panneau en
//! Slint interprété, logique en JavaScript). Conception :
//! `.superpowers/docs/specs/2026-11-19-plugins-design.md`.
//!
//! Un fil relève le dossier toutes les 500 ms (date de modification la plus récente de chaque
//! sous-dossier) ; un changement est pris après 300 ms de calme (`registry`). Un plugin vu pour
//! la première fois, ou qui demande de nouvelles permissions, attend l'accord de l'utilisateur
//! (page État) ; rien n'est chargé avant.

mod registry;
mod store;

pub use registry::{Entry, PluginState};
pub use store::Disabled;

use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{lock, start_thread};
use crate::widgets::Manifest;
use horizon_core::plugin::{PluginManifest, kind_of};
use registry::{Registry, Snapshot, Watcher};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Le registre des plugins a changé (relire [`Plugins::list`]).
pub const EV_PLUGINS: &str = "horizon://plugins";

/// Intervalle des relevés du dossier.
const SCAN: Duration = Duration::from_millis(500);

/// Dossier des plugins (`HORIZON_PLUGINS_DIR` le remplace pour les essais), créé au besoin.
pub fn plugins_dir() -> PathBuf {
    let dir = std::env::var_os("HORIZON_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::state::app_dir().join("plugins"));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("{} : création impossible : {e}", dir.display());
    }
    dir
}

/// État des plugins (géré par l'application).
#[derive(Default)]
pub struct Plugins {
    registry: Mutex<Registry>,
}

/// Ce que montre la page État pour un plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub id: String,
    pub manifest: Option<Arc<PluginManifest>>,
    pub dir: PathBuf,
    pub state: PluginState,
    pub error: Option<String>,
}

impl Plugins {
    /// Manifeste du plugin `id` s'il est chargeable (valide, accepté, activé).
    pub fn manifest(&self, id: &str) -> Option<Arc<PluginManifest>> {
        let r = lock(&self.registry);
        r.entries.get(id).filter(|e| e.state == PluginState::Ok).and_then(|e| e.manifest.clone())
    }

    /// Tous les plugins connus.
    pub fn list(&self) -> Vec<PluginInfo> {
        let r = lock(&self.registry);
        r.entries
            .iter()
            .map(|(id, e)| PluginInfo {
                id: id.clone(),
                manifest: e.manifest.clone(),
                dir: e.dir.clone(),
                state: e.state.clone(),
                error: e.error.clone(),
            })
            .collect()
    }

    /// Active ou désactive un plugin (`reason` : pourquoi il est désactivé ; `None` = activé).
    pub fn set_disabled(&self, app: &AppHandle, id: &str, reason: Option<Disabled>) {
        self.edit_store(app, |r| r.store.entry(id.to_owned()).or_default().disabled = reason);
    }

    /// L'utilisateur accepte les permissions actuelles du plugin `id`.
    pub fn accept(&self, app: &AppHandle, id: &str) {
        self.edit_store(app, |r| {
            if let Some(m) = r.entries.get(id).and_then(|e| e.manifest.clone()) {
                r.store.entry(id.to_owned()).or_default().accepted = m.permissions.clone();
            }
        });
    }

    fn edit_store(&self, app: &AppHandle, f: impl FnOnce(&mut Registry)) {
        {
            let mut r = lock(&self.registry);
            f(&mut r);
            r.refresh_states();
            if let Err(e) = store::save(&store::path(), &r.store) {
                log::error!("plugins.json : écriture impossible : {e:#}");
            }
            publish(&r);
        }
        changed(app);
    }
}

/// Manifestes de widget des plugins chargeables, par genre (`plugin:<id>`), pour
/// `widgets::manifest` (appelé de tous les fils).
/// Avec le manifeste : le plugin a un panneau.
static WIDGETS: RwLock<BTreeMap<String, (&'static Manifest, bool)>> = RwLock::new(BTreeMap::new());

/// Manifeste de widget du genre `kind` s'il est celui d'un plugin chargeable.
pub fn widget_manifest(kind: &str) -> Option<&'static Manifest> {
    WIDGETS.read().unwrap_or_else(|e| e.into_inner()).get(kind).map(|w| w.0)
}

/// Le genre `kind` est celui d'un plugin chargeable qui a un panneau.
pub fn has_panel(kind: &str) -> bool {
    WIDGETS.read().unwrap_or_else(|e| e.into_inner()).get(kind).is_some_and(|w| w.1)
}

/// Genres de widget des plugins chargeables (menu « Ajouter un widget »).
pub fn widget_kinds() -> Vec<&'static Manifest> {
    WIDGETS.read().unwrap_or_else(|e| e.into_inner()).values().map(|w| w.0).collect()
}

/// `Manifest` de widget (textes `'static`) pour un manifeste de plugin. Chaque contenu distinct
/// n'est alloué qu'une fois : recharger un plugin inchangé ne fuit rien.
fn widget_of(m: &PluginManifest) -> Option<&'static Manifest> {
    type Key = (String, String, String, Option<u64>, Option<u16>, bool);
    static MADE: Mutex<BTreeMap<Key, &'static Manifest>> = Mutex::new(BTreeMap::new());
    let w = m.widget.as_ref()?;
    let icon = m.icon.clone().unwrap_or_else(|| "squaresFour".to_owned());
    let key = (kind_of(&m.id), m.name.clone(), icon, w.squeeze, w.grows, w.clickable_on_hover);
    let mut made = lock(&MADE);
    Some(*made.entry(key.clone()).or_insert_with(|| {
        let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
        Box::leak(Box::new(Manifest {
            id: leak(key.0),
            label: leak(key.1),
            icon: leak(key.2),
            squeeze: key.3,
            clickable_on_hover: key.5,
            grows: key.4,
            personal: false,
            personal_settings: false,
        }))
    }))
}

/// Recopie les manifestes de widget des plugins chargeables dans [`WIDGETS`].
fn publish(r: &Registry) {
    let table: BTreeMap<String, (&'static Manifest, bool)> = r
        .entries
        .values()
        .filter(|e| e.state == PluginState::Ok)
        .filter_map(|e| e.manifest.as_deref().and_then(|m| Some((widget_of(m)?, m.panel))))
        .map(|(w, panel)| (w.id.to_owned(), (w, panel)))
        .collect();
    *WIDGETS.write().unwrap_or_else(|e| e.into_inner()) = table;
}

/// Le registre a changé : les barres refont leurs emplacements, la page État se relit.
fn changed(app: &AppHandle) {
    let a = app.clone();
    let _ = slint::invoke_from_event_loop(move || crate::edit::broadcast_layout(&a));
    let _ = app.emit(EV_PLUGINS, ());
}

/// Date de modification la plus récente (ms) de chaque sous-dossier de `root` (ses fichiers et
/// ceux de `assets/`).
fn snapshot(root: &Path) -> Snapshot {
    let newest = |dir: &Path| -> u64 {
        let files = std::fs::read_dir(dir).into_iter().flatten().flatten();
        let assets = std::fs::read_dir(dir.join("assets")).into_iter().flatten().flatten();
        files
            .chain(assets)
            .filter_map(|f| f.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .max()
            .unwrap_or(0)
    };
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, newest(&e.path()))))
        .collect()
}

/// Identifiants des widgets intégrés (interdits aux plugins).
fn builtin() -> Vec<&'static str> {
    crate::widgets::MANIFESTS.iter().map(|m| m.id).collect()
}

/// Lit le dossier et l'accord, puis le surveille.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let root = plugins_dir();
    let (mut watcher, first) = Watcher::start(snapshot(&root));
    let builtin = builtin();
    {
        let plugins = app.state::<Plugins>();
        let mut r = lock(&plugins.registry);
        r.store = store::load(&store::path());
        for c in &first {
            r.apply(&root, c, &builtin);
        }
        publish(&r);
        log::info!("plugins : {} dans {}", r.entries.len(), root.display());
    }
    if !first.is_empty() {
        changed(app);
    }
    let a = app.clone();
    start_thread("plugins-dossier", move || {
        let t0 = Instant::now();
        loop {
            std::thread::sleep(SCAN);
            let changes = watcher.step(&snapshot(&root), t0.elapsed().as_millis() as u64);
            if changes.is_empty() {
                continue;
            }
            {
                let plugins = a.state::<Plugins>();
                let mut r = lock(&plugins.registry);
                for c in &changes {
                    log::info!("plugins : {c:?}");
                    r.apply(&root, c, &builtin);
                    if let Some(e) = changes_error(&r, c) {
                        log::warn!("plugin {e}");
                    }
                }
                publish(&r);
            }
            changed(&a);
        }
    })
}

/// Erreur de lecture du plugin touché par `c`, pour le journal.
fn changes_error(r: &Registry, c: &registry::Change) -> Option<String> {
    let (registry::Change::Added(n) | registry::Change::Modified(n)) = c else { return None };
    r.entries.get(n).and_then(|e| e.error.as_ref()).map(|e| format!("{n} : {e}"))
}
