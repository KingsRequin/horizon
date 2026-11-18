//! Notifications Windows : liste récente, non lues. Depuis le panneau (phase C) : ouvrir (et
//! marquer lue), écarter (retirée aussi de Windows), couper une appli (ni îlot ni compte ;
//! gardé dans `%APPDATA%\Horizon\notifs.json`).
//!
//! Celle qui arrive passe quelques secondes dans l'îlot dynamique (`alerts`, `ui::island`) ; le
//! widget le montre lui-même en style graphique, ou en îlots sur une barre sans Lecture en cours.
//!
//! Repris de l'`app/src-tauri/src/widgets/notifications.rs` de la version Tauri.
//!
//! **Identité de paquet** : `UserNotificationListener` exige une identité de paquet (paquet
//! « sparse » de `packaging/`, enregistré pour `horizon.exe` du dossier d'installation ; le
//! manifeste `natif/horizon.exe.manifest` la déclare). Lancé depuis le dossier de compilation,
//! `horizon.exe` n'en a pas : la lecture échoue, le service le signale (`unavailable`, cloche
//! estompée) et retente avec un délai croissant.

mod panel;

use super::Backend;
use crate::alerts::{Alert, Severity};
use crate::brands;
use horizon_core::dnd::AlertKind;
use crate::dnd::DndView;
use crate::events::EV_DND;
use crate::host::{AppHandle, Emitter, Manager};
use crate::ui::{BarWindow, NotifsData};
use horizon_core::layout::BarLayout;
use crate::util::{lock, start_thread};
use horizon_core::config::OrderedWriter;
use horizon_core::notifs::{NotifStore, Notification};
use horizon_sys::notifications::NotifWatcher;
use serde::{Deserialize, Serialize};
use slint::ComponentHandle;
use std::sync::Mutex;

/// Notifications Windows (non lues, récentes).
pub const EV_NOTIFS: &str = "horizon://notifs";

pub const BACKEND: Backend = Backend {
    id: "notifications",
    name: "notifications",
    manage: |app| {
        app.manage(Notifs::default());
    },
    start: Some(start),
    bind: Some(bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

const RECENT: usize = 20;
/// Au-delà, la dernière notification laisse la place à la cloche (barre sans clic, style
/// graphique).
pub const RECENT_MS: u64 = 10 * 60_000;

pub struct Notifs {
    store: Mutex<NotifStore>,
    watcher: Mutex<Option<NotifWatcher>>,
    writer: OrderedWriter,
}

/// Contenu de `notifs.json`.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct NotifsFile {
    /// Applis coupées (identifiants d'appli).
    muted: Vec<String>,
}

fn path() -> std::path::PathBuf {
    crate::state::app_dir().join("notifs.json")
}

impl Default for Notifs {
    fn default() -> Self {
        let file: NotifsFile =
            std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut store = NotifStore::default();
        for app_id in &file.muted {
            store.set_muted(app_id, true);
        }
        Notifs { store: Mutex::new(store), watcher: Mutex::default(), writer: OrderedWriter::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotifsPayload {
    pub unread: usize,
    pub recent: Vec<Notification>,
    /// Non lues parmi `recent`.
    pub unread_ids: Vec<u32>,
    /// Applis coupées.
    pub muted: Vec<String>,
    pub access_denied: bool,
    pub unavailable: bool,
}

fn payload(store: &NotifStore) -> NotifsPayload {
    NotifsPayload {
        unread: store.unread(),
        recent: store.recent(RECENT),
        unread_ids: store.unread_ids(),
        muted: store.muted(),
        access_denied: store.access_denied,
        unavailable: store.unavailable,
    }
}

fn broadcast(app: &AppHandle) {
    let p = payload(&lock(&app.state::<Notifs>().inner().store));
    let _ = app.emit(EV_NOTIFS, p);
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    let w = horizon_sys::notifications::start(tx)?;
    *lock(&app.state::<Notifs>().inner().watcher) = Some(w);
    let app = app.clone();
    start_thread("notifs-hub", move || {
        while let Ok(first) = rx.recv() {
            // Une lecture produit souvent plusieurs événements (toute la liste au démarrage) :
            // ils sont appliqués ensemble, puis diffusés une seule fois.
            let batch: Vec<_> = std::iter::once(first).chain(rx.try_iter()).collect();
            let now = horizon_core::time::now_ms();
            let fresh: Vec<Notification> = {
                let notifs = app.state::<Notifs>();
                let mut store = lock(&notifs.store);
                batch.into_iter().filter_map(|ev| store.apply(ev, now)).collect()
            };
            // Chacune passe dans l'îlot dynamique (sauf « Ne pas déranger »).
            for n in fresh {
                let alert = Alert::app(&n.app_name, &n.app_id, Severity::Info, n.title, n.body);
                crate::alerts::show(&app, AlertKind::Notification, alert);
            }
            broadcast(&app);
        }
    })
}

/// Accès refusé par Windows, lecture momentanément impossible (page « État »).
pub fn health(app: &AppHandle) -> (bool, bool) {
    let notifs = app.state::<Notifs>();
    let store = lock(&notifs.store);
    (store.access_denied, store.unavailable)
}

pub fn notifs_state(app: &AppHandle) -> NotifsPayload {
    payload(&lock(&app.state::<Notifs>().inner().store))
}

pub fn notifs_mark_read(app: &AppHandle) {
    lock(&app.state::<Notifs>().inner().store).mark_all_read();
    broadcast(app);
}

/// Ouvre l'appli d'où vient une notification (par son identifiant d'appli) ; la notification
/// devient lue.
pub fn notifs_open(app: &AppHandle, id: u32, app_id: String) {
    if lock(&app.state::<Notifs>().inner().store).mark_read(id) {
        broadcast(app);
    }
    horizon_sys::launch::launch(&format!(r"shell:AppsFolder\{app_id}"));
}

/// Écarte une notification : retirée de la liste et du centre de notifications de Windows.
pub fn notifs_dismiss(app: &AppHandle, id: u32) {
    if lock(&app.state::<Notifs>().inner().store).dismiss(id) {
        broadcast(app);
    }
    horizon_sys::notifications::remove(id);
}

/// Coupe (ou rétablit) les notifications d'une appli dans Horizon (pas dans Windows).
pub fn notifs_mute_app(app: &AppHandle, app_id: String, muted: bool) {
    let state = app.state::<Notifs>();
    let (ticket, text) = {
        let mut store = lock(&state.inner().store);
        store.set_muted(&app_id, muted);
        (state.writer.ticket(), serde_json::to_string(&NotifsFile { muted: store.muted() }).unwrap_or_default())
    };
    if let Err(e) = state.writer.write(&path(), ticket, text.as_bytes())
    {
        log::warn!("enregistrement de notifs.json : {e}");
    }
    broadcast(app);
}

// ---- Widget (fil de l'interface) ----

/// Dernière notification, si elle a moins de `RECENT_MS`.
pub fn latest(p: &NotifsPayload, now_ms: u64) -> Option<&Notification> {
    p.recent.first().filter(|n| now_ms.saturating_sub(n.at_ms) < RECENT_MS)
}

/// Pose l'état des notifications dans une barre.
fn apply(window: &BarWindow, p: &NotifsPayload) {
    let g = window.global::<NotifsData>();
    g.set_unread(p.unread.min(i32::MAX as usize) as i32);
    g.set_denied(p.access_denied || p.unavailable);
    apply_latest(window, p, horizon_core::time::now_ms());
}

/// Dernière notification récente (texte d'une barre sans clic en style graphique), relue aussi
/// toutes les 30 s par le widget (`NotifsData.tick`).
fn apply_latest(window: &BarWindow, p: &NotifsPayload, now_ms: u64) {
    let g = window.global::<NotifsData>();
    let Some(n) = latest(p, now_ms) else {
        g.set_has_latest(false);
        return;
    };
    // Logo de l'appli (Simple Icons, `AppMark.svelte`) ; sinon une cloche.
    let logo = brands::by_app_name(&n.app_name);
    g.set_who(n.title.as_str().into());
    g.set_what(n.body.replace('\n', " ").into());
    g.set_when(crate::ui::time::ago(n.at_ms, now_ms).into());
    g.set_has_logo(logo.is_some());
    if let Some(b) = logo {
        g.set_logo(brands::image(&b));
        g.set_logo_tinted(brands::readable_on_dark(b.hex).is_some());
        g.set_logo_color(brands::readable_on_dark(b.hex).unwrap_or_default());
    }
    g.set_has_latest(true);
}

/// Relie le widget d'une barre au service (fil de l'interface).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _bar: &BarLayout) {
    apply(window, &notifs_state(app));
    let w = window.as_weak();
    crate::host::listen(EV_NOTIFS, Some(label), move |e| {
        if let (Some(p), Some(w)) = (e.payload::<NotifsPayload>(), w.upgrade()) {
            apply(&w, p);
        }
    });
    let g = window.global::<NotifsData>();
    // « Ne pas déranger » : lune à la place de la cloche.
    g.set_dnd(crate::dnd::dnd_state(app).active);
    let w = window.as_weak();
    crate::host::listen(EV_DND, Some(label), move |e| {
        if let (Some(v), Some(w)) = (e.payload::<DndView>(), w.upgrade()) {
            w.global::<NotifsData>().set_dnd(v.active);
        }
    });
    let (a, w) = (app.clone(), window.as_weak());
    g.on_tick(move || {
        if let Some(w) = w.upgrade() {
            apply_latest(&w, &notifs_state(&a), horizon_core::time::now_ms());
        }
    });
    // Clic sur la cloche : tout est marqué comme lu (le panneau s'épingle aussi). Le
    // service prend un verrou et diffuse : hors du fil de l'interface.
    let a = app.clone();
    g.on_mark_read(move || {
        let a = a.clone();
        let _ = crate::util::start_thread("notifs-lues", move || notifs_mark_read(&a));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(at_ms: u64) -> Notification {
        Notification { id: 1, app_name: "Discord".into(), app_id: "d".into(), title: "T".into(), body: "B".into(), at_ms }
    }

    #[test]
    fn latest_only_while_recent() {
        let now = 10_000_000_000;
        let p = NotifsPayload {
            unread: 1,
            recent: vec![notif(now - 60_000)],
            unread_ids: vec![1],
            muted: vec![],
            access_denied: false,
            unavailable: false,
        };
        assert!(latest(&p, now).is_some());
        assert!(latest(&p, now + RECENT_MS).is_none());
        assert!(latest(&NotifsPayload { recent: vec![], ..p }, now).is_none());
    }
}
