//! Profils de disposition : bascule manuelle (menu de la barre, réglages) et automatique selon
//! l'appli au premier plan. Le retour au profil choisi par l'utilisateur attend que l'appli
//! soit quittée depuis 2 s, pour qu'un Alt-Tab rapide ne fasse pas tout redessiner.
//!
//! Repris de `app/src-tauri/src/profiles.rs`. Différences : les commandes de la page « Profils »
//! des réglages (`ui::profiles`) sont des fonctions ; la vue des profils (noms
//! d'applis lus dans la base des suggestions) est faite hors du fil de l'interface.

use crate::alerts::{self, Alert, Severity};
use crate::events::EV_PROFILES;
use crate::host::{AppHandle, Emitter, Manager};
use crate::state::AppState;
use crate::util::start_thread;
use horizon_core::apps::app_key;
use horizon_core::dnd::AlertKind;
use horizon_core::layout::{Layout, ProfileFollower};
use serde::Serialize;
use std::time::{Duration, Instant};

const RETURN_DELAY: Duration = Duration::from_secs(2);

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct AppView {
    pub key: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProfileView {
    pub id: String,
    pub name: String,
    pub apps: Vec<AppView>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProfilesView {
    pub profiles: Vec<ProfileView>,
    pub active: String,
    /// Profil activé par une appli (le choix de l'utilisateur reprendra ensuite).
    pub auto: bool,
}

/// Nom lisible d'une clé d'appli (journal du premier plan, sinon la clé).
fn app_name(app: &AppHandle, key: &str) -> String {
    app.try_state::<crate::widgets::suggestions::Suggestions>()
        .and_then(|_| crate::widgets::suggestions::app_display_name(app, key))
        .unwrap_or_else(|| horizon_core::apps::display_name(key, None))
}

fn view(app: &AppHandle, l: &Layout) -> ProfilesView {
    ProfilesView {
        profiles: l
            .profiles
            .iter()
            .map(|p| ProfileView {
                id: p.id.clone(),
                name: p.name.clone(),
                apps: p.apps.iter().map(|k| AppView { key: k.clone(), name: app_name(app, k) }).collect(),
            })
            .collect(),
        active: l.active_profile.clone(),
        auto: l.auto_from.is_some(),
    }
}

/// Modifie les profils, enregistre, et redessine les barres si le profil actif a changé.
fn apply(app: &AppHandle, f: impl FnOnce(&mut Layout) -> anyhow::Result<()>) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (switched, snapshot) = state
        .update(|l| {
            let before = l.active_profile.clone();
            f(l)?;
            Ok(l.active_profile != before)
        })
        .map_err(|e| e.to_string())?;
    if switched {
        // L'historique d'annulation parle des barres de l'autre profil.
        state.clear_undo();
        state.sync_monitors(&horizon_sys::monitors::list());
        crate::edit::broadcast_layout(app);
        log::info!("profil actif : {}", snapshot.active_profile);
    }
    // Noms d'applis lus dans la base des suggestions : hors du fil de l'interface.
    let a = app.clone();
    if let Err(e) = start_thread("profils", move || {
        let _ = a.emit(EV_PROFILES, view(&a, &snapshot));
    }) {
        log::warn!("profils : {e:#}");
    }
    Ok(())
}

pub fn switch(app: &AppHandle, id: &str) -> Result<(), String> {
    apply(app, |l| l.switch_profile(id, false))
}

/// Menu de la barre « Nouveau profil (copie de celui-ci) » : le profil est créé et activé
/// (l'îlot le dit), puis la page des profils s'ouvre pour le nommer et lui associer des applis.
pub fn add_from_menu(app: &AppHandle) -> Result<(), String> {
    let mut name = String::new();
    apply(app, |l| {
        name = l.next_profile_name();
        l.add_profile(&name, true).map(|_| ())
    })?;
    let body = "Copie du profil d’avant, à régler dans la fenêtre qui s’ouvre".to_owned();
    alerts::show(app, AlertKind::Feedback, Alert::icon("users", Severity::Info, format!("Profil « {name} » créé et activé"), body));
    crate::settings_window::open(app, crate::settings_window::SettingsPage::Profiles);
    Ok(())
}

/// Texte de l'annonce d'un changement de profil automatique : activé par une appli (`by`), ou
/// retour au profil choisi.
fn auto_text(name: &str, auto: bool, by: &str) -> (String, String) {
    if auto {
        (format!("Profil « {name} »"), format!("Activé : {by} au premier plan"))
    } else {
        (format!("Retour au profil « {name} »"), "L’appli qui l’avait changé n’est plus au premier plan".to_owned())
    }
}

/// Annonce un changement de profil automatique dans l'îlot.
fn announce_auto(app: &AppHandle, app_key: Option<&str>) {
    let (name, auto) = app.state::<AppState>().read(|l| {
        let name = l.profiles.iter().find(|p| p.id == l.active_profile).map(|p| p.name.clone()).unwrap_or_default();
        (name, l.auto_from.is_some())
    });
    let by = app_key.map(|k| app_name(app, k)).unwrap_or_default();
    let (title, body) = auto_text(&name, auto, &by);
    alerts::show(app, AlertKind::Feedback, Alert::icon("users", Severity::Info, title, body));
}

/// Suit le premier plan et applique les profils associés aux applis (à démarrer avant
/// `foreground::start`, dont il est abonné).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let rx = crate::foreground::subscribe(app);
    let app = app.clone();
    start_thread("profiles", move || {
        let mut follower = ProfileFollower::new(RETURN_DELAY);
        let mut last_key: Option<String> = None;
        loop {
            let wait = follower.deadline().map_or(Duration::from_secs(3600), |p| p.saturating_duration_since(Instant::now()));
            match rx.recv_timeout(wait) {
                Ok(Some(f)) => {
                    let key = app_key(&f.app);
                    last_key = Some(key.clone());
                    follower.foreground(key);
                }
                // Bureau, menu Démarrer… : l'appli d'avant reste la référence.
                Ok(None) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
            let next = app.state::<AppState>().read(|l| follower.next(l, Instant::now()));
            if let Some(id) = next {
                match apply(&app, |l| l.switch_profile(&id, true)) {
                    Ok(()) => announce_auto(&app, last_key.as_deref()),
                    Err(e) => log::warn!("profil automatique : {e}"),
                }
            }
        }
    })
}

/// Page des profils : la liste. Hors du fil de l'interface (noms d'applis).
pub fn profiles_state(app: &AppHandle) -> ProfilesView {
    view(app, &app.state::<AppState>().snapshot())
}

pub fn profiles_switch(app: &AppHandle, id: &str) -> Result<(), String> {
    switch(app, id)
}

/// Nouveau profil (fenêtre des profils), copie du profil actif, pas activé : renvoie son nom.
pub fn profiles_add(app: &AppHandle) -> Result<String, String> {
    let mut name = String::new();
    apply(app, |l| {
        name = l.next_profile_name();
        l.add_profile(&name, false).map(|_| ())
    })?;
    Ok(name)
}

pub fn profiles_rename(app: &AppHandle, id: &str, name: &str) -> Result<(), String> {
    apply(app, |l| l.rename_profile(id, name))
}

pub fn profiles_delete(app: &AppHandle, id: &str) -> Result<(), String> {
    apply(app, |l| l.delete_profile(id))
}

pub fn profiles_set_apps(app: &AppHandle, id: &str, apps: Vec<String>) -> Result<(), String> {
    apply(app, |l| l.set_profile_apps(id, apps))
}

/// Applis utilisées ces 30 derniers jours, les plus utilisées d'abord (choix des applis). Hors
/// du fil de l'interface.
pub fn profiles_recent_apps(app: &AppHandle) -> Vec<AppView> {
    if app.try_state::<crate::widgets::suggestions::Suggestions>().is_none() {
        return Vec::new();
    }
    crate::widgets::suggestions::top_apps(app).into_iter().map(|key| AppView { name: app_name(app, &key), key }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_commands_work_in_memory() {
        let (app, dir) = crate::edit::tests::app_with_layout("profils");
        let first = profiles_state(&app);
        assert_eq!(first.profiles.len(), 1);
        let start = first.active.clone();
        // Nouveau profil depuis la fenêtre : copie, pas activé.
        let name = profiles_add(&app).unwrap();
        assert_eq!(name, "Profil 2");
        let v = profiles_state(&app);
        let added = v.profiles.iter().find(|p| p.name == name).unwrap().id.clone();
        assert_eq!(v.active, start);
        profiles_rename(&app, &added, "  Jeu ").unwrap();
        assert!(profiles_rename(&app, &added, "  ").is_err(), "nom vide refusé");
        profiles_set_apps(&app, &added, vec!["steam".into(), "steam".into()]).unwrap();
        let v = profiles_state(&app);
        let jeu = v.profiles.iter().find(|p| p.id == added).unwrap();
        assert_eq!((jeu.name.as_str(), jeu.apps.len()), ("Jeu", 1));
        assert_eq!(jeu.apps[0].key, "steam");
        // Bascule : la disposition suit, l'historique d'annulation repart de zéro.
        let id = app.state::<AppState>().read(|l| l.bars[0].left[0].id.clone());
        crate::edit::on_menu(&app, &format!("remove|{id}"));
        profiles_switch(&app, &added).unwrap();
        assert_eq!(profiles_state(&app).active, added);
        assert!(app.state::<AppState>().read(|l| l.widget(&id).is_some()), "le profil copié a encore le widget");
        assert!(!crate::edit::edit_undo(&app).unwrap(), "historique vidé");
        switch(&app, &start).unwrap();
        assert!(app.state::<AppState>().read(|l| l.widget(&id).is_none()));
        // Menu « Nouveau profil » : créé et activé.
        add_from_menu(&app).unwrap();
        let v = profiles_state(&app);
        assert_eq!(v.profiles.len(), 3);
        assert_ne!(v.active, start);
        profiles_delete(&app, &v.active).unwrap();
        assert_eq!(profiles_state(&app).profiles.len(), 2);
        profiles_delete(&app, &added).unwrap();
        assert!(profiles_delete(&app, &start).is_err(), "le dernier profil reste");
        assert!(profiles_recent_apps(&app).is_empty(), "pas de base des suggestions ici");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_announcements() {
        assert_eq!(auto_text("Jeu", true, "Steam"), ("Profil « Jeu »".into(), "Activé : Steam au premier plan".into()));
        assert_eq!(auto_text("Normal", false, "").0, "Retour au profil « Normal »");
    }
}
