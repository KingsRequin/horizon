//! Tests du widget Serveurs.

use super::*;
use super::ssh_config::glob;
use serde_json::json;

const CONFIG: &str = r#"
# commentaire
Host *.lan
User admin

Host website web
HostName 192.168.1.20
Port 2222
IdentityFile ~/.ssh/cle_secrete

Host nas
  HostName=nas.lan
  User "moi"

Host * !nas
Port 22
User defaut

Host jump-?? bastion*
HostName 10.0.0.1

Match host truc
HostName 1.2.3.4
Host truc
"#;

#[test]
fn hosts_without_wildcards_in_file_order() {
    let h = parse_ssh_config(CONFIG);
    let aliases: Vec<&str> = h.iter().map(|h| h.alias.as_str()).collect();
    assert_eq!(aliases, ["website", "web", "nas", "truc"]);
}

#[test]
fn first_value_wins_and_generic_blocks_apply() {
    let h = parse_ssh_config(CONFIG);
    let web = &h[0];
    assert_eq!(web.hostname.as_deref(), Some("192.168.1.20"));
    assert_eq!(web.port, Some(2222));
    assert_eq!(web.user.as_deref(), Some("defaut"));
    assert_eq!(h[1].hostname, web.hostname);
    // `nas` : `Host * !nas` ne s'applique pas ; `HostName=` et guillemets acceptés.
    let nas = &h[2];
    assert_eq!(nas.hostname.as_deref(), Some("nas.lan"));
    assert_eq!(nas.port, None);
    assert_eq!(nas.user.as_deref(), Some("moi"));
    // Le bloc `Match` n'est pas évalué.
    assert_eq!(h[3].hostname, None);
}

#[test]
fn global_lines_and_hostname_token() {
    let h = parse_ssh_config("Port 2200\nHost box\n  HostName %h.maison\n");
    assert_eq!(h, [SshHost { alias: "box".into(), hostname: Some("box.maison".into()), port: Some(2200), user: None }]);
}

#[test]
fn identity_files_are_never_kept() {
    let h = parse_ssh_config(CONFIG);
    let dump = format!("{h:?}");
    assert!(!dump.contains("cle_secrete"));
}

#[test]
fn empty_or_garbage_config() {
    assert!(parse_ssh_config("").is_empty());
    assert!(parse_ssh_config("Host *\n  User x\n").is_empty());
    assert_eq!(parse_ssh_config("Host a\n Port pas-un-nombre\n")[0].port, None);
}

#[test]
fn glob_patterns() {
    assert!(glob("*.lan", "nas.LAN"));
    assert!(glob("jump-??", "jump-01"));
    assert!(!glob("jump-??", "jump-1"));
    assert!(glob("*", ""));
    assert!(glob("a*b*c", "axxbyyc"));
    assert!(!glob("a*b", "ac"));
}

fn ssh() -> Vec<SshHost> {
    parse_ssh_config("Host website\n HostName 192.168.1.20\n Port 2222\nHost nas\n HostName nas.lan\n")
}

#[test]
fn targets_from_ssh_with_defaults() {
    let t = targets(&ssh(), &ServersSettings::default());
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].probe, Probe::Tcp { host: "192.168.1.20".into(), port: 2222 });
    assert_eq!(t[0].ssh.as_deref(), Some("website"));
    assert_eq!(t[1].probe, Probe::Tcp { host: "nas.lan".into(), port: DEFAULT_PORT });
    assert_eq!(t[1].label, "nas");
    assert!(!t[0].hidden && !t[0].manual);
}

#[test]
fn settings_rename_hide_check_and_add() {
    let s = ServersSettings::from_value(&json!({ "hosts": [
        { "id": "website", "label": "Site", "check": "http", "url": "https://site.lan" },
        { "id": "nas", "hidden": true, "port": 5000 },
        { "id": "pi", "address": "192.168.1.30", "url": "javascript:alert(1)" },
        { "id": "vide", "address": "  " },
    ] }));
    let t = targets(&ssh(), &s);
    assert_eq!(t.len(), 3);
    assert_eq!(t[0].label, "Site");
    assert_eq!(t[0].probe, Probe::Http { url: "https://site.lan".into() });
    assert_eq!(t[0].probe.label(), "HTTPS");
    assert!(t[1].hidden);
    assert_eq!(t[1].probe, Probe::Tcp { host: "nas.lan".into(), port: 5000 });
    assert!(t[2].manual);
    assert_eq!(t[2].ssh.as_deref(), Some("192.168.1.30"));
    assert_eq!(t[2].url, None);
}

#[test]
fn http_without_url_falls_back_to_tcp_and_bad_settings_are_ignored() {
    let s = ServersSettings::from_value(&json!({ "hosts": [{ "id": "nas", "check": "http" }] }));
    assert_eq!(targets(&ssh(), &s)[1].probe.label(), "TCP 22");
    assert_eq!(ServersSettings::from_value(&json!("n'importe quoi")), ServersSettings::default());
    assert_eq!(ServersSettings::from_value(&Value::Null), ServersSettings::default());
}

#[test]
fn ssh_targets_cannot_smuggle_options() {
    assert!(safe_ssh_target("website"));
    assert!(safe_ssh_target("moi@192.168.1.2"));
    assert!(!safe_ssh_target("-oProxyCommand=calc"));
    assert!(!safe_ssh_target("a b"));
    assert!(!safe_ssh_target("a\"b"));
    assert!(!safe_ssh_target(""));
    let t = targets(&parse_ssh_config("Host -oProxyCommand=x\n"), &ServersSettings::default());
    assert_eq!(t[0].ssh, None);
}

#[test]
fn one_failure_is_not_enough() {
    let mut h = Health::default();
    assert_eq!(h.status(), Status::Unknown);
    assert_eq!(h.observe(Some(4), 0), None);
    assert_eq!(h.status(), Status::Up);
    assert_eq!(h.observe(None, 60), None);
    assert_eq!(h.status(), Status::Up);
    assert_eq!(h.observe(Some(5), 120), None);
    assert_eq!(h.observe(None, 180), None);
    assert_eq!(h.status(), Status::Up);
}

#[test]
fn two_failures_then_return_are_announced_once() {
    let mut h = Health::default();
    h.observe(Some(4), 0);
    assert_eq!(h.observe(None, 60), None);
    assert_eq!(h.observe(None, 120), Some(Change::WentDown));
    assert_eq!(h.status(), Status::Down);
    assert_eq!(h.since_ms(), Some(120));
    assert_eq!(h.latency_ms(), None);
    assert_eq!(h.observe(None, 180), None);
    assert_eq!(h.observe(Some(7), 240), Some(Change::CameBack));
    assert_eq!(h.since_ms(), Some(240));
    assert_eq!(h.observe(Some(7), 300), None);
}

#[test]
fn host_down_at_startup_is_not_announced() {
    let mut h = Health::default();
    assert_eq!(h.observe(None, 0), None);
    assert_eq!(h.status(), Status::Unknown);
    assert_eq!(h.observe(None, 60), None);
    assert_eq!(h.status(), Status::Down);
    assert_eq!(h.observe(Some(3), 120), None);
    assert_eq!(h.status(), Status::Up);
}

#[test]
fn slow_latency() {
    let mut h = Health::default();
    h.observe(Some(SLOW_MS), 0);
    assert_eq!(h.status(), Status::Slow);
    h.observe(Some(SLOW_MS - 1), 1);
    assert_eq!(h.status(), Status::Up);
}
