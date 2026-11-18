//! Partie interface des Sessions Claude Code (`ui/widgets/claude_sessions/widget.slint`) : les
//! sessions de chaque barre (`ClaudeSessionsData`), textes faits ici comme dans Widget.svelte ;
//! la durée d'attente (« attend · 2 min ») avance toutes les 30 s tant qu'une session attend.

use super::{EV_CLAUDE, claude_focus};
use crate::host::{self, AppHandle};
use crate::ui::model;
use crate::ui::{BarWindow, ClaudeSessionRow, ClaudeSessionsData};
use horizon_core::claude_sessions::{SessionState, SessionView};
use horizon_core::layout::BarLayout;
use horizon_core::time::now_ms;
use slint::{ComponentHandle, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Cadence de la durée d'attente affichée (`every(30_000)` de la version Tauri).
const TICK: Duration = Duration::from_secs(30);

/// Modèles d'une barre (toutes les sessions, sans les terminées).
struct Bound {
    window: slint::Weak<BarWindow>,
    all: Rc<VecModel<ClaudeSessionRow>>,
    active: Rc<VecModel<ClaudeSessionRow>>,
}

thread_local! {
    static BOUND: RefCell<Vec<Bound>> = RefCell::default();
    /// Dernière liste reçue (relue par la minuterie).
    static LAST: RefCell<Vec<SessionView>> = RefCell::default();
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Nom court d'un outil : « mcp__github__create_issue » → « github · create_issue ».
pub fn tool_name(tool: &str) -> String {
    tool.strip_prefix("mcp__")
        .and_then(|rest| rest.split_once("__"))
        .filter(|(server, name)| !server.is_empty() && !name.is_empty())
        .map_or_else(|| tool.to_owned(), |(server, name)| format!("{server} · {name}"))
}

fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "travaille",
        SessionState::Waiting => "attend",
        SessionState::Done => "terminé",
    }
}

/// Ligne d'une session à l'instant `now` (`label` de Widget.svelte, barre normale et large).
pub fn row(s: &SessionView, now: u64) -> ClaudeSessionRow {
    let waiting = s.state == SessionState::Waiting;
    let permission = s.permission.as_ref().filter(|_| waiting);
    let since = crate::ui::time::since(s.since_ms, now);
    let label = match permission {
        Some(p) => format!("{} demande {} · {since}", s.name, tool_name(&p.tool)),
        None if waiting => format!("{} attend · {since}", s.name),
        None => s.name.clone(),
    };
    let label_wide = if waiting { label.clone() } else { format!("{} {}", s.name, state_label(s.state)) };
    let (message, mono) = match permission.and_then(|p| p.detail.clone()) {
        Some(d) => (d, true),
        None => (s.message.clone().unwrap_or_default(), false),
    };
    ClaudeSessionRow {
        id: s.id.as_str().into(),
        label: label.into(),
        label_wide: label_wide.into(),
        message: message.into(),
        mono,
        working: s.state == SessionState::Working,
        waiting,
        done: s.state == SessionState::Done,
        permission: permission.is_some(),
        can_focus: s.can_focus,
    }
}

/// Pose la liste sur toutes les barres liées ; la minuterie ne tourne que si une session attend.
fn show_all(list: &[SessionView]) {
    let now = now_ms();
    let rows: Vec<ClaudeSessionRow> = list.iter().map(|s| row(s, now)).collect();
    let active: Vec<ClaudeSessionRow> = rows.iter().filter(|r| !r.done).cloned().collect();
    BOUND.with_borrow_mut(|b| {
        b.retain(|x| x.window.upgrade().is_some());
        for x in b.iter() {
            model::sync(&x.all, rows.clone());
            model::sync(&x.active, active.clone());
        }
    });
    let waiting = list.iter().any(|s| s.state == SessionState::Waiting);
    TIMER.with(|t| {
        if !waiting {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, TICK, || LAST.with_borrow(|l| show_all(l)));
        }
    });
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    let data = window.global::<ClaudeSessionsData>();
    let (all, all_rc) = model::new();
    let (active, active_rc) = model::new();
    data.set_all(all_rc);
    data.set_active(active_rc);
    let a = app.clone();
    data.on_focus(move |id| {
        // Énumération des fenêtres et mise au premier plan : hors du fil de l'interface.
        let a = a.clone();
        crate::util::spawn_named("claude-focus", move || {
            if let Err(e) = claude_focus(a, id.to_string()) {
                log::info!("terminal de la session : {e}");
            }
        });
    });
    BOUND.with_borrow_mut(|b| b.push(Bound { window: window.as_weak(), all, active }));
    let first = LAST.with_borrow(|l| l.is_empty());
    if first {
        LAST.set(super::sessions(app));
    }
    LAST.with_borrow(|l| show_all(l));
    host::listen(EV_CLAUDE, Some(label), move |e| {
        if let Some(list) = e.payload::<Vec<SessionView>>() {
            // Un seul événement par barre : la liste n'est posée qu'une fois si elle a changé.
            let changed = LAST.with_borrow(|l| l != list);
            if changed {
                LAST.set(list.clone());
                show_all(list);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::claude_sessions::Permission;

    fn session(state: SessionState) -> SessionView {
        SessionView {
            id: "1".into(),
            name: "deskbar".into(),
            cwd: "E:\\deskbar".into(),
            state,
            since_ms: 1_000_000 - 95_000,
            message: Some("Fini.".into()),
            can_focus: true,
            git: None,
            permission: None,
        }
    }

    #[test]
    fn labels_follow_widget_svelte() {
        let now = 1_000_000;
        let r = row(&session(SessionState::Working), now);
        assert_eq!((r.label.as_str(), r.label_wide.as_str()), ("deskbar", "deskbar travaille"));
        let r = row(&session(SessionState::Waiting), now);
        assert_eq!((r.label.as_str(), r.label_wide.as_str()), ("deskbar attend · 1 min", "deskbar attend · 1 min"));
        let mut s = session(SessionState::Waiting);
        s.permission = Some(Permission { tool: "mcp__github__create_issue".into(), detail: Some("titre".into()) });
        let r = row(&s, now);
        assert_eq!(r.label.as_str(), "deskbar demande github · create_issue · 1 min");
        assert!(r.permission && r.mono);
        assert_eq!(r.message.as_str(), "titre");
        let r = row(&session(SessionState::Done), now);
        assert!(r.done && !r.mono);
        assert_eq!(r.label_wide.as_str(), "deskbar terminé");
    }

    #[test]
    fn tool_names_are_shortened() {
        assert_eq!(tool_name("Bash"), "Bash");
        assert_eq!(tool_name("mcp__serveur__outil"), "serveur · outil");
        assert_eq!(tool_name("mcp__x"), "mcp__x");
    }
}
