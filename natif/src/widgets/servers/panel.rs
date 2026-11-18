//! Panneau Serveurs (global `ServersPanelData` de `ui/widgets/servers/panel.slint`), propre à
//! chaque exemplaire (« servers:<id> ») : hôtes de l'exemplaire, relus à chaque sondage
//! (`EV_SERVERS`, envoyé à chaque sondage tant qu'un panneau Serveurs est montré), sondage
//! relancé à l'affichage, « depuis … » tenu à jour chaque seconde panneau montré ; terminal SSH
//! et interface web. Chaque fenêtre (surgissante, tuile) a son exemplaire. Fil de l'interface.

use super::{EV_SERVERS, HostView, servers_refresh, servers_ssh, servers_state};
use crate::host::{self, AppHandle};
use crate::popup::{LABEL, instance_of};
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{ServersPanelData, ServersPanelHost, model, time};
use crate::widgets::Panel;
use horizon_core::homelab::Status;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub(super) const PANEL: Panel =
    Panel { opened: Some(opened), closed: Some(closed), shown: Some(shown), hidden: Some(hidden), ..Panel::new(bind) };

/// Exemplaire montré par une fenêtre, ses hôtes et sa liste.
struct Open {
    id: String,
    hosts: Vec<HostView>,
    model: Rc<VecModel<ServersPanelHost>>,
}

thread_local! {
    /// Par fenêtre (étiquette).
    static OPEN: RefCell<HashMap<String, Open>> = RefCell::default();
    /// « depuis … » mis à jour chaque seconde, panneau montré.
    static TICK: slint::Timer = slint::Timer::default();
}

/// Global de la fenêtre `label`.
fn with_data(label: &str, f: impl FnOnce(ServersPanelData<'_>)) {
    if let Some(h) = panel_hosts::host(label) {
        f(h.global::<ServersPanelData>());
    }
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<ServersPanelData>();
    let (a, l) = (app.clone(), win.label.clone());
    g.on_ssh(move |id| {
        let name = name_of(&l, &id);
        with_data(&l, |g| g.set_error("".into()));
        if let Err(e) = servers_ssh(a.clone(), id.to_string()) {
            with_data(&l, |g| g.set_error(format!("{name} : {e}").into()));
        }
    });
    let l = win.label.clone();
    g.on_web(move |id| {
        let url = OPEN.with_borrow(|o| o.get(&l).and_then(|o| o.hosts.iter().find(|h| h.id == id.as_str()).and_then(|h| h.url.clone())));
        if let Some(url) = url
            && let Err(e) = crate::web::url_open(&url)
        {
            with_data(&l, |g| g.set_error(e.into()));
        }
    });
    // Une seule fois : chaque fenêtre qui montre un exemplaire relit ses hôtes.
    if win.is_popup() {
        let a = app.clone();
        host::listen(EV_SERVERS, Some(LABEL), move |_| load_all(&a));
    }
}

fn name_of(label: &str, id: &str) -> String {
    OPEN.with_borrow(|o| o.get(label).and_then(|o| o.hosts.iter().find(|h| h.id == id).map(|h| h.label.clone()))).unwrap_or_default()
}

/// Une fenêtre montre l'exemplaire `kind` : hôtes lus, sondage relancé.
fn shown(app: &AppHandle, win: &Host, kind: &str) {
    let model: Rc<VecModel<ServersPanelHost>> = Rc::default();
    let g = win.global::<ServersPanelData>();
    g.set_hosts(ModelRc::from(model.clone()));
    g.set_loaded(false);
    g.set_error("".into());
    OPEN.with_borrow_mut(|o| o.insert(win.label.clone(), Open { id: instance_of(kind).to_owned(), hosts: Vec::new(), model }));
    load(app, &win.label);
    servers_refresh(app.clone());
}

fn hidden(_app: &AppHandle, win: &Host, _kind: &str) {
    OPEN.with_borrow_mut(|o| o.remove(&win.label));
}

/// Panneau montré quelque part : durées tenues à jour.
fn opened(_app: &AppHandle, _kind: &str) {
    TICK.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(1), show_all));
}

fn closed(_app: &AppHandle, _kind: &str) {
    TICK.with(slint::Timer::stop);
}

fn load_all(app: &AppHandle) {
    let labels: Vec<String> = OPEN.with_borrow(|o| o.keys().cloned().collect());
    for l in labels {
        load(app, &l);
    }
}

/// Relit les hôtes de l'exemplaire montré par la fenêtre `label`.
fn load(app: &AppHandle, label: &str) {
    let Some(id) = OPEN.with_borrow(|o| o.get(label).map(|o| o.id.clone())) else { return };
    let hosts = servers_state(app.clone(), id);
    OPEN.with_borrow_mut(|o| {
        if let Some(o) = o.get_mut(label) {
            o.hosts = hosts;
        }
    });
    with_data(label, |g| g.set_loaded(true));
    show(label);
}

/// État en clair (`STATUS_LABEL`).
fn status_label(s: Status) -> &'static str {
    match s {
        Status::Unknown => "en attente",
        Status::Up => "en ligne",
        Status::Slow => "lent",
        Status::Down => "hors ligne",
    }
}

fn row(h: &HostView, now_ms: u64) -> ServersPanelHost {
    ServersPanelHost {
        id: h.id.as_str().into(),
        label: h.label.as_str().into(),
        meta: format!("{} · {}", h.address, h.check).into(),
        status: match h.status {
            Status::Unknown => 0,
            Status::Up => 1,
            Status::Slow => 2,
            Status::Down => 3,
        },
        latency: h.latency_ms.map_or_else(|| status_label(h.status).to_owned(), |ms| format!("{ms} ms")).into(),
        since: h.since_ms.map(|s| time::since(s.max(0) as u64, now_ms)).unwrap_or_default().into(),
        url: h.url.clone().unwrap_or_default().into(),
        ssh: h.ssh,
    }
}

fn show_all() {
    let labels: Vec<String> = OPEN.with_borrow(|o| o.keys().cloned().collect());
    for l in labels {
        show(&l);
    }
}

fn show(label: &str) {
    let now = horizon_core::time::now_ms();
    let counts = OPEN.with_borrow(|o| {
        let o = o.get(label)?;
        model::sync(&o.model, o.hosts.iter().map(|h| row(h, now)).collect());
        let h = &o.hosts;
        Some((
            h.len() as i32,
            h.iter().filter(|h| matches!(h.status, Status::Up | Status::Slow)).count() as i32,
            h.iter().filter(|h| h.status == Status::Down).count() as i32,
        ))
    });
    if let Some((total, online, down)) = counts {
        with_data(label, |g| {
            g.set_total(total);
            g.set_online(online);
            g.set_down(down);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_show_latency_or_status_and_since() {
        let mut h = HostView {
            id: "nas".into(),
            label: "NAS".into(),
            address: "192.168.1.30".into(),
            ssh: false,
            url: Some("http://nas.lan:5000".into()),
            check: "HTTP".into(),
            status: Status::Down,
            latency_ms: None,
            since_ms: Some(1_000_000),
        };
        let r = row(&h, 1_000_000 + 12 * 60_000);
        assert_eq!(r.meta.as_str(), "192.168.1.30 · HTTP");
        assert_eq!((r.status, r.latency.as_str()), (3, "hors ligne"));
        assert_eq!(r.since.as_str(), time::since(1_000_000, 1_000_000 + 12 * 60_000));
        h.status = Status::Up;
        h.latency_ms = Some(5);
        h.since_ms = None;
        let r = row(&h, 0);
        assert_eq!((r.latency.as_str(), r.since.as_str()), ("5 ms", ""));
    }
}
