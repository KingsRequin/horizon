//! Réglages Serveurs (`ui/widget-settings/servers.slint`, app/src/widgets/servers/Settings.svelte
//! et `servers.ts`) : hôtes de `~/.ssh/config` complétés par leur réglage, puis hôtes ajoutés à
//! la main. Seuls les hôtes modifiés sont enregistrés (`settings.hosts`) ; hôte ajouté ou test
//! changé : sondage peu après (`servers_refresh`). « Tester » sonde l'hôte tel qu'il est réglé
//! (`servers_test`, hors du fil de l'interface).

use crate::host::AppHandle;
use crate::ui::menu::Entry;
use crate::ui::settings_window::offer_undo;
use crate::ui::{ServerRow, ServersSettingsData, SettingsWindow};
use crate::undoable::Removal;
use crate::widgets::servers::{SshHostView, servers_refresh, servers_ssh_hosts, servers_test};
use horizon_core::homelab::{CheckKind, HostSetting, ServersSettings};
use serde_json::{Value, json};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Sondage après un changement (le temps que l'enregistrement parte).
const REFRESH_DELAY: Duration = Duration::from_millis(800);

/// Ligne de la page : hôte de ssh ou ajouté, avec son réglage.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingRow {
    pub setting: HostSetting,
    pub manual: bool,
    pub ssh: Option<SshHostView>,
}

/// Lignes à afficher : les hôtes de ssh dans l'ordre du fichier, puis les ajouts manuels.
pub fn setting_rows(ssh: &[SshHostView], hosts: &[HostSetting]) -> Vec<SettingRow> {
    let mut rows: Vec<SettingRow> = ssh
        .iter()
        .map(|s| SettingRow {
            setting: hosts
                .iter()
                .find(|h| h.address.as_deref().is_none_or(str::is_empty) && h.id == s.alias)
                .cloned()
                .unwrap_or_else(|| HostSetting { id: s.alias.clone(), ..Default::default() }),
            manual: false,
            ssh: Some(s.clone()),
        })
        .collect();
    rows.extend(hosts.iter().filter(|h| h.address.is_some()).map(|h| SettingRow { setting: h.clone(), manual: true, ssh: None }));
    rows
}

/// Réglages à enregistrer : une ligne de ssh sans aucun changement n'est pas gardée.
pub fn to_settings(rows: &[SettingRow]) -> Vec<HostSetting> {
    let filled = |o: &Option<String>| o.as_deref().is_some_and(|t| !t.trim().is_empty());
    rows.iter()
        .map(|r| r.setting.clone())
        .filter(|s| s.address.is_some() || filled(&s.label) || s.hidden || s.check == CheckKind::Http || s.port.is_some() || filled(&s.url))
        .collect()
}

/// Port saisi : `Some(None)` si le champ est vide (port de ssh, sinon 22), `None` s'il est
/// invalide (pas un entier de 1 à 65535).
pub fn parse_port(text: &str) -> Option<Option<u16>> {
    let t = text.trim();
    if t.is_empty() {
        return Some(None);
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<u16>().ok().filter(|&p| p >= 1).map(Some)
}

/// Identifiant libre pour un hôte ajouté (« hote-5 », « hote-6 »…).
pub fn new_host_id(rows: &[SettingRow]) -> String {
    (rows.len() + 1..).map(|n| format!("hote-{n}")).find(|id| !rows.iter().any(|r| &r.setting.id == id)).unwrap_or_default()
}

/// Résultat du dernier « Tester ».
#[derive(Clone)]
enum Test {
    Pending,
    Ok(u32),
    Error(String),
}

#[derive(Default)]
struct Page {
    ssh: Vec<SshHostView>,
    loaded: bool,
    /// Port invalide saisi, par hôte (rien n'est enregistré).
    bad_port: HashMap<String, bool>,
    tests: HashMap<String, Test>,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
    static REFRESH: Rc<slint::Timer> = Rc::default();
    static OPENING: Cell<u64> = const { Cell::new(0) };
}

fn hosts() -> Vec<HostSetting> {
    ServersSettings::from_value(&super::settings()).hosts
}

fn rows() -> Vec<SettingRow> {
    PAGE.with_borrow(|p| setting_rows(&p.ssh, &hosts()))
}

fn opt(o: &Option<String>) -> slint::SharedString {
    o.as_deref().unwrap_or_default().into()
}

fn refresh() {
    let Some(w) = super::window() else { return };
    let list = rows();
    let view: Vec<ServerRow> = PAGE.with_borrow(|p| {
        list.iter()
            .map(|r| {
                let s = &r.setting;
                let (test, state) = match p.tests.get(&s.id) {
                    None => (String::new(), 0),
                    Some(Test::Pending) => ("Test…".to_owned(), 0),
                    Some(Test::Ok(ms)) => (format!("En ligne : répond en {ms} ms"), 1),
                    Some(Test::Error(e)) => (format!("Hors ligne : {e}"), 2),
                };
                let name = [s.label.as_deref(), s.address.as_deref()].into_iter().flatten().find(|t| !t.is_empty()).unwrap_or(&s.id);
                ServerRow {
                    label: opt(&s.label),
                    address: opt(&s.address),
                    hidden: s.hidden,
                    http: s.check == CheckKind::Http,
                    port: s.port.map(|p| p.to_string()).unwrap_or_default().into(),
                    url: opt(&s.url),
                    manual: r.manual,
                    from: r.ssh.as_ref().map(|h| format!("{}{}", h.address, h.port.map(|p| format!(":{p}")).unwrap_or_default())).unwrap_or_default().into(),
                    name_placeholder: if r.manual { "Nom affiché".into() } else { s.id.as_str().into() },
                    port_placeholder: r.ssh.as_ref().and_then(|h| h.port).unwrap_or(22).to_string().into(),
                    bad_port: p.bad_port.get(&s.id).copied().unwrap_or(false),
                    test: test.into(),
                    test_state: state,
                    what: format!("le serveur « {name} »").into(),
                }
            })
            .collect()
    });
    let g = w.global::<ServersSettingsData>();
    g.set_loaded(PAGE.with_borrow(|p| p.loaded));
    crate::ui::model::sync_rc(&g.get_rows(), view, |m| g.set_rows(m));
}

/// Enregistre les lignes ; le widget sonde peu après.
fn commit(next: &[SettingRow]) {
    let hosts = serde_json::to_value(to_settings(next)).unwrap_or(Value::Array(Vec::new()));
    super::save(json!({ "hosts": hosts }));
    refresh();
    if let Some(app) = super::app() {
        REFRESH.with(|t| t.start(slint::TimerMode::SingleShot, REFRESH_DELAY, move || servers_refresh(app.clone())));
    }
}

fn update(i: usize, f: impl FnOnce(&mut HostSetting)) {
    let mut list = rows();
    if let Some(r) = list.get_mut(i) {
        f(&mut r.setting);
        commit(&list);
    }
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<ServersSettingsData>();
    g.on_text(|i, field, text| {
        let i = i.max(0) as usize;
        let text = text.to_string();
        match field.as_str() {
            "label" => update(i, |s| s.label = Some(text)),
            "address" => update(i, |s| s.address = Some(text)),
            "url" => update(i, |s| s.url = Some(text)),
            "port" => set_port(i, &text),
            _ => {}
        }
    });
    g.on_shown(|i, shown| update(i.max(0) as usize, |s| s.hidden = !shown));
    g.on_pick_check(|i, x, y| {
        let i = i.max(0) as usize;
        let http = rows().get(i).is_some_and(|r| r.setting.check == CheckKind::Http);
        let entries = vec![Entry::check("tcp", "Port TCP", !http), Entry::check("http", "HTTP(S)", http)];
        super::menu_at(x, y, entries, move |v| update(i, |s| s.check = if v == "http" { CheckKind::Http } else { CheckKind::Tcp }));
    });
    g.on_test(|i| test(i.max(0) as usize));
    g.on_add(|| {
        let mut list = rows();
        let id = new_host_id(&list);
        list.push(SettingRow { setting: HostSetting { id, address: Some(String::new()), ..Default::default() }, manual: true, ssh: None });
        commit(&list);
    });
    g.on_remove(|i| remove(i.max(0) as usize));
}

/// La page s'affiche : hôtes de ssh lus hors du fil de l'interface (en échec : liste vide, la
/// page s'affiche quand même).
pub fn show(app: &AppHandle, window: &SettingsWindow, _settings: &Value) {
    OPENING.set(OPENING.get() + 1);
    PAGE.set(Page::default());
    let g = window.global::<ServersSettingsData>();
    g.set_rows(slint::ModelRc::new(slint::VecModel::default()));
    refresh();
    let (app, opening) = (app.clone(), OPENING.get());
    crate::util::spawn_named("reglages-serveurs", move || {
        let ssh = servers_ssh_hosts(app);
        let _ = slint::invoke_from_event_loop(move || {
            if OPENING.get() == opening {
                PAGE.with_borrow_mut(|p| (p.ssh, p.loaded) = (ssh, true));
                refresh();
            }
        });
    });
}

pub fn leave() {
    OPENING.set(OPENING.get() + 1);
}

/// Port saisi : invalide, signalé et pas enregistré.
fn set_port(i: usize, text: &str) {
    let Some(id) = rows().get(i).map(|r| r.setting.id.clone()) else { return };
    let port = parse_port(text);
    PAGE.with_borrow_mut(|p| p.bad_port.insert(id, port.is_none()));
    match port {
        Some(p) => update(i, |s| s.port = p),
        None => refresh(),
    }
}

/// Sonde l'hôte tel qu'il est réglé (hors du fil de l'interface).
fn test(i: usize) {
    let (Some(r), Some(app)) = (rows().get(i).cloned(), super::app()) else { return };
    let id = r.setting.id.clone();
    PAGE.with_borrow_mut(|p| p.tests.insert(id.clone(), Test::Pending));
    refresh();
    let opening = OPENING.get();
    crate::util::spawn_named("serveur-test", move || {
        let result = servers_test(app, r.setting);
        let _ = slint::invoke_from_event_loop(move || {
            if OPENING.get() != opening {
                return;
            }
            let t = match result {
                Ok(ms) => Test::Ok(ms),
                Err(e) => Test::Error(e),
            };
            PAGE.with_borrow_mut(|p| p.tests.insert(id, t));
            refresh();
        });
    });
}

/// Retrait annulable d'un hôte ajouté : il revient à sa place.
fn remove(i: usize) {
    let before = rows();
    let Some(r) = before.get(i) else { return };
    let s = &r.setting;
    let name = [s.label.as_deref(), s.address.as_deref()].into_iter().flatten().find(|t| !t.is_empty()).unwrap_or(&s.id).to_owned();
    let mut next = before.clone();
    next.remove(i);
    commit(&next);
    offer_undo(Removal { label: format!("Serveur « {name} » retiré"), undo: Box::new(move || commit(&before)), commit: None });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(alias: &str, port: Option<u16>) -> SshHostView {
        SshHostView { alias: alias.into(), address: format!("{alias}.lan"), port }
    }

    #[test]
    fn rows_follow_ssh_then_manual_hosts() {
        let hosts = vec![
            HostSetting { id: "nas".into(), label: Some("NAS".into()), ..Default::default() },
            HostSetting { id: "hote-3".into(), address: Some("192.168.1.40".into()), ..Default::default() },
        ];
        let rows = setting_rows(&[ssh("nas", None), ssh("pi", Some(2222))], &hosts);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].setting.label.as_deref(), Some("NAS"));
        assert_eq!(rows[1].setting, HostSetting { id: "pi".into(), ..Default::default() });
        assert!(rows[2].manual && !rows[0].manual);
        // Seuls les hôtes modifiés sont gardés.
        let kept = to_settings(&rows);
        assert_eq!(kept.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["nas", "hote-3"]);
        assert_eq!(new_host_id(&rows), "hote-4");
    }

    #[test]
    fn untouched_ssh_rows_are_not_saved() {
        let mut row = SettingRow { setting: HostSetting { id: "pi".into(), label: Some("  ".into()), ..Default::default() }, manual: false, ssh: None };
        assert!(to_settings(std::slice::from_ref(&row)).is_empty());
        row.setting.check = CheckKind::Http;
        assert_eq!(to_settings(&[row]).len(), 1);
    }

    #[test]
    fn ports_are_checked() {
        assert_eq!(parse_port(""), Some(None));
        assert_eq!(parse_port(" 22 "), Some(Some(22)));
        assert_eq!(parse_port("0"), None);
        assert_eq!(parse_port("65536"), None);
        assert_eq!(parse_port("2a"), None);
        assert_eq!(parse_port("-1"), None);
    }
}
