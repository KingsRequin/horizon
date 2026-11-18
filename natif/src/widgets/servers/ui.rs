//! Partie interface des Serveurs (`ui/widgets/servers/widget.slint`) : chaque exemplaire
//! demande ses hôtes (`servers_state`, selon ses réglages) à chaque relecture des sondes
//! (`ServersData.revision`), comme Widget.svelte à chaque `horizon://servers`.

use super::{EV_SERVERS, HostView, servers_state};
use crate::host::{self, AppHandle};
use crate::ui::{BarWindow, ServerHostRow, ServersData, ServersGroup};
use horizon_core::homelab::Status;
use horizon_core::layout::BarLayout;
use slint::{ComponentHandle, ModelRc, VecModel};

/// 0 en attente, 1 en ligne, 2 lent, 3 hors ligne.
fn code(s: Status) -> i32 {
    match s {
        Status::Unknown => 0,
        Status::Up => 1,
        Status::Slow => 2,
        Status::Down => 3,
    }
}

/// Gravité pour la couleur d'ensemble (`GRAVITY` de servers.ts : en ligne, en attente, lent,
/// hors ligne).
fn gravity(s: Status) -> u8 {
    match s {
        Status::Up => 0,
        Status::Unknown => 1,
        Status::Slow => 2,
        Status::Down => 3,
    }
}

/// Résumé d'un exemplaire (`summary` de servers.ts) et ses hôtes.
pub fn group(hosts: &[HostView]) -> ServersGroup {
    let worst = hosts.iter().map(|h| h.status).max_by_key(|s| gravity(*s)).unwrap_or(Status::Up);
    let rows: Vec<ServerHostRow> = hosts
        .iter()
        .map(|h| ServerHostRow { id: h.id.as_str().into(), label: h.label.as_str().into(), status: code(h.status) })
        .collect();
    ServersGroup {
        hosts: ModelRc::new(VecModel::from(rows)),
        total: hosts.len() as i32,
        down: hosts.iter().filter(|h| h.status == Status::Down).count() as i32,
        worst: code(worst),
    }
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    let data = window.global::<ServersData>();
    let a = app.clone();
    data.on_group(move |id, _revision| group(&servers_state(a.clone(), id.to_string())));
    let w = window.as_weak();
    host::listen(EV_SERVERS, Some(label), move |_| {
        if let Some(w) = w.upgrade() {
            let d = w.global::<ServersData>();
            d.set_revision(d.get_revision().wrapping_add(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;

    fn host(id: &str, status: Status) -> HostView {
        HostView {
            id: id.into(),
            label: id.into(),
            address: "192.168.1.2".into(),
            ssh: true,
            url: None,
            check: "TCP 22".into(),
            status,
            latency_ms: None,
            since_ms: None,
        }
    }

    #[test]
    fn summary_follows_servers_ts() {
        let g = group(&[host("a", Status::Up), host("b", Status::Unknown), host("c", Status::Slow)]);
        assert_eq!((g.total, g.down, g.worst), (3, 0, 2));
        assert_eq!(g.hosts.row_count(), 3);
        let g = group(&[host("a", Status::Up), host("d", Status::Down)]);
        assert_eq!((g.total, g.down, g.worst), (2, 1, 3));
        let g = group(&[]);
        assert_eq!((g.total, g.worst), (0, 1));
    }
}
