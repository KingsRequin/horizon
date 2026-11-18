//! Panneau des notifications (global `NotifsPanelData` de
//! `ui/widgets/notifications/panel.slint`) : liste reçue du service, ancienneté revue toutes les
//! 30 s panneau ouvert (`every(30_000)` de la version Tauri), « Ne pas déranger » (`crate::dnd`),
//! actions vers le service. Fil de l'interface.
//!
//! Sans identité de paquet (exe lancé hors du dossier d'installation, voir l'en-tête du module),
//! la lecture échoue : le panneau montre « momentanément indisponibles ».

use super::{EV_NOTIFS, NotifsPayload};
use crate::brands;
use crate::dnd::DndView;
use crate::events::EV_DND;
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{NotifsPanelData, NotifsPanelItem, NotifsPanelMuted, format, model, time};
use crate::widgets::Panel;
use horizon_core::notifs::Notification;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

/// Revue de l'ancienneté, panneau ouvert.
const REFRESH: Duration = Duration::from_secs(30);

/// Logo d'une appli : image, et teinte si elle est lisible sur fond sombre.
type Logo = Option<(slint::Image, Option<slint::Color>)>;

thread_local! {
    static LAST: RefCell<Option<NotifsPayload>> = const { RefCell::new(None) };
    static ITEMS: Rc<VecModel<NotifsPanelItem>> = Rc::new(VecModel::default());
    static MUTED: Rc<VecModel<NotifsPanelMuted>> = Rc::new(VecModel::default());
    /// Logos déjà dessinés, par nom d'appli.
    static LOGOS: RefCell<HashMap<String, Logo>> = RefCell::default();
    static TICK: slint::Timer = slint::Timer::default();
}

fn with_data(mut f: impl FnMut(NotifsPanelData<'_>)) {
    panel_hosts::each("notifications", |h| f(h.global::<NotifsPanelData>()));
}

/// Action vers le service, hors du fil de l'interface (verrous, Windows, Discord et Prisme pour
/// « Ne pas déranger »).
fn act(app: &AppHandle, f: impl FnOnce(&AppHandle) + Send + 'static) {
    let a = app.clone();
    crate::util::spawn_named("notifs-action", move || f(&a));
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<NotifsPanelData>();
    g.set_items(ModelRc::from(ITEMS.with(Rc::clone)));
    g.set_muted(ModelRc::from(MUTED.with(Rc::clone)));
    on_notifs(&super::notifs_state(app));
    on_dnd(&crate::dnd::dnd_state(app));
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_NOTIFS, Some(LABEL), |e| {
            if let Some(p) = e.payload::<NotifsPayload>() {
                on_notifs(p);
            }
        });
        host::listen(EV_DND, Some(LABEL), |e| {
            if let Some(v) = e.payload::<DndView>() {
                on_dnd(v);
            }
        });
    }

    let a = app.clone();
    g.on_mark_read(move || act(&a, super::notifs_mark_read));
    let a = app.clone();
    g.on_toggle_dnd(move || act(&a, crate::dnd::dnd_toggle));
    let a = app.clone();
    g.on_open(move |id, app_id| {
        let (id, app_id) = (id as u32, app_id.to_string());
        act(&a, move |a| super::notifs_open(a, id, app_id));
    });
    let a = app.clone();
    g.on_dismiss(move |id| act(&a, move |a| super::notifs_dismiss(a, id as u32)));
    let a = app.clone();
    g.on_mute(move |app_id, muted| {
        let app_id = app_id.to_string();
        act(&a, move |a| super::notifs_mute_app(a, app_id, muted));
    });
}

fn opened(app: &AppHandle, _kind: &str) {
    on_notifs(&super::notifs_state(app));
    TICK.with(|t| t.start(slint::TimerMode::Repeated, REFRESH, show));
}

fn closed(_app: &AppHandle, _kind: &str) {
    TICK.with(slint::Timer::stop);
}

fn on_notifs(p: &NotifsPayload) {
    LAST.set(Some(p.clone()));
    show();
}

fn on_dnd(v: &DndView) {
    with_data(|g| {
        g.set_dnd(v.active);
        g.set_dnd_why(v.why().into());
    });
}

fn logo(app_name: &str) -> Logo {
    LOGOS.with_borrow_mut(|cache| {
        cache
            .entry(app_name.to_owned())
            .or_insert_with(|| brands::by_app_name(app_name).map(|b| (brands::image(&b), brands::readable_on_dark(b.hex))))
            .clone()
    })
}

fn item(n: &Notification, p: &NotifsPayload, now: u64) -> NotifsPanelItem {
    let logo = logo(&n.app_name);
    NotifsPanelItem {
        id: n.id as i32,
        app_id: n.app_id.as_str().into(),
        app: n.app_name.as_str().into(),
        title: n.title.as_str().into(),
        body: n.body.as_str().into(),
        when: time::ago(n.at_ms, now).into(),
        unread: p.unread_ids.contains(&n.id),
        muted: p.muted.contains(&n.app_id),
        has_logo: logo.is_some(),
        logo_tinted: logo.as_ref().is_some_and(|(_, c)| c.is_some()),
        logo_color: logo.as_ref().and_then(|(_, c)| *c).unwrap_or_default(),
        logo: logo.map(|(i, _)| i).unwrap_or_default(),
    }
}

/// Nom affiché d'une appli coupée : celui de sa dernière notification, sinon tiré de
/// l'identifiant.
fn muted_name(app_id: &str, recent: &[Notification]) -> String {
    recent.iter().find(|n| n.app_id == app_id).map_or_else(|| format::app_name(app_id), |n| n.app_name.clone())
}

/// Pose la dernière liste reçue (et l'ancienneté à l'instant).
fn show() {
    let Some(p) = LAST.with_borrow(Clone::clone) else { return };
    let now = horizon_core::time::now_ms();
    let items: Vec<NotifsPanelItem> = p.recent.iter().map(|n| item(n, &p, now)).collect();
    let muted: Vec<NotifsPanelMuted> =
        p.muted.iter().map(|id| NotifsPanelMuted { id: id.as_str().into(), name: muted_name(id, &p.recent).into() }).collect();
    ITEMS.with(|m| model::sync(m, items));
    MUTED.with(|m| model::sync(m, muted));
    with_data(|g| {
        g.set_unread(p.unread.min(i32::MAX as usize) as i32);
        g.set_access_denied(p.access_denied);
        g.set_unavailable(p.unavailable);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notif(id: u32, app_id: &str, app_name: &str) -> Notification {
        Notification { id, app_name: app_name.into(), app_id: app_id.into(), title: "T".into(), body: "B".into(), at_ms: 0 }
    }

    #[test]
    fn muted_apps_are_named() {
        let recent = [notif(1, "com.squirrel.Discord.Discord", "Discord")];
        assert_eq!(muted_name("com.squirrel.Discord.Discord", &recent), "Discord");
        assert_eq!(muted_name("Spotify.exe", &recent), "Spotify");
    }
}
