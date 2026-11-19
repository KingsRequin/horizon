//! Palette de commandes : raccourci clavier global (`[palette]` de `settings.toml`, défaut
//! Ctrl+Alt+P, `RegisterHotKey` comme la version Tauri) qui ouvre, sur l'écran de la souris, une
//! fenêtre créée au démarrage et seulement cachée ensuite (ouverture immédiate). Sources :
//! raccourcis des barres, suggestions, actions, profils, entrées des plugins, sorties audio et micros,
//! hôtes du homelab, fenêtres ouvertes, applis du menu Démarrer. Classement flou dans
//! `horizon_core::palette` ; perte du focus = fermeture.
//!
//! Repris de `app/src-tauri/src/palette/` : `catalogue` (les entrées), `window` (raccourci,
//! ouverture et fermeture) ; la fenêtre est menée par `ui::palette` sur le fil de l'interface.
//! Différence : un essai (`--palette-essai`) ouvre la palette sans qu'aucune action ne s'exécute (journalisée).

mod catalogue;
mod window;

pub use catalogue::IconRef;
pub use window::{hide, hotkey_status, open, open_essai, set_hotkey, start, toggle};
pub(crate) use window::deactivated;

use crate::host::{AppHandle, Manager};
use crate::util::{lock, spawn_named};
use horizon_core::geometry::Rect;
use horizon_core::palette::{Candidate, Usage, rank};
use horizon_sys::startmenu::StartApp;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use catalogue::{Entry, run_action};

/// Étiquette de la fenêtre.
pub const LABEL: &str = "palette";
/// Résultats affichés au plus.
const LIMIT: usize = 8;
/// Applis du menu Démarrer relues en tâche de fond au-delà de cet âge.
const APPS_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Default)]
pub struct Palette {
    entries: Mutex<Vec<Entry>>,
    /// Numéro de la dernière construction du catalogue demandée : une construction plus
    /// ancienne qui finit après elle est ignorée.
    catalogue_gen: AtomicU64,
    usage: Mutex<Option<Usage>>,
    usage_writer: horizon_core::config::OrderedWriter,
    apps: Mutex<(Option<Instant>, Vec<StartApp>)>,
    scanning: AtomicBool,
    /// Instant du dernier affichage : la perte de focus qui suit de près est ignorée.
    shown_at: Mutex<Option<Instant>>,
    /// Fenêtre au premier plan avant l'ouverture : elle le retrouve à la fermeture (sinon
    /// Windows activerait une barre d'Horizon).
    previous: Mutex<Option<isize>>,
    /// Rectangle (physique) de la fenêtre à sa dernière ouverture : « Ramener ici ».
    origin: Mutex<Option<Rect>>,
    /// Raccourci global : celui en service et la dernière erreur.
    hotkey: Mutex<HotkeySlot>,
    /// Essai : les actions sont journalisées, pas exécutées.
    essai: AtomicBool,
}

/// Raccourci global de la palette.
#[derive(Default)]
struct HotkeySlot {
    /// Texte du raccourci en service (vide = aucun).
    text: String,
    handle: Option<horizon_sys::hotkey::HotkeyHandle>,
    /// Raccourci demandé qui n'a pas pu être pris, et pourquoi.
    error: Option<String>,
}

fn usage_path() -> PathBuf {
    crate::state::app_dir().join("palette.json")
}

impl Palette {
    fn with_usage<T>(&self, f: impl FnOnce(&mut Usage) -> T) -> T {
        let mut u = lock(&self.usage);
        let u = u.get_or_insert_with(|| {
            std::fs::read_to_string(usage_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
        });
        f(u)
    }

    /// Applis du menu Démarrer connues ; relues en tâche de fond si elles datent.
    fn start_apps(&self, app: &AppHandle) -> Vec<StartApp> {
        let (at, list) = lock(&self.apps).clone();
        if at.is_none_or(|t| t.elapsed() > APPS_TTL) && !self.scanning.swap(true, Ordering::SeqCst) {
            let app = app.clone();
            spawn_named("palette-applis", move || scan_apps(&app));
        }
        list
    }
}

/// Relit le menu Démarrer (fil à part : quelques centaines de fichiers).
fn scan_apps(app: &AppHandle) {
    let list = horizon_sys::startmenu::apps();
    let p = app.state::<Palette>();
    log::info!("palette : {} applis dans le menu Démarrer", list.len());
    *lock(&p.apps) = (Some(Instant::now()), list);
    p.scanning.store(false, Ordering::SeqCst);
}

/// Rectangle de la palette à sa dernière ouverture (écran où ramener une fenêtre).
pub(crate) fn origin(app: &AppHandle) -> Option<Rect> {
    *lock(&app.state::<Palette>().origin)
}

/// Un résultat, pour la fenêtre.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemView {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub icon: IconRef,
    /// Noms des actions (la première = Entrée).
    pub actions: Vec<String>,
    /// Lettres du titre à surligner (rangs de caractères).
    pub hits: Vec<usize>,
}

/// Recherche (fil de l'interface : le classement est rapide).
pub fn palette_search(app: &AppHandle, query: &str) -> Vec<ItemView> {
    let p = app.state::<Palette>();
    let entries = lock(&p.entries);
    let ranked = p.with_usage(|u| {
        let cands: Vec<Candidate> =
            entries.iter().map(|e| Candidate { title: &e.title, keywords: &e.keywords, uses: u.uses(&e.id) }).collect();
        rank(query, &cands, LIMIT)
    });
    ranked
        .into_iter()
        .map(|r| {
            let e = &entries[r.index];
            ItemView {
                id: e.id.clone(),
                title: e.title.clone(),
                subtitle: e.subtitle.clone(),
                icon: e.icon.clone(),
                actions: e.actions.iter().map(|(l, _)| l.clone()).collect(),
                hits: r.hits,
            }
        })
        .collect()
}

/// Exécute l'action `action` (0 = principale) de l'entrée `id` ; la palette se ferme avant.
pub fn palette_run(app: &AppHandle, id: &str, action: usize) -> Result<(), String> {
    let a = {
        let p = app.state::<Palette>();
        let entries = lock(&p.entries);
        let e = entries.iter().find(|e| e.id == id).ok_or("entrée inconnue")?;
        e.actions.get(action).ok_or("action inconnue")?.1.clone()
    };
    // L'appli d'avant reprend le clavier ; une action qui ouvre une fenêtre le prendra ensuite.
    hide(app, true);
    let p = app.state::<Palette>();
    if p.essai.load(Ordering::SeqCst) {
        log::info!("palette (essai) : {id}, action {action} ({a:?}) non exécutée");
        return Ok(());
    }
    if id.starts_with(catalogue::WINDOW_PREFIX) {
        return run_action(app, a).inspect_err(|e| log::warn!("palette {id} : {e}"));
    }
    // Version prise sous le verrou des usages, écriture après l'avoir relâché.
    let (ticket, text) = p.with_usage(|u| {
        u.record(id);
        (p.usage_writer.ticket(), serde_json::to_string(u))
    });
    if let Err(e) =
        text.map_err(std::io::Error::other).and_then(|t| p.usage_writer.write(&usage_path(), ticket, t.as_bytes()))
    {
        log::warn!("palette.json : {e}");
    }
    run_action(app, a).inspect_err(|e| log::warn!("palette {id} : {e}"))
}

/// Segments du titre : (texte, surligné), d'après les rangs de caractères `hits`
/// (`highlight` de `lib/palette.ts`).
pub fn highlight(title: &str, hits: &[usize]) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    for (i, c) in title.chars().enumerate() {
        let hit = hits.contains(&i);
        match out.last_mut() {
            Some((s, h)) if *h == hit => s.push(c),
            _ => out.push((c.to_string(), hit)),
        }
    }
    out
}

/// Rang suivant ou précédent dans une liste de `n` (en boucle ; `move` de `lib/palette.ts`).
pub fn step(i: usize, n: usize, delta: i32) -> usize {
    if n == 0 {
        return 0;
    }
    (i as i64 + delta as i64).rem_euclid(n as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_become_segments() {
        assert_eq!(highlight("Discord", &[0, 1, 4]), vec![("Di".into(), true), ("sc".into(), false), ("o".into(), true), ("rd".into(), false)]);
        assert_eq!(highlight("Été", &[1]), vec![("É".into(), false), ("t".into(), true), ("é".into(), false)]);
        assert!(highlight("", &[]).is_empty());
    }

    #[test]
    fn selection_wraps_around() {
        assert_eq!(step(0, 3, -1), 2);
        assert_eq!(step(2, 3, 1), 0);
        assert_eq!(step(1, 3, 1), 2);
        assert_eq!(step(0, 0, 1), 0);
    }
}
