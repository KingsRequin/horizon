//! Page « Profils » de la fenêtre de réglages (`ui/profiles/page.slint`, global `ProfilesData`),
//! sur le fil de l'interface. Repris de `app/src/settings/ProfilesEditor.svelte` : la liste vient
//! de `profiles::profiles_state` et suit `EV_PROFILES` ; les commandes (`profiles_rename`,
//! `_switch`, `_add`, `_delete`, `_set_apps`) changent la disposition. Applis récentes : `profiles_recent_apps` (journal du premier plan), lues hors du
//! fil de l'interface. Suppression différée : le profil disparaît et n'est supprimé qu'à la fin
//! du délai d'annulation ; retrait d'une appli annulable.

use super::{ProfileApp, ProfileRow, ProfilesData, SettingsWindow};
use crate::events::EV_PROFILES;
use crate::host::{self, AppHandle};
use crate::profiles::{self, AppView, ProfileView, ProfilesView};
use crate::settings_window::LABEL;
use crate::ui::settings_window::offer_undo;
use crate::undoable::Removal;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

#[derive(Default)]
struct Page {
    app: Option<AppHandle>,
    window: Option<slint::Weak<SettingsWindow>>,
    view: Option<ProfilesView>,
    recent: Vec<AppView>,
    /// Profils en cours de suppression (délai d'annulation) : cachés.
    leaving: Vec<String>,
    /// Profil dont le choix d'une appli est ouvert.
    picking: Option<String>,
    /// Nom du dernier profil créé (pas activé).
    created: Option<String>,
    shown: bool,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut Page) -> R) -> R {
    PAGE.with_borrow_mut(f)
}

fn window() -> Option<SettingsWindow> {
    with(|p| p.window.as_ref().and_then(slint::Weak::upgrade))
}

fn app() -> Option<AppHandle> {
    with(|p| p.app.clone())
}

/// Profils montrés : sans ceux qui partent.
fn shown_profiles(p: &Page) -> Vec<ProfileView> {
    p.view.as_ref().map(|v| v.profiles.iter().filter(|x| !p.leaving.contains(&x.id)).cloned().collect()).unwrap_or_default()
}

/// Applis récentes proposées pour un profil : celles qu'il n'a pas, avec le profil qui les a
/// déjà (elles en seront retirées).
pub fn candidates(profiles: &[ProfileView], profile: &ProfileView, recent: &[AppView]) -> Vec<(AppView, Option<String>)> {
    recent
        .iter()
        .filter(|r| !profile.apps.iter().any(|a| a.key == r.key))
        .map(|r| (r.clone(), profiles.iter().find(|p| p.apps.iter().any(|a| a.key == r.key)).map(|p| p.name.clone())))
        .collect()
}

fn app_row(a: &AppView, other: Option<&str>) -> ProfileApp {
    ProfileApp { key: a.key.as_str().into(), name: a.name.as_str().into(), other: other.unwrap_or_default().into() }
}

fn refresh() {
    let Some(w) = window() else { return };
    let g = w.global::<ProfilesData>();
    let (profiles, active, auto, recent, picking, created) = with(|p| {
        let v = p.view.as_ref();
        (
            shown_profiles(p),
            v.map(|v| v.active.clone()).unwrap_or_default(),
            v.is_some_and(|v| v.auto),
            p.recent.clone(),
            p.picking.clone(),
            p.created.clone(),
        )
    });
    let rows: Vec<ProfileRow> = profiles
        .iter()
        .map(|p| {
            let picking = picking.as_deref() == Some(p.id.as_str());
            let cands: Vec<ProfileApp> =
                if picking { candidates(&profiles, p, &recent).iter().map(|(a, o)| app_row(a, o.as_deref())).collect() } else { Vec::new() };
            ProfileRow {
                name: p.name.as_str().into(),
                active: p.id == active,
                apps: ModelRc::new(VecModel::from(p.apps.iter().map(|a| app_row(a, None)).collect::<Vec<_>>())),
                picking,
                candidates: ModelRc::new(VecModel::from(cands)),
            }
        })
        .collect();
    g.set_rows(ModelRc::new(VecModel::from(rows)));
    g.set_auto(auto);
    // « X » créé : seulement tant qu'il existe et n'est pas actif.
    let created = created.filter(|n| profiles.iter().any(|p| &p.name == n && p.id != active));
    g.set_created(created.unwrap_or_default().into());
}

fn set_error(e: &str) {
    if let Some(w) = window() {
        w.global::<ProfilesData>().set_error(e.into());
    }
}

/// Lance une commande ; son erreur s'affiche sous la page.
fn run(f: impl FnOnce(&AppHandle) -> Result<(), String>) {
    let Some(app) = app() else { return };
    set_error("");
    if let Err(e) = f(&app) {
        set_error(&e);
    }
}

/// Profil montré au rang `i`.
fn at(i: i32) -> Option<ProfileView> {
    with(|p| shown_profiles(p).get(i.max(0) as usize).cloned())
}

/// Relit la liste (hors du fil de l'interface : noms d'applis).
fn load() {
    let Some(app) = app() else { return };
    crate::util::spawn_named("reglages-profils", move || {
        let view = profiles::profiles_state(&app);
        let recent = profiles::profiles_recent_apps(&app);
        let _ = slint::invoke_from_event_loop(move || {
            with(|p| {
                p.view = Some(view);
                p.recent = recent;
            });
            refresh();
        });
    });
}

/// Relie une fenêtre de réglages qui vient d'être créée.
pub fn bind(app: &AppHandle, window: &SettingsWindow) {
    with(|p| {
        p.app = Some(app.clone());
        p.window = Some(window.as_weak());
    });
    let g = window.global::<ProfilesData>();
    g.on_rename(|i, name| {
        let Some(p) = at(i) else { return };
        let name = name.trim().to_owned();
        if !name.is_empty() && name != p.name {
            run(|a| profiles::profiles_rename(a, &p.id, &name));
        } else {
            refresh();
        }
    });
    g.on_activate(|i| {
        if let Some(p) = at(i) {
            run(|a| profiles::profiles_switch(a, &p.id));
        }
    });
    g.on_remove(remove);
    g.on_remove_app(remove_app);
    g.on_toggle_picker(|i| {
        let Some(p) = at(i) else { return };
        with(|s| s.picking = if s.picking.as_deref() == Some(p.id.as_str()) { None } else { Some(p.id.clone()) });
        refresh();
    });
    g.on_add_app(|i, k| {
        let Some(p) = at(i) else { return };
        let recent = with(|s| s.recent.clone());
        let all = with(|s| shown_profiles(s));
        let Some((a, _)) = candidates(&all, &p, &recent).into_iter().nth(k.max(0) as usize) else { return };
        let mut keys: Vec<String> = p.apps.iter().map(|x| x.key.clone()).collect();
        keys.push(a.key);
        run(|app| profiles::profiles_set_apps(app, &p.id, keys));
    });
    g.on_add(|| {
        let Some(app) = self::app() else { return };
        set_error("");
        match profiles::profiles_add(&app) {
            Ok(name) => with(|p| p.created = Some(name)),
            Err(e) => set_error(&e),
        }
    });
    g.on_activate_created(|| {
        let created = with(|p| p.created.clone());
        let id = with(|p| shown_profiles(p).into_iter().find(|x| Some(&x.name) == created.as_ref()).map(|x| x.id));
        if let Some(id) = id {
            run(|a| profiles::profiles_switch(a, &id));
        }
    });
    host::listen(EV_PROFILES, Some(LABEL), |e| {
        if let Some(v) = e.payload::<ProfilesView>() {
            let shown = with(|p| {
                p.view = Some(v.clone());
                p.shown
            });
            if shown {
                refresh();
            }
        }
    });
}

/// La page s'affiche.
pub fn show(_app: &AppHandle, window: &SettingsWindow) {
    with(|p| {
        p.shown = true;
        p.leaving.clear();
        p.picking = None;
        p.created = None;
    });
    window.global::<ProfilesData>().set_error("".into());
    window.invoke_scroll_top();
    refresh();
    load();
}

/// La page n'est plus affichée.
pub fn leave() {
    with(|p| p.shown = false);
}

/// La fenêtre se ferme.
pub fn closing(_app: &AppHandle) {
    leave();
    with(|p| p.window = None);
}

/// Suppression différée d'un profil (ses barres seraient perdues) : caché tout de suite,
/// supprimé à la fin du délai.
fn remove(i: i32) {
    let Some(p) = at(i) else { return };
    if with(|s| shown_profiles(s).len()) < 2 {
        return;
    }
    with(|s| s.leaving.push(p.id.clone()));
    refresh();
    let back = |id: &str| {
        with(|s| s.leaving.retain(|x| x != id));
        refresh();
    };
    let (undo_id, commit_id) = (p.id.clone(), p.id.clone());
    offer_undo(Removal {
        label: format!("Profil « {} » supprimé", p.name),
        undo: Box::new(move || back(&undo_id)),
        commit: Some(Box::new(move || {
            run(|a| profiles::profiles_delete(a, &commit_id));
            back(&commit_id);
        })),
    });
}

/// Retrait annulable d'une appli d'un profil.
fn remove_app(i: i32, k: i32) {
    let Some(p) = at(i) else { return };
    let Some(a) = p.apps.get(k.max(0) as usize).cloned() else { return };
    let before: Vec<String> = p.apps.iter().map(|x| x.key.clone()).collect();
    let next: Vec<String> = before.iter().filter(|x| **x != a.key).cloned().collect();
    run(|app| profiles::profiles_set_apps(app, &p.id, next));
    let id = p.id.clone();
    offer_undo(Removal {
        label: format!("{} retirée du profil « {} »", a.name, p.name),
        undo: Box::new(move || run(|app| profiles::profiles_set_apps(app, &id, before))),
        commit: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(k: &str) -> AppView {
        AppView { key: k.into(), name: k.to_uppercase() }
    }

    #[test]
    fn candidates_skip_the_apps_of_the_profile_and_name_their_owner() {
        let profiles = vec![
            ProfileView { id: "a".into(), name: "Jeu".into(), apps: vec![app("steam")] },
            ProfileView { id: "b".into(), name: "Travail".into(), apps: vec![app("code")] },
        ];
        let recent = vec![app("code"), app("steam"), app("vivaldi")];
        let c = candidates(&profiles, &profiles[0], &recent);
        assert_eq!(c.iter().map(|(a, o)| (a.key.as_str(), o.as_deref())).collect::<Vec<_>>(), [("code", Some("Travail")), ("vivaldi", None)]);
    }
}
