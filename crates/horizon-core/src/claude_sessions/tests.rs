//! Tests des sessions Claude Code.

use super::*;
use super::hook::{MAX_DETAIL, MAX_MESSAGE, first_line, french_message, tool_detail};
use super::tracker::{MAX_SESSIONS, STALE_MS, folder_name};
use crate::git_status::GitInfo;

fn ev(id: &str, hook: &str) -> HookEvent {
    HookEvent {
        session_id: id.into(),
        cwd: r"E:\Documents\APPLICATION\deskbar".into(),
        hook_event_name: hook.into(),
        message: None,
        notification_type: None,
        last_assistant_message: None,
        error: None,
        source: None,
        tool_name: None,
        tool_input: None,
        transcript_path: None,
        horizon_procs: Vec::new(),
        horizon_window: None,
    }
}

fn notif(id: &str, kind: &str, msg: &str) -> HookEvent {
    HookEvent { notification_type: Some(kind.into()), message: Some(msg.into()), ..ev(id, "Notification") }
}

fn states(t: &mut SessionTracker, now: u64) -> Vec<(String, SessionState)> {
    t.list(now).into_iter().map(|s| (s.id, s.state)).collect()
}

#[test]
fn prompt_then_stop_goes_working_then_done() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "SessionStart"), 0);
    assert_eq!(states(&mut t, 1), vec![("a".into(), SessionState::Done)]);
    t.apply(ev("a", "UserPromptSubmit"), 10);
    assert_eq!(states(&mut t, 11), vec![("a".into(), SessionState::Working)]);
    t.apply(ev("a", "Stop"), 20);
    let l = t.list(21);
    assert_eq!((l[0].state, l[0].since_ms), (SessionState::Done, 20));
    assert_eq!(l[0].name, "deskbar");
}

#[test]
fn notification_sets_waiting_with_message() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    t.apply(notif("a", "permission_prompt", "Claude needs your permission to use Bash"), 5);
    let l = t.list(6);
    assert_eq!(l[0].state, SessionState::Waiting);
    assert_eq!(l[0].message.as_deref(), Some("Demande l’autorisation : Bash"));
    // Réponse donnée : retour au travail, le message disparaît.
    t.apply(ev("a", "PostToolUse"), 7);
    let l = t.list(8);
    assert_eq!((l[0].state, l[0].message.clone()), (SessionState::Working, None));
}

#[test]
fn notification_without_type_is_waiting() {
    // Ancienne version de Claude Code : pas de `notification_type`.
    let mut t = SessionTracker::default();
    let a = t.apply(HookEvent { message: Some("Claude is waiting for your input".into()), ..ev("a", "Notification") }, 0);
    assert_eq!(a.unwrap().message, "Attend ta réponse");
    assert_eq!(states(&mut t, 1), vec![("a".into(), SessionState::Waiting)]);
}

#[test]
fn informative_notifications_do_not_change_state() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    assert_eq!(t.apply(notif("a", "auth_success", "ok"), 5), None);
    assert_eq!(t.apply(notif("a", "agent_completed", "fini"), 6), None);
    assert_eq!(states(&mut t, 7), vec![("a".into(), SessionState::Working)]);
}

#[test]
fn one_alert_per_wait_until_working_again() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    let stop = HookEvent { last_assistant_message: Some("## Fini\n\nTout est vert.".into()), ..ev("a", "Stop") };
    let a = t.apply(stop, 10).expect("alerte au Stop");
    assert_eq!((a.state, a.message.as_str(), a.name.as_str()), (SessionState::Done, "Fini", "deskbar"));
    // Notification d'inactivité 60 s plus tard : même attente, pas de nouvelle alerte.
    assert_eq!(t.apply(notif("a", "idle_prompt", "Claude is waiting for your input"), 60_010), None);
    assert_eq!(states(&mut t, 60_011), vec![("a".into(), SessionState::Waiting)]);
    // Retour au travail puis nouvelle attente : nouvelle alerte.
    t.apply(ev("a", "UserPromptSubmit"), 70_000);
    assert!(t.apply(notif("a", "permission_prompt", "x"), 70_010).is_some());
}

#[test]
fn new_session_does_not_alert_until_it_worked() {
    let mut t = SessionTracker::default();
    assert_eq!(t.apply(ev("a", "SessionStart"), 0), None);
    assert_eq!(t.apply(notif("a", "idle_prompt", "Claude is waiting for your input"), 60_000), None);
    // Session découverte en cours de route (Horizon lancé après Claude Code) : alerte.
    assert!(t.apply(ev("b", "Stop"), 0).is_some());
}

#[test]
fn compaction_does_not_end_work() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    t.apply(HookEvent { source: Some("compact".into()), ..ev("a", "SessionStart") }, 5);
    assert_eq!(states(&mut t, 6), vec![("a".into(), SessionState::Working)]);
}

#[test]
fn stop_failure_reports_error() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    let a = t.apply(HookEvent { error: Some("rate_limit".into()), ..ev("a", "StopFailure") }, 1).unwrap();
    assert_eq!(a.message, "Échec : rate_limit");
}

#[test]
fn stop_without_message_says_done() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    assert_eq!(t.apply(ev("a", "Stop"), 1).unwrap().message, "A terminé");
    assert_eq!(t.list(2)[0].message, None);
}

#[test]
fn tool_use_after_waiting_means_working_again() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "Notification"), 0);
    t.apply(ev("a", "PreToolUse"), 5);
    assert_eq!(states(&mut t, 6), vec![("a".into(), SessionState::Working)]);
}

#[test]
fn session_end_removes() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    t.apply(ev("a", "SessionEnd"), 5);
    assert!(t.list(6).is_empty());
}

#[test]
fn stale_after_6h_removed() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    assert_eq!(t.list(STALE_MS - 1).len(), 1);
    assert!(t.list(STALE_MS + 1).is_empty());
}

#[test]
fn unknown_hook_ignored() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "Inconnu"), 0);
    assert!(t.list(1).is_empty());
    t.apply(ev("a", "UserPromptSubmit"), 2);
    t.apply(ev("a", "Inconnu"), 3);
    assert_eq!(states(&mut t, 4), vec![("a".into(), SessionState::Working)]);
}

#[test]
fn list_sorted_waiting_first() {
    let mut t = SessionTracker::default();
    t.apply(ev("done", "Stop"), 0);
    t.apply(ev("work", "UserPromptSubmit"), 1);
    t.apply(ev("wait", "Notification"), 2);
    let ids: Vec<String> = t.list(3).into_iter().map(|s| s.id).collect();
    assert_eq!(ids, vec!["wait", "work", "done"]);
}

#[test]
fn session_count_is_capped() {
    let mut t = SessionTracker::default();
    for i in 0..(MAX_SESSIONS as u64 + 20) {
        t.apply(ev(&format!("s{i}"), "UserPromptSubmit"), i);
    }
    let l = t.list(1000);
    assert_eq!(l.len(), MAX_SESSIONS);
    // Les plus anciennes sont parties.
    assert!(!l.iter().any(|s| s.id == "s0"));
}

#[test]
fn folder_names() {
    assert_eq!(folder_name(r"E:\Documents\APPLICATION\deskbar"), "deskbar");
    assert_eq!(folder_name("/home/u/projet/"), "projet");
    assert_eq!(folder_name(r"C:\"), "C:");
    assert_eq!(folder_name(""), "Claude Code");
}

#[test]
fn hook_json_is_parsed() {
    let e: HookEvent = serde_json::from_str(
        r#"{"session_id":"x","transcript_path":"t","cwd":"C:\\p\\q","hook_event_name":"Stop","stop_hook_active":false}"#,
    )
    .unwrap();
    assert_eq!(e.hook_event_name, "Stop");
    assert_eq!(folder_name(&e.cwd), "q");
    assert!(e.horizon_procs.is_empty());
}

#[test]
fn notification_json_with_host_processes_is_parsed() {
    let e: HookEvent = serde_json::from_str(
        r#"{"session_id":"x","cwd":"C:\\p","permission_mode":"default","hook_event_name":"Notification",
        "message":"Claude needs your permission to use Bash","title":"Claude Code","notification_type":"permission_prompt",
        "horizon_procs":[{"pid":12,"name":"claude.exe"},{"pid":3,"name":"WindowsTerminal.exe"}]}"#,
    )
    .unwrap();
    assert_eq!(e.notification_type.as_deref(), Some("permission_prompt"));
    assert_eq!(e.horizon_procs[1], HostProc { pid: 3, name: "WindowsTerminal.exe".into() });
    let mut t = SessionTracker::default();
    t.apply(e, 0);
    assert!(t.list(1)[0].can_focus);
    assert_eq!(t.host("x").0.len(), 2);
    // Les événements suivants sans chaîne (outils) la conservent.
    t.apply(ev("x", "PostToolUse"), 2);
    assert_eq!(t.host("x").0.len(), 2);
    assert_eq!(t.host("inconnue"), (Vec::new(), None));
}

#[test]
fn host_window_skips_dead_or_reused_processes() {
    let procs = [
        HostProc { pid: 10, name: "bash.exe".into() },
        HostProc { pid: 20, name: "pwsh.exe".into() },
        HostProc { pid: 30, name: "WindowsTerminal.exe".into() },
    ];
    let names = |pid: u32| match pid {
        10 => Some("bash.exe".to_string()),
        // PID 20 réutilisé par un autre programme qui a une fenêtre : ignoré.
        20 => Some("notepad.exe".to_string()),
        30 => Some("windowsterminal.exe".to_string()),
        _ => None,
    };
    let windows = [(0x200, 20), (0x300, 30), (0x301, 30)];
    assert_eq!(pick_host_window(&procs, None, names, &windows), Some(0x300));
    // Plusieurs fenêtres du terminal : celle de la console du hook.
    assert_eq!(pick_host_window(&procs, Some(0x301), names, &windows), Some(0x301));
    // Fenêtre indiquée disparue : une autre fenêtre du terminal.
    assert_eq!(pick_host_window(&procs, Some(0x999), names, &windows), Some(0x300));
    assert_eq!(pick_host_window(&procs, None, |_| None, &windows), None);
    assert_eq!(pick_host_window(&[], None, names, &windows), None);
}

#[test]
fn console_window_is_kept_with_the_chain() {
    let mut t = SessionTracker::default();
    let e = HookEvent {
        horizon_procs: vec![HostProc { pid: 1, name: "a.exe".into() }],
        horizon_window: Some(0x1234),
        ..ev("a", "UserPromptSubmit")
    };
    t.apply(e, 0);
    assert_eq!(t.host("a").1, Some(0x1234));
}

#[test]
fn messages_are_translated_and_clipped() {
    assert_eq!(french_message("Claude is waiting for your input"), "Attend ta réponse");
    assert_eq!(french_message("Autre chose"), "Autre chose");
    let long = "x".repeat(500);
    assert_eq!(french_message(&long).chars().count(), MAX_MESSAGE);
    assert_eq!(first_line("\n\n**Terminé.** Les tests passent.\nDétail").as_deref(), Some("Terminé. Les tests passent."));
    assert_eq!(first_line("  \n "), None);
}

#[test]
fn git_info_is_shared_by_sessions_of_a_folder() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    t.apply(ev("b", "UserPromptSubmit"), 0);
    assert_eq!(t.cwds(), vec![r"E:\Documents\APPLICATION\deskbar".to_string()]);
    let g = GitInfo { branch: Some("main".into()), changed: 2, ahead: Some(1), behind: Some(0) };
    t.set_git(r"E:\Documents\APPLICATION\deskbar", Some(g.clone()));
    assert!(t.list(1).iter().all(|s| s.git.as_ref() == Some(&g)));
}

#[test]
fn alert_options() {
    let on = AlertOptions { island: true, light: true };
    assert_eq!(AlertOptions::from_widget(Some(&serde_json::Value::Null)), on);
    let v = serde_json::json!({ "alertLight": false });
    assert_eq!(AlertOptions::from_widget(Some(&v)), AlertOptions { island: true, light: false });
    assert_eq!(AlertOptions::from_widget(None), AlertOptions { island: false, light: false });
}

#[test]
fn forgotten_session_disappears_until_it_speaks_again() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    t.apply(ev("b", "UserPromptSubmit"), 0);
    assert!(t.forget("a"));
    assert!(!t.forget("a"));
    assert_eq!(states(&mut t, 1), vec![("b".into(), SessionState::Working)]);
    // Un nouvel événement de la session la fait revenir.
    t.apply(ev("a", "Stop"), 2);
    assert_eq!(t.list(3).len(), 2);
}

fn permission(id: &str, tool: &str, input: serde_json::Value) -> HookEvent {
    HookEvent { tool_name: Some(tool.into()), tool_input: Some(input), ..ev(id, "PermissionRequest") }
}

#[test]
fn permission_request_waits_with_tool_and_detail() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    let a = t.apply(permission("a", "Bash", serde_json::json!({ "command": "npm test", "description": "Tests" })), 5).unwrap();
    assert!(a.permission);
    assert_eq!(a.message, "Autorisation : Bash · npm test");
    let l = t.list(6);
    assert_eq!(l[0].state, SessionState::Waiting);
    assert_eq!(l[0].permission, Some(Permission { tool: "Bash".into(), detail: Some("npm test".into()) }));
    // La notification « permission_prompt » qui suit garde le détail et n'alerte pas.
    assert_eq!(t.apply(notif("a", "permission_prompt", "Claude needs your permission to use Bash"), 6_000), None);
    assert_eq!(t.list(6_001)[0].message.as_deref(), Some("Autorisation : Bash · npm test"));
    // Accordée : l'outil tourne, la demande disparaît.
    t.apply(ev("a", "PostToolUse"), 7_000);
    let l = t.list(7_001);
    assert_eq!((l[0].state, l[0].permission.clone()), (SessionState::Working, None));
}

#[test]
fn permission_request_json_is_parsed() {
    let json = r#"{"session_id":"s","transcript_path":"C:/x/s.jsonl","cwd":"C:/p","permission_mode":"default",
        "hook_event_name":"PermissionRequest","tool_name":"Edit","tool_input":{"file_path":"C:/p/a.rs","old_string":"x"},
        "permission_suggestions":[{"type":"addRules"}]}"#;
    let e: HookEvent = serde_json::from_str(json).unwrap();
    assert_eq!(e.transcript_path.as_deref(), Some("C:/x/s.jsonl"));
    let mut t = SessionTracker::default();
    assert_eq!(t.apply(e, 0).unwrap().message, "Autorisation : Edit · C:/p/a.rs");
    // Sans outil ni paramètres : message générique.
    let a = SessionTracker::default().apply(ev("b", "PermissionRequest"), 0).unwrap();
    assert_eq!(a.message, "Autorisation : outil inconnu");
}

#[test]
fn tool_details() {
    assert_eq!(tool_detail("WebFetch", &serde_json::json!({ "url": "https://a.b", "prompt": "x" })).as_deref(), Some("https://a.b"));
    assert_eq!(tool_detail("mcp__srv__do", &serde_json::json!({ "n": 3, "path": "  /tmp/x " })).as_deref(), Some("/tmp/x"));
    assert_eq!(tool_detail("Bash", &serde_json::json!({ "command": "cd a\n\nnpm test" })).as_deref(), Some("cd a …"));
    assert_eq!(tool_detail("Bash", &serde_json::json!({})), None);
    assert_eq!(tool_detail("X", &serde_json::json!("pas un objet")), None);
    let long = tool_detail("Bash", &serde_json::json!({ "command": "é".repeat(500) })).unwrap();
    assert_eq!(long.chars().count(), MAX_DETAIL);
}

#[test]
fn only_real_questions_are_questions() {
    let mut t = SessionTracker::default();
    // Fin de réponse (même en attendant des sous-agents) : alerte, mais pas une question.
    t.apply(ev("a", "UserPromptSubmit"), 0);
    assert!(!t.apply(ev("a", "Stop"), 10).unwrap().question);
    // Inactivité qui suit une fin de réponse, sans Stop connu avant : pas une question.
    let idle = t.apply(notif("b", "idle_prompt", "Claude is waiting for your input"), 0).unwrap();
    assert!(!idle.question);
    let old = HookEvent { message: Some("Claude is waiting for your input".into()), ..ev("c", "Notification") };
    assert!(!t.apply(old, 0).unwrap().question);
    // Autorisation, dialogue : questions.
    t.apply(ev("a", "UserPromptSubmit"), 20);
    assert!(t.apply(ev("a", "PermissionRequest"), 30).unwrap().question);
    t.apply(ev("a", "PostToolUse"), 40);
    assert!(t.apply(notif("a", "elicitation_dialog", "x"), 50).unwrap().question);
}

#[test]
fn ask_user_question_waits_with_the_question() {
    let mut t = SessionTracker::default();
    t.apply(ev("a", "UserPromptSubmit"), 0);
    let input = serde_json::json!({ "questions": [{ "question": "Quelle couleur ?", "options": [] }] });
    let ask = HookEvent { tool_name: Some("AskUserQuestion".into()), tool_input: Some(input), ..ev("a", "PreToolUse") };
    let a = t.apply(ask, 10).expect("alerte à la question");
    assert!(a.question);
    assert_eq!((a.state, a.message.as_str()), (SessionState::Waiting, "Quelle couleur ?"));
    // La demande d'autorisation qui peut suivre ne fait pas une seconde alerte.
    let perm = HookEvent { tool_name: Some("AskUserQuestion".into()), ..ev("a", "PermissionRequest") };
    assert_eq!(t.apply(perm, 11), None);
    // Réponse donnée : retour au travail.
    t.apply(ev("a", "PostToolUse"), 20);
    assert_eq!(states(&mut t, 21), vec![("a".into(), SessionState::Working)]);
    // Un autre outil ne met pas la session en attente.
    let bash = HookEvent { tool_name: Some("Bash".into()), ..ev("a", "PreToolUse") };
    assert_eq!(t.apply(bash, 30), None);
}
