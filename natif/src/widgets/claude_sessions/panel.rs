//! Panneau Sessions Claude Code (global `ClaudeSessionsPanelData` de
//! `ui/widgets/claude_sessions/panel.slint`) : textes de Panel.svelte et de lib/claude.ts
//! (`compactTokens`, `gitSummary`, `STATE_LABEL`). À l'ouverture, l'état git est relu et la
//! consommation du jour redemandée ; les durées avancent chaque seconde panneau ouvert. Le
//! panneau n'envoie rien aux sessions : il met leur terminal au premier plan ou oublie une
//! session morte. Fil de l'interface.

use super::ui::tool_name;
use super::{EV_CLAUDE, EV_CLAUDE_USAGE, claude_focus, claude_forget, claude_refresh_git, claude_usage, sessions};
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{ClaudeSessionsPanelData, ClaudeSessionsPanelProject, ClaudeSessionsPanelRow};
use crate::widgets::Panel;
use horizon_core::claude_sessions::{SessionState, SessionView};
use horizon_core::claude_usage::{SessionUsage, UsageView};
use horizon_core::git_status::GitInfo;
use horizon_core::time::now_ms;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

/// Cadence des durées (`every(1000)`).
const TICK: Duration = Duration::from_secs(1);
/// Projets montrés dans la consommation du jour.
const PROJECTS: usize = 4;

thread_local! {
    static LIST: RefCell<Vec<SessionView>> = RefCell::default();
    static USAGE: RefCell<UsageView> = RefCell::default();
    static ROWS: Rc<VecModel<ClaudeSessionsPanelRow>> = Rc::new(VecModel::default());
    static PROJECTS_MODEL: Rc<VecModel<ClaudeSessionsPanelProject>> = Rc::new(VecModel::default());
    static TIMER: slint::Timer = slint::Timer::default();
}

/// « 830 », « 14 k », « 1,2 M » (`compactTokens`).
pub fn compact_tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{} k", (n as f64 / 1000.0).round() as u64)
    } else {
        let m = (n as f64 / 1_000_000.0 * 10.0).round() / 10.0;
        let s = if m == m.trunc() { format!("{}", m as u64) } else { format!("{m:.1}").replace('.', ",") };
        format!("{s} M")
    }
}

/// « 3 modifiés », « 2 non poussés », « 1 en retard » (`gitSummary` ; rien si tout est propre).
pub fn git_summary(g: &GitInfo) -> Vec<String> {
    let plural = |n: u32| if n > 1 { "s" } else { "" };
    let mut out = Vec::new();
    if g.changed > 0 {
        out.push(format!("{} modifié{}", g.changed, plural(g.changed)));
    }
    if let Some(a) = g.ahead.filter(|&a| a > 0) {
        out.push(format!("{a} non poussé{}", plural(a)));
    }
    if let Some(b) = g.behind.filter(|&b| b > 0) {
        out.push(format!("{b} en retard"));
    }
    out
}

fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::Working => "travaille",
        SessionState::Waiting => "attend",
        SessionState::Done => "terminé",
    }
}

/// Niveau du remplissage du contexte (compactage automatique vers 80 %).
fn fill_level(f: f64) -> i32 {
    if f >= 0.8 {
        2
    } else if f >= 0.6 {
        1
    } else {
        0
    }
}

/// Carte d'une session à l'instant `now`.
pub fn row(s: &SessionView, usage: Option<&SessionUsage>, now: u64) -> ClaudeSessionsPanelRow {
    let waiting = s.state == SessionState::Waiting;
    let permission = s.permission.as_ref().filter(|_| waiting);
    let mut r = ClaudeSessionsPanelRow {
        id: s.id.as_str().into(),
        name: s.name.as_str().into(),
        state: match s.state {
            SessionState::Working => 0,
            SessionState::Waiting => 1,
            SessionState::Done => 2,
        },
        status: format!("{} · {}", state_label(s.state), crate::ui::time::since(s.since_ms, now)).into(),
        can_focus: s.can_focus,
        permission: permission.is_some(),
        tool: permission.map(|p| tool_name(&p.tool)).unwrap_or_default().into(),
        detail: permission.and_then(|p| p.detail.clone()).unwrap_or_default().into(),
        message: s.message.clone().unwrap_or_default().into(),
        ..Default::default()
    };
    if let Some(u) = usage.filter(|u| u.today.fresh() > 0 || u.context.is_some_and(|c| c > 0)) {
        r.usage = format!("{} aujourd’hui", compact_tokens(u.today.fresh())).into();
        if let Some(c) = u.context.filter(|&c| c > 0) {
            let f = c as f64 / u.window.max(1) as f64;
            r.ctx_text = format!("contexte {} %", (f * 100.0).round() as i64).into();
            r.ctx = f as f32;
            r.ctx_level = fill_level(f);
            r.ctx_tip = format!(
                "Contexte de la dernière réponse : {} sur {} (fenêtre supposée)",
                compact_tokens(c),
                compact_tokens(u.window)
            )
            .into();
        }
    }
    if let Some(g) = &s.git {
        r.git = true;
        r.branch = g.branch.clone().unwrap_or_else(|| "tête détachée".into()).into();
        let mut chips = git_summary(g).into_iter();
        r.chip1 = chips.next().unwrap_or_default().into();
        r.chip2 = chips.next().unwrap_or_default().into();
        r.chip3 = chips.next().unwrap_or_default().into();
    }
    r
}

fn with_data(mut f: impl FnMut(ClaudeSessionsPanelData<'_>)) {
    panel_hosts::each("claude_sessions", |h| f(h.global::<ClaudeSessionsPanelData>()));
}

fn show() {
    let now = now_ms();
    let (rows, waiting) = LIST.with_borrow(|list| {
        USAGE.with_borrow(|u| {
            let rows: Vec<_> = list.iter().map(|s| row(s, u.sessions.iter().find(|x| x.id == s.id), now)).collect();
            (rows, list.iter().filter(|s| s.state == SessionState::Waiting).count())
        })
    });
    ROWS.with(|m| crate::ui::model::sync(m, rows));
    let (total, projects) = USAGE.with_borrow(|u| {
        let fresh = u.today.fresh();
        let total = if fresh > 0 {
            format!("{} jetons · {} relus du cache", compact_tokens(fresh), compact_tokens(u.today.cache_read))
        } else {
            String::new()
        };
        let shown = &u.projects[..u.projects.len().min(PROJECTS)];
        let top = shown.iter().map(|p| p.today.fresh()).max().unwrap_or(0).max(1);
        let projects: Vec<_> = shown
            .iter()
            .map(|p| ClaudeSessionsPanelProject {
                name: p.name.as_str().into(),
                value: p.today.fresh() as f32 / top as f32,
                tokens: compact_tokens(p.today.fresh()).into(),
            })
            .collect();
        (total, projects)
    });
    PROJECTS_MODEL.with(|m| crate::ui::model::sync(m, projects));
    with_data(|g| {
        g.set_waiting(if waiting > 0 { format!("{waiting} en attente").into() } else { Default::default() });
        g.set_total(total.as_str().into());
    });
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<ClaudeSessionsPanelData>();
    g.set_sessions(ModelRc::from(ROWS.with(Rc::clone)));
    g.set_projects(ModelRc::from(PROJECTS_MODEL.with(Rc::clone)));
    LIST.set(sessions(app));
    show();
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_CLAUDE, Some(LABEL), |e| {
            if let Some(list) = e.payload::<Vec<SessionView>>() {
                LIST.set(list.clone());
                show();
            }
        });
        host::listen(EV_CLAUDE_USAGE, Some(LABEL), |e| {
            if let Some(u) = e.payload::<UsageView>() {
                USAGE.set(u.clone());
                show();
            }
        });
    }
    let a = app.clone();
    g.on_focus(move |id| {
        with_data(|g| g.set_error(Default::default()));
        // Énumération des fenêtres et mise au premier plan : hors du fil de l'interface.
        let a = a.clone();
        crate::util::spawn_named("claude-focus", move || {
            let err = claude_focus(a, id.to_string()).err().unwrap_or_default();
            let _ = slint::invoke_from_event_loop(move || with_data(|g| g.set_error(err.as_str().into())));
        });
    });
    let a = app.clone();
    g.on_forget(move |id| claude_forget(a.clone(), id.to_string()));
}

/// Ouverture : état git relu (en tâche de fond), consommation redemandée, durées qui avancent.
fn opened(app: &AppHandle, _kind: &str) {
    claude_refresh_git(app.clone());
    USAGE.set(claude_usage(app.clone()));
    with_data(|g| g.set_error(Default::default()));
    show();
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, TICK, show));
}

fn closed(_app: &AppHandle, _kind: &str) {
    TIMER.with(slint::Timer::stop);
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::claude_sessions::Permission;
    use horizon_core::claude_usage::Tokens;

    #[test]
    fn compact_tokens_follows_claude_ts() {
        assert_eq!(compact_tokens(830), "830");
        assert_eq!(compact_tokens(14_200), "14 k");
        assert_eq!(compact_tokens(1_240_000), "1,2 M");
        assert_eq!(compact_tokens(18_000_000), "18 M");
    }

    #[test]
    fn git_summary_follows_claude_ts() {
        let g = |changed, ahead, behind| GitInfo { branch: None, changed, ahead, behind };
        assert_eq!(git_summary(&g(7, Some(2), Some(0))), ["7 modifiés", "2 non poussés"]);
        assert_eq!(git_summary(&g(1, Some(1), Some(3))), ["1 modifié", "1 non poussé", "3 en retard"]);
        assert!(git_summary(&g(0, None, None)).is_empty());
    }

    #[test]
    fn card_texts() {
        let now = 1_000_000;
        let s = SessionView {
            id: "1".into(),
            name: "deskbar".into(),
            cwd: String::new(),
            state: SessionState::Waiting,
            since_ms: now - 95_000,
            message: Some("Autorisation".into()),
            can_focus: true,
            git: Some(GitInfo { branch: None, changed: 2, ahead: None, behind: None }),
            permission: Some(Permission { tool: "Bash".into(), detail: Some("npm test".into()) }),
        };
        let u = SessionUsage {
            id: "1".into(),
            today: Tokens { input: 1_400, output: 160_000, cache_write: 512_000, cache_read: 14_000_000 },
            context: Some(862_000),
            window: 1_000_000,
        };
        let r = row(&s, Some(&u), now);
        assert_eq!(r.status.as_str(), "attend · 1 min");
        assert!(r.permission);
        assert_eq!((r.tool.as_str(), r.detail.as_str()), ("Bash", "npm test"));
        assert_eq!(r.usage.as_str(), "673 k aujourd’hui");
        assert_eq!((r.ctx_text.as_str(), r.ctx_level), ("contexte 86 %", 2));
        assert_eq!((r.branch.as_str(), r.chip1.as_str(), r.chip2.as_str()), ("tête détachée", "2 modifiés", ""));
        // Sans jetons ni contexte : pas de ligne de consommation.
        let r = row(&SessionView { state: SessionState::Done, ..s }, None, now);
        assert!(r.usage.is_empty() && !r.permission);
    }
}
