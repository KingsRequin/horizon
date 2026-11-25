//! Widget « Suggestions » : journal du premier plan (`usage.db`), import de l'historique
//! Vivaldi, calcul des suggestions de l'écran au premier plan et diffusion aux barres
//! (conception `2026-11-10-suggestions-design.md`). Dans la barre : une icône par suggestion
//! (favicon du site, icône de l'appli), seulement sur l'écran au premier plan et quand une
//! suggestion est assez sûre ; clic = ouvrir, clic droit = « Ne plus proposer ».
//!
//! Verrous : chacun est tenu le temps d'une lecture ou d'une requête SQL, jamais pendant une
//! copie de fichier, un appel COM ou un `emit`, et jamais deux à la fois. Les fonctions
//! appelées depuis le fil de l'interface n'attendent donc pas le fil de calcul.
//!
//! `worker` : le fil de calcul ; `view` : la suggestion affichée ; ici l'état, les commandes et
//! la rangée d'une barre. Repris de `app/src-tauri/src/widgets/suggestions/` ; différences :
//! les icônes vont à Slint en images (`ui::images`, qui garde aussi celles obtenues) ; le menu
//! « Ne plus proposer » est un menu Windows (`ui::menu`). `usage.db` est écrit comme par la
//! version Tauri (journal, visites, exclusions) : ce n'est pas un fichier de réglages.

mod view;
mod worker;

pub use worker::start;

use super::Backend;
use crate::host::{AppHandle, Manager};
use crate::ui::images::{self, Pixels};
use crate::ui::{BarWindow, SuggestionItem, SuggestionsData};
use horizon_core::layout::BarLayout;
use crate::util::{Published, Waker, lock};
use horizon_core::misses::Misses;
use horizon_core::suggest::ItemKey;
use horizon_core::suggest::history::read_favicon;
use horizon_core::suggest::store::Store;
use horizon_core::time::now_secs as now_utc;
use serde::Serialize;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use view::view;

/// Suggestions par écran.
pub const EV_SUGGESTIONS: &str = "horizon://suggestions";

pub const BACKEND: Backend = Backend {
    id: "suggestions",
    name: "suggestions",
    manage: |app| {
        app.manage(Suggestions::default());
    },
    start: Some(start),
    bind: Some(bind),
    exit: Some(flush),
    ..Backend::NONE
};

/// Une appli qu'on n'a pas su relier à un raccourci du menu Démarrer n'est pas recherchée de
/// nouveau avant ce délai (chaque recherche parcourt tout le menu par COM).
const UNRESOLVED_TTL: Duration = Duration::from_secs(30 * 60);
/// Une icône introuvable n'est pas recherchée de nouveau avant ce délai.
const ICON_MISS_TTL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuggestionView {
    pub key: String,
    pub label: String,
    pub kind: &'static str,
    pub reason: String,
}

/// Appli au premier plan depuis `start`.
struct Current {
    app: String,
    monitor: String,
    start_utc: i64,
    start_local: i64,
}

type Shown = HashMap<String, Vec<SuggestionView>>;

pub struct Suggestions {
    /// Base d'usage (ouverte par le fil de calcul).
    store: Mutex<Option<Store>>,
    /// Appli au premier plan (journal en cours).
    current: Mutex<Option<Current>>,
    /// Suggestions affichées, par écran.
    shown: Published<Shown>,
    /// Icônes introuvables récemment (chaque recherche d'un site copie la base des favicons).
    icon_misses: Mutex<Misses<String>>,
    /// Applis qu'on n'a pas su relancer récemment (cache négatif de `resolve_launch`).
    unresolved: Mutex<Misses<String>>,
    wake: Waker,
}

impl Default for Suggestions {
    fn default() -> Self {
        Suggestions {
            store: Mutex::default(),
            current: Mutex::default(),
            shown: Published::new(EV_SUGGESTIONS, Shown::new()),
            icon_misses: Mutex::new(Misses::new(ICON_MISS_TTL)),
            unresolved: Mutex::new(Misses::new(UNRESOLVED_TTL)),
            wake: Waker::default(),
        }
    }
}

impl Suggestions {
    /// Exécute `f` sur la base, si elle est ouverte.
    fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Option<T> {
        lock(&self.store).as_ref().map(f)
    }

    /// Demande un recalcul (exclusion changée…).
    fn refresh(&self) {
        self.wake.wake();
    }

    /// Ferme l'intervalle en cours à `end_utc` ; renvoie l'appli qui était au premier plan.
    fn close_current(&self, end_utc: i64) -> Option<(String, String)> {
        let c = lock(&self.current).take()?;
        let saved = self.with_store(|s| s.add_interval(&c.app, &c.monitor, c.start_utc, end_utc, c.start_local));
        if let Some(Err(e)) = saved {
            log::warn!("journal du premier plan : {e:#}");
        }
        Some((c.app, c.monitor))
    }

    /// Cible de lancement d'une appli : en base, sinon cherchée dans le menu Démarrer (par le fil
    /// shell : bloquant, jamais depuis le fil de l'interface).
    fn launch_target(&self, key: &str) -> Option<String> {
        let raw = self.with_store(|s| match s.launch_target(key) {
            // Entrée fausse d'une version antérieure (Explorateur → raccourci du SDK) : réparée.
            Some(t) if horizon_core::apps::is_bogus_launch(key, &t) => {
                let _ = s.clear_launch(key);
                Err(s.last_raw_app(key))
            }
            Some(t) => Ok(t),
            None => Err(s.last_raw_app(key)),
        })?;
        let raw = match raw {
            Ok(target) => return Some(target),
            Err(raw) => raw?,
        };
        if lock(&self.unresolved).recent(&key.to_owned(), Instant::now()) {
            return None;
        }
        match horizon_sys::foreground::resolve_launch(&raw) {
            Some(t) => {
                let _ = self.with_store(|s| s.set_launch(key, &t));
                Some(t)
            }
            None => {
                lock(&self.unresolved).record(key.to_owned(), Instant::now());
                None
            }
        }
    }
}

/// Nom lisible d'une appli connue du journal du premier plan.
pub fn app_display_name(app: &AppHandle, key: &str) -> Option<String> {
    app.state::<Suggestions>()
        .with_store(|s| {
            let raw = s.last_raw_app(key)?;
            Some(horizon_core::apps::display_name(&raw, s.launch_target(key).as_deref()))
        })
        .flatten()
}

/// Applis les plus utilisées ces 30 derniers jours.
pub fn top_apps(app: &AppHandle) -> Vec<String> {
    let top = app.state::<Suggestions>().with_store(|s| s.top_apps(now_utc() - 30 * 86_400, 40));
    match top {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            log::warn!("applis récentes : {e:#}");
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// Ferme l'intervalle en cours (arrêt d'Horizon).
pub fn flush(app: &AppHandle) {
    if let Some(hub) = app.try_state::<Suggestions>() {
        hub.close_current(now_utc());
    }
}

/// Suggestions affichées, par écran.
pub fn suggestions_state(app: &AppHandle) -> Shown {
    app.state::<Suggestions>().shown.get()
}

/// Ouvre une suggestion. Une appli est relancée par le fil shell (la recherche de sa cible
/// passe par COM) ; un échec y est journalisé.
pub fn suggestions_open(app: &AppHandle, key: &str) -> Result<(), String> {
    let item = ItemKey::parse(key).ok_or("suggestion inconnue")?;
    let hub = app.state::<Suggestions>();
    match item {
        ItemKey::Site(h) => {
            let profile = hub.with_store(|s| s.profile_of(&h)).ok_or("base indisponible")?;
            let profile = profile.unwrap_or_else(|| "Default".into());
            horizon_sys::vivaldi::open(&format!("https://{h}/"), &profile);
        }
        ItemKey::App(k) => {
            let raw = hub.with_store(|s| s.last_raw_app(&k)).ok_or("base indisponible")?.ok_or("appli inconnue")?;
            let app = app.clone();
            // Sur le fil shell (recherche de la cible et lancement), sans attendre.
            horizon_sys::shell::spawn(move || {
                if horizon_sys::foreground::focus_running(&raw) {
                    return;
                }
                match app.state::<Suggestions>().launch_target(&k) {
                    Some(target) => horizon_sys::launch::launch(&target),
                    None => log::warn!("suggestion {k} : appli impossible à relancer"),
                }
            });
        }
    }
    Ok(())
}

/// « Ne plus proposer ».
pub fn block(app: &AppHandle, key: &str) -> Result<(), String> {
    let item = ItemKey::parse(key).ok_or("suggestion inconnue")?;
    let hub = app.state::<Suggestions>();
    hub.with_store(|s| s.block(&item)).ok_or("base indisponible")?.map_err(|e| e.to_string())?;
    hub.refresh();
    Ok(())
}

pub fn suggestions_unblock(app: &AppHandle, key: &str) -> Result<(), String> {
    let item = ItemKey::parse(key).ok_or("suggestion inconnue")?;
    let hub = app.state::<Suggestions>();
    hub.with_store(|s| s.unblock(&item)).ok_or("base indisponible")?.map_err(|e| e.to_string())?;
    hub.refresh();
    Ok(())
}

/// Éléments exclus (« Ne plus proposer »), pour la page de réglages.
pub fn suggestions_blocked(app: &AppHandle) -> Vec<SuggestionView> {
    let hub = app.state::<Suggestions>();
    let mut out: Vec<SuggestionView> = hub
        .with_store(|s| s.blocked().unwrap_or_default().iter().map(|k| view(s, k, String::new())).collect())
        .unwrap_or_default();
    out.sort_by_key(|v| v.label.to_lowercase());
    out
}

/// Icône d'une suggestion : favicon Vivaldi d'un site, icône d'une appli. Lent (copie de la base
/// des favicons, COM) : sur le fil des chargements d'images.
pub fn suggestion_icon(app: &AppHandle, key: &str) -> Option<Pixels> {
    let item = ItemKey::parse(key)?;
    let hub = app.state::<Suggestions>();
    if lock(&hub.icon_misses).recent(&key.to_owned(), Instant::now()) {
        return None;
    }
    let px = match item {
        ItemKey::Site(h) => {
            let profile = hub.with_store(|s| s.profile_of(&h)).flatten().unwrap_or_else(|| "Default".into());
            horizon_sys::vivaldi::profiles()
                .into_iter()
                .find(|p| p.dir == profile)
                .and_then(|p| read_favicon(&p.favicons, &h, &crate::state::app_dir().join("tmp")).ok().flatten())
                .and_then(|png| images::decode(&png))
                .map(Pixels::Rgba)
        }
        ItemKey::App(k) => hub.launch_target(&k).and_then(|t| super::shortcuts::exe_icon(&t)),
    };
    if px.is_none() {
        lock(&hub.icon_misses).record(key.to_owned(), Instant::now());
    }
    px
}

// ---- Rangée d'une barre (fil de l'interface) ----

/// Infobulle : « youtube.com — souvent le samedi vers 16:00 ».
pub fn title(v: &SuggestionView) -> String {
    format!("{} — {}", v.label, v.reason)
}

struct BarSuggestions {
    app: AppHandle,
    monitor: String,
    model: Rc<VecModel<SuggestionItem>>,
    shown: std::cell::RefCell<Vec<SuggestionView>>,
}

impl BarSuggestions {
    /// Rangée de cet écran (seules les lignes changées sont reposées).
    fn refresh(self: &Rc<Self>) {
        let list = self.shown.borrow().clone();
        let rows: Vec<SuggestionItem> = list
            .iter()
            .map(|v| {
                let (a, k, me) = (self.app.clone(), v.key.clone(), Rc::downgrade(self));
                let image = images::get(&format!("suggestion:{}", v.key), move || suggestion_icon(&a, &k), move |_| {
                    if let Some(me) = me.upgrade() {
                        me.refresh();
                    }
                });
                SuggestionItem {
                    key: v.key.as_str().into(),
                    site: v.kind == "site",
                    has_image: image.is_some(),
                    image: image.unwrap_or_default(),
                    title: title(v).into(),
                }
            })
            .collect();
        let m = &self.model;
        if m.row_count() != rows.len() || rows.iter().enumerate().any(|(i, r)| m.row_data(i).as_ref().map(|x| &x.key) != Some(&r.key)) {
            m.set_vec(rows);
        } else {
            for (i, r) in rows.into_iter().enumerate() {
                if m.row_data(i).as_ref() != Some(&r) {
                    m.set_row_data(i, r);
                }
            }
        }
    }

    fn apply(self: &Rc<Self>, shown: &Shown) {
        let mine = shown.get(&self.monitor).cloned().unwrap_or_default();
        if *self.shown.borrow() != mine {
            *self.shown.borrow_mut() = mine;
            self.refresh();
        }
    }
}

/// Relie le widget Suggestions d'une barre (fil de l'interface).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let me = Rc::new(BarSuggestions {
        app: app.clone(),
        monitor: bar.monitor_id.clone(),
        model: Rc::default(),
        shown: Default::default(),
    });
    let g = window.global::<SuggestionsData>();
    g.set_items(ModelRc::from(me.model.clone()));
    me.apply(&suggestions_state(app));
    // Le rappel garde la rangée en vie tant que la fenêtre existe.
    let m = me.clone();
    g.on_open(move |key| {
        if let Err(e) = suggestions_open(&m.app, &key) {
            log::warn!("suggestion {key} : {e}");
        }
    });
    let (a, l) = (app.clone(), label.to_owned());
    g.on_menu(move |key| {
        let Some(hwnd) = a.get_window(&l).map(|w| w.hwnd()) else { return };
        let a = a.clone();
        crate::ui::menu::popup(hwnd, vec![Some((1, "Ne plus proposer".to_owned()))], move |_| {
            let key = key.to_string();
            // Requête SQL : hors du fil de l'interface.
            let _ = crate::util::start_thread("suggestion-exclue", move || {
                if let Err(e) = block(&a, &key) {
                    log::warn!("ne plus proposer {key} : {e}");
                }
            });
        });
    });
    let m = Rc::downgrade(&me);
    crate::host::listen(EV_SUGGESTIONS, Some(label), move |e| {
        if let (Some(s), Some(m)) = (e.payload::<Shown>(), m.upgrade()) {
            m.apply(s);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        let v = SuggestionView { key: "site:youtube.com".into(), label: "youtube.com".into(), kind: "site", reason: "souvent le samedi vers 16:00".into() };
        assert_eq!(title(&v), "youtube.com — souvent le samedi vers 16:00");
    }
}
