//! « Ne pas déranger » commun : bouton du panneau des notifications, séance du minuteur, plage
//! de nuit. Pendant le NPD, l'îlot ne montre plus les notifications (ni, selon la section
//! « Alertes » des réglages, « Claude attend » et les alertes d'état) ; Discord passe en « Ne pas
//! déranger » et Prisme tient la scène `focus` si c'est demandé. La bascule manuelle et l'option
//! du minuteur sont gardées dans `%APPDATA%\Horizon\dnd.json`.
//!
//! Repris de l'`app/src-tauri/src/dnd.rs` de la version Tauri.

use crate::events::EV_DND;
use crate::host::{AppHandle, Emitter, Manager};
use crate::state::AppState;
use horizon_core::config::OrderedWriter;
use horizon_core::dnd::{AlertKind, Dnd, DndFile, DndOptions, allows as dnd_allows};
use serde::Serialize;
use std::sync::Mutex;

struct Inner {
    dnd: Dnd,
    /// Option « NPD pendant la séance » du minuteur.
    timer_dnd: bool,
}

pub struct DndHub(Mutex<Inner>, OrderedWriter);

impl Default for DndHub {
    fn default() -> Self {
        let file = std::fs::read_to_string(path()).map(|t| DndFile::parse(&t)).unwrap_or_default();
        let inner = Inner { dnd: Dnd { manual: file.manual, ..Dnd::default() }, timer_dnd: file.timer_dnd };
        DndHub(Mutex::new(inner), OrderedWriter::new())
    }
}

/// État envoyé aux fenêtres (`horizon://dnd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct DndView {
    pub active: bool,
    /// Activé par la séance du minuteur (et non par le bouton).
    pub session: bool,
    /// Activé par la plage de nuit (option « NPD la nuit »).
    pub night: bool,
}

impl DndView {
    /// Raison affichée sous « Ne pas déranger » dans le panneau des notifications.
    pub fn why(&self) -> &'static str {
        if self.session {
            "pendant la séance du minuteur"
        } else if self.night {
            "la nuit"
        } else {
            "activé"
        }
    }
}

fn path() -> std::path::PathBuf {
    crate::state::app_dir().join("dnd.json")
}

fn lock(app: &AppHandle) -> std::sync::MutexGuard<'_, Inner> {
    crate::util::lock(&app.state::<DndHub>().inner().0)
}

fn view(i: &Inner) -> DndView {
    DndView { active: i.dnd.active(), session: i.dnd.session, night: i.dnd.night }
}

/// Contenu de `dnd.json` et numéro de version, pris sous le verrou ; écrit par `save` après.
fn snapshot(app: &AppHandle, i: &Inner) -> (u64, String) {
    let file = DndFile { manual: i.dnd.manual, timer_dnd: i.timer_dnd };
    (app.state::<DndHub>().1.ticket(), serde_json::to_string(&file).unwrap_or_default())
}

fn save(app: &AppHandle, (ticket, text): (u64, String)) {
    if let Err(e) = app.state::<DndHub>().1.write(&path(), ticket, text.as_bytes()) {
        log::warn!("enregistrement de dnd.json : {e}");
    }
}

/// Options du NPD (section « Alertes » des réglages).
pub fn options(app: &AppHandle) -> DndOptions {
    let alerts = crate::state::settings().alerts;
    app.state::<AppState>().read(|l| alerts.dnd(l))
}

pub fn is_active(app: &AppHandle) -> bool {
    lock(app).dnd.active()
}

/// L'îlot dynamique peut-il montrer cette alerte en ce moment ? (phase D)
pub fn allows(app: &AppHandle, kind: AlertKind) -> bool {
    let active = is_active(app);
    active.then(|| options(app)).is_none_or(|o| dnd_allows(kind, active, &o))
}

/// Effets hors de l'îlot : statut Discord et scène de Prisme.
fn apply(app: &AppHandle, active: bool) {
    let opts = options(app);
    if opts.discord {
        crate::widgets::discord_voice::set_dnd(app, active);
    }
    // Option décochée pendant un NPD : la scène est rendue quand même à la fin.
    crate::plugins::events::emit(app, "dnd", serde_json::json!({ "active": active, "light": opts.light }));
}

/// Diffuse le nouvel état et applique ses effets si l'état actif a changé.
fn changed(app: &AppHandle, before: bool) {
    let (v, active, file) = {
        let i = lock(app);
        (view(&i), i.dnd.active(), snapshot(app, &i))
    };
    save(app, file);
    if active != before {
        log::info!("ne pas déranger : {}", if active { "activé" } else { "désactivé" });
        apply(app, active);
    }
    let _ = app.emit(EV_DND, v);
    crate::widgets::phone::actions_changed(app);
}

/// Au démarrage : un NPD enregistré reprend ses effets (Discord et Prisme les appliquent à
/// leur connexion).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    if is_active(app) {
        apply(app, true);
    }
    Ok(())
}

pub fn toggle(app: &AppHandle) {
    let before = {
        let mut i = lock(app);
        let before = i.dnd.active();
        i.dnd.toggle();
        before
    };
    changed(app, before);
}

/// Séance du minuteur (début, fin, passage en pause du pomodoro).
pub fn set_session(app: &AppHandle, on: bool) {
    let before = {
        let mut i = lock(app);
        if i.dnd.session == on {
            return;
        }
        let before = i.dnd.active();
        i.dnd.set_session(on);
        before
    };
    changed(app, before);
}

/// Début ou fin de la plage de nuit (option « NPD la nuit »).
pub fn set_night(app: &AppHandle, on: bool) {
    let before = {
        let mut i = lock(app);
        if i.dnd.night == on {
            return;
        }
        let before = i.dnd.active();
        i.dnd.set_night(on);
        before
    };
    changed(app, before);
}

/// Réglages modifiés : un NPD en cours applique ses nouvelles options (Discord, Prisme).
pub fn reapply(app: &AppHandle) {
    if is_active(app) {
        apply(app, true);
    }
}

/// Option « NPD pendant la séance » du minuteur.
pub fn timer_dnd(app: &AppHandle) -> bool {
    lock(app).timer_dnd
}

/// Commande `dnd_state`.
pub fn dnd_state(app: &AppHandle) -> DndView {
    view(&lock(app))
}

/// Commande `dnd_toggle` (bouton du panneau des notifications).
pub fn dnd_toggle(app: &AppHandle) {
    toggle(app);
}

/// Commande `timer_set_dnd` (case du panneau du minuteur).
pub fn timer_set_dnd(app: &AppHandle, on: bool) {
    let before = {
        let mut i = lock(app);
        i.timer_dnd = on;
        i.dnd.active()
    };
    changed(app, before);
    crate::timer::sync_session(app);
    // La case du panneau lit l'option dans l'état du minuteur (la case HTML de la version
    // Tauri se cochait d'elle-même).
    crate::timer::broadcast(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_of_the_dnd() {
        let v = |session, night| DndView { active: true, session, night };
        assert_eq!(v(true, true).why(), "pendant la séance du minuteur");
        assert_eq!(v(false, true).why(), "la nuit");
        assert_eq!(v(false, false).why(), "activé");
    }
}
