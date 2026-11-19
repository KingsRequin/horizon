//! Bibliothèque de plugins (dépôt `KingsRequin/horizon-plugins`) : index relu au démarrage puis
//! toutes les 6 h (gardé dans `%APPDATA%\Horizon\library.json`), installation, mise à jour et
//! retrait. Un paquet est vérifié (taille, SHA-256 de l'index, chemins) avant d'être écrit, puis
//! mis en place d'un coup (renommage) : Horizon ne voit jamais un plugin à moitié écrit. Les
//! plugins venus de la bibliothèque se mettent à jour d'eux-mêmes ; une nouvelle permission
//! redemande l'accord (registre). La page Plugins (réglages) s'appuie sur [`tiles`].

use super::{Plugins, PluginState};
use crate::host::{AppHandle, Emitter, Manager};
use crate::util::lock;
use horizon_core::plugin::library::{self, Entry};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

/// Relecture de l'index.
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// Première lecture après le démarrage.
const FIRST: Duration = Duration::from_secs(90);

/// État d'une tuile de la page Plugins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileState {
    Installed,
    Update,
    NeedsConsent,
    Disabled,
    Broken,
    Available,
}

/// Un plugin, installé ou proposé par la bibliothèque.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    pub id: String,
    pub name: String,
    pub author: Option<String>,
    pub description: Option<String>,
    /// Version installée.
    pub installed: Option<String>,
    /// Version de la bibliothèque.
    pub available: Option<String>,
    pub icon: String,
    /// Phrases des permissions (celles de la version de la bibliothèque si elle est plus récente).
    pub permissions: Vec<String>,
    pub state: TileState,
    /// Octets du paquet.
    pub size: Option<u64>,
    pub error: Option<String>,
    /// Part d'un cœur (installés).
    pub load: f32,
}

#[derive(Default)]
struct Cache {
    entries: Vec<Entry>,
    loaded: bool,
    error: Option<String>,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache { entries: Vec::new(), loaded: false, error: None });

fn cache_path() -> PathBuf {
    crate::state::app_dir().join("library.json")
}

/// Index gardé (lu une fois sur le disque).
fn entries() -> Vec<Entry> {
    let mut c = lock(&CACHE);
    if !c.loaded {
        c.loaded = true;
        if let Ok(text) = std::fs::read_to_string(cache_path()) {
            c.entries = library::parse_index(&text).unwrap_or_default();
        }
    }
    c.entries.clone()
}

/// Dernière erreur de lecture de l'index (page Plugins).
pub fn last_error() -> Option<String> {
    lock(&CACHE).error.clone()
}

/// Plugins installés et proposés, pour la page Plugins (ne bloque pas).
pub fn tiles(app: &AppHandle) -> Vec<Tile> {
    let plugins = app.state::<Plugins>();
    let index = entries();
    let mut out: Vec<Tile> = plugins
        .list()
        .into_iter()
        .map(|p| {
            let m = p.manifest.as_deref();
            let entry = index.iter().find(|e| e.id == p.id);
            let installed = m.map(|m| m.version.clone());
            let update = entry.zip(installed.as_deref()).is_some_and(|(e, v)| library::newer(&e.version, v));
            let state = match &p.state {
                PluginState::Ok if update => TileState::Update,
                PluginState::Ok => TileState::Installed,
                PluginState::NeedsConsent(_) => TileState::NeedsConsent,
                PluginState::Disabled(_) => TileState::Disabled,
                PluginState::Broken => TileState::Broken,
            };
            let permissions = match (&p.state, entry) {
                (PluginState::NeedsConsent(asks), _) => asks.clone(),
                (_, Some(e)) if update => e.permissions.describe(),
                _ => m.map(|m| m.permissions.describe()).unwrap_or_default(),
            };
            Tile {
                id: p.id.clone(),
                name: m.map_or_else(|| p.id.clone(), |m| m.name.clone()),
                author: m.and_then(|m| m.author.clone()),
                description: m.and_then(|m| m.description.clone()),
                installed,
                available: entry.map(|e| e.version.clone()),
                icon: m.and_then(|m| m.icon.clone()).unwrap_or_else(|| "squaresFour".into()),
                permissions,
                state,
                size: entry.map(|e| e.size),
                error: p.error.clone().or_else(|| plugins.last_crash(&p.id)),
                load: plugins.load(&p.id),
            }
        })
        .collect();
    let missing: Vec<&Entry> = index.iter().filter(|e| !out.iter().any(|t| t.id == e.id)).collect();
    for e in missing {
        out.push(Tile {
            id: e.id.clone(),
            name: e.name.clone(),
            author: e.author.clone(),
            description: e.description.clone(),
            installed: None,
            available: Some(e.version.clone()),
            icon: e.icon.clone().unwrap_or_else(|| "squaresFour".into()),
            permissions: e.permissions.describe(),
            state: TileState::Available,
            size: Some(e.size),
            error: None,
            load: 0.0,
        });
    }
    out
}

/// Relit l'index sur GitHub (bloquant) et le garde.
fn fetch() -> Result<Vec<Entry>, String> {
    let text = crate::web::get(library::INDEX, library::MAX_INDEX)?;
    let list = library::parse_index(&text).map_err(|e| format!("index illisible : {e}"))?;
    let _ = std::fs::write(cache_path(), &text);
    Ok(list)
}

/// Relit l'index en arrière-plan, met à jour les plugins venus de la bibliothèque, puis prévient
/// la page Plugins (`EV_PLUGINS`).
pub fn refresh(app: &AppHandle) {
    let a = app.clone();
    crate::util::spawn_named("plugins-bibliotheque", move || refresh_now(&a));
}

fn refresh_now(app: &AppHandle) {
    let result = fetch();
    {
        let mut c = lock(&CACHE);
        c.loaded = true;
        match &result {
            Ok(list) => {
                c.entries = list.clone();
                c.error = None;
            }
            Err(e) => c.error = Some(e.clone()),
        }
    }
    if let Err(e) = &result {
        log::warn!("bibliothèque de plugins : {e}");
    }
    // Mises à jour des plugins installés depuis la bibliothèque.
    let plugins = app.state::<Plugins>();
    for t in tiles(app).into_iter().filter(|t| t.state == TileState::Update && plugins.from_library(&t.id)) {
        match install(app, &t.id) {
            Ok(()) => log::info!("plugin {} mis à jour ({})", t.id, t.available.unwrap_or_default()),
            Err(e) => log::warn!("plugin {} : mise à jour impossible : {e}", t.id),
        }
    }
    let _ = app.emit(super::EV_PLUGINS, ());
}

/// Installe (ou met à jour) le plugin `id` de la bibliothèque : bloquant (hors du fil de
/// l'interface). S'il demande des permissions, il attend ensuite l'accord.
pub fn install(app: &AppHandle, id: &str) -> Result<(), String> {
    let version = install_files(id)?;
    app.state::<Plugins>().set_from_library(app, id);
    log::info!("plugin {id} {version} installé depuis la bibliothèque");
    Ok(())
}

/// Télécharge, vérifie et met en place les fichiers du plugin `id` ; sa version.
fn install_files(id: &str) -> Result<String, String> {
    let entry = match entries().into_iter().find(|e| e.id == id) {
        Some(e) => e,
        None => fetch()?.into_iter().find(|e| e.id == id).ok_or("plugin absent de la bibliothèque")?,
    };
    let bytes = crate::web::get_bytes(&entry.url, library::MAX_BUNDLE)?;
    if bytes.len() as u64 != entry.size || horizon_core::update::sha256_hex(&bytes) != entry.sha256.to_ascii_lowercase() {
        return Err("paquet altéré (taille ou empreinte différente de l'index)".into());
    }
    let files = library::parse_bundle(&bytes, id).map_err(|e| format!("paquet refusé : {e}"))?;
    put_in_place(&super::plugins_dir(), id, &files).map_err(|e| format!("installation : {e:#}"))?;
    Ok(entry.version)
}

/// Ligne de commande `--install-plugin <id>` : installe depuis la bibliothèque (Horizon, s'il
/// tourne, le voit aussitôt ; l'accord des permissions se donne ensuite dans Horizon).
pub fn cli(id: &str) -> i32 {
    match install_files(id) {
        Ok(version) => {
            let path = super::store::path();
            let mut s = super::store::load(&path);
            s.entry(id.to_owned()).or_default().library = true;
            let _ = super::store::save(&path, &s);
            println!("{id} {version} installé dans {}", super::plugins_dir().join(id).display());
            0
        }
        Err(e) => {
            println!("{id} : {e}");
            1
        }
    }
}

/// Met à jour le plugin `id` (comme [`install`]).
pub fn update(app: &AppHandle, id: &str) -> Result<(), String> {
    install(app, id)
}

/// Écrit les fichiers dans un dossier à part, puis remplace l'ancien dossier d'un coup.
fn put_in_place(root: &Path, id: &str, files: &[(String, Vec<u8>)]) -> anyhow::Result<()> {
    let dir = root.join(id);
    let tmp = root.join(format!(".{id}.nouveau"));
    let old = root.join(format!(".{id}.ancien"));
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&old);
    for (rel, data) in files {
        let p = tmp.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, data)?;
    }
    if dir.exists() {
        std::fs::rename(&dir, &old)?;
    }
    if let Err(e) = std::fs::rename(&tmp, &dir) {
        // Remis comme avant.
        let _ = std::fs::rename(&old, &dir);
        return Err(e.into());
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Retire le plugin `id` : son dossier, son accord et son origine.
pub fn remove(app: &AppHandle, id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err("identifiant invalide".into());
    }
    let dir = super::plugins_dir().join(id);
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| "plugin introuvable".to_owned())?;
    if meta.file_type().is_symlink() {
        // Un lien : on retire le lien, jamais ce vers quoi il mène.
        std::fs::remove_dir(&dir).or_else(|_| std::fs::remove_file(&dir)).map_err(|e| e.to_string())?;
    } else {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    app.state::<Plugins>().forget(app, id);
    log::info!("plugin {id} retiré");
    Ok(())
}

/// Lecture de l'index au démarrage puis toutes les 6 h.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    crate::util::start_thread("plugins-bibliotheque", move || {
        std::thread::sleep(FIRST);
        loop {
            refresh_now(&a);
            std::thread::sleep(EVERY);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_in_place_replaces_the_whole_folder() {
        let root = std::env::temp_dir().join(format!("horizon-biblio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("meteo")).unwrap();
        std::fs::write(root.join("meteo/ancien.slint"), "x").unwrap();
        let files = vec![("plugin.json".to_owned(), b"{}".to_vec()), ("assets/a.png".to_owned(), vec![1, 2])];
        put_in_place(&root, "meteo", &files).unwrap();
        assert!(root.join("meteo/plugin.json").exists() && root.join("meteo/assets/a.png").exists());
        assert!(!root.join("meteo/ancien.slint").exists());
        assert!(!root.join(".meteo.nouveau").exists() && !root.join(".meteo.ancien").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
