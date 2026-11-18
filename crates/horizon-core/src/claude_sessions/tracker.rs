//! Sessions suivies : état, depuis quand, message, alerte « Claude attend » (une par
//! attente), oubli des sessions muettes.

use super::hook::{Effect, HookEvent, Permission, effect_of};
use super::host::{HostProc, MAX_PROCS};
use crate::git_status::GitInfo;
use serde::Serialize;
use std::collections::HashMap;

/// Sans nouvelles d'une session depuis ce délai, elle disparaît.
pub(crate) const STALE_MS: u64 = 6 * 3_600_000;
/// Plafond : une source qui invente des sessions ne fait pas grossir la mémoire.
pub(crate) const MAX_SESSIONS: usize = 50;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Working,
    Waiting,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionView {
    pub id: String,
    /// Dernier segment du dossier de travail.
    pub name: String,
    pub cwd: String,
    pub state: SessionState,
    /// Depuis quand la session est dans cet état (millisecondes Unix).
    pub since_ms: u64,
    /// Attente : ce que demande Claude ; terminé : début de sa dernière réponse.
    pub message: Option<String>,
    /// Le terminal qui héberge la session est connu (clic = premier plan).
    pub can_focus: bool,
    pub git: Option<GitInfo>,
    /// Autorisation en attente (outil et détail), tant que la session attend.
    pub permission: Option<Permission>,
}

/// Alerte « Claude attend » à montrer (îlot, lumière).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    pub session_id: String,
    pub name: String,
    pub state: SessionState,
    pub message: String,
    /// L'alerte annonce une autorisation demandée.
    pub permission: bool,
}

#[derive(Debug)]
struct Session {
    view: SessionView,
    seen_ms: u64,
    procs: Vec<HostProc>,
    window: Option<isize>,
    /// Une alerte est déjà partie depuis le dernier passage en « travaille ».
    alerted: bool,
}

#[derive(Debug, Default)]
pub struct SessionTracker {
    sessions: HashMap<String, Session>,
}

/// Dernier segment d'un chemin Windows ou Unix.
pub fn folder_name(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("Claude Code")
        .to_owned()
}

impl SessionTracker {
    /// Applique un événement ; renvoie l'alerte à montrer, s'il y en a une (une seule par
    /// attente : la suivante attend un retour au travail).
    pub fn apply(&mut self, ev: HookEvent, now_ms: u64) -> Option<Alert> {
        let effect = effect_of(&ev);
        if effect == Effect::Remove {
            self.sessions.remove(&ev.session_id);
            return None;
        }
        let exists = self.sessions.contains_key(&ev.session_id);
        let starting = effect == Effect::Start;
        // Notification « permission_prompt » (quelques secondes après la demande) : la demande
        // déjà connue garde son détail, et l'alerte est déjà partie.
        let pending = self.sessions.get(&ev.session_id).is_some_and(|s| s.view.permission.is_some());
        if pending && ev.hook_event_name == "Notification" {
            let s = self.sessions.get_mut(&ev.session_id).expect("session présente");
            s.seen_ms = now_ms;
            return None;
        }
        let (state, message, alert, permission) = match effect {
            Effect::State { state, message, alert, permission } => (state, message, alert, permission),
            // Début d'une session déjà connue (compactage, reprise) ou signe de vie : rien ne
            // change, sauf la date de dernière nouvelle et les processus hôtes.
            Effect::Start | Effect::Alive if exists => {
                let s = self.sessions.get_mut(&ev.session_id).expect("session présente");
                s.seen_ms = now_ms;
                s.set_host(ev.horizon_procs, ev.horizon_window);
                return None;
            }
            // Session nouvelle : signe de vie d'une session inconnue = ignoré.
            Effect::Alive => return None,
            Effect::Start => (SessionState::Done, None, false, None),
            Effect::Remove => unreachable!(),
        };
        let name = folder_name(&ev.cwd);
        let s = self.sessions.entry(ev.session_id.clone()).or_insert_with(|| Session {
            view: SessionView {
                id: ev.session_id.clone(),
                name: name.clone(),
                cwd: ev.cwd.clone(),
                state,
                since_ms: now_ms,
                message: None,
                can_focus: false,
                git: None,
                permission: None,
            },
            seen_ms: now_ms,
            procs: Vec::new(),
            window: None,
            // Une session qui vient de s'ouvrir n'a rien à annoncer.
            alerted: starting,
        });
        if s.view.state != state {
            s.view.state = state;
            s.view.since_ms = now_ms;
        }
        s.view.message = message;
        s.view.permission = permission;
        if !ev.cwd.is_empty() {
            s.view.name = name;
            s.view.cwd = ev.cwd.clone();
        }
        s.set_host(ev.horizon_procs, ev.horizon_window);
        s.seen_ms = now_ms;
        let out = if state == SessionState::Working {
            s.alerted = false;
            None
        } else if alert && !s.alerted {
            s.alerted = true;
            Some(Alert {
                session_id: s.view.id.clone(),
                name: s.view.name.clone(),
                state,
                message: s.view.message.clone().unwrap_or_else(|| match state {
                    SessionState::Waiting => "Attend ta réponse".into(),
                    _ => "A terminé".into(),
                }),
                permission: s.view.permission.is_some(),
            })
        } else {
            None
        };
        self.cap();
        out
    }

    /// Au-delà du plafond, la session la plus anciennement vue disparaît.
    fn cap(&mut self) {
        while self.sessions.len() > MAX_SESSIONS {
            let oldest = self.sessions.iter().min_by_key(|(_, s)| s.seen_ms).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => self.sessions.remove(&k),
                None => break,
            };
        }
    }

    /// Sessions encore actives, triées : en attente, en cours, terminées (les plus récentes d'abord).
    pub fn list(&mut self, now_ms: u64) -> Vec<SessionView> {
        self.sessions.retain(|_, s| now_ms.saturating_sub(s.seen_ms) <= STALE_MS);
        let rank = |s: SessionState| match s {
            SessionState::Waiting => 0,
            SessionState::Working => 1,
            SessionState::Done => 2,
        };
        let mut v: Vec<SessionView> = self.sessions.values().map(|s| s.view.clone()).collect();
        v.sort_by(|a, b| rank(a.state).cmp(&rank(b.state)).then(b.since_ms.cmp(&a.since_ms)));
        v
    }

    /// Oublie une session (« × » du panneau : terminal fermé sans `SessionEnd`) ; elle revient
    /// si elle donne encore des nouvelles. Renvoie faux si elle était inconnue.
    pub fn forget(&mut self, id: &str) -> bool {
        self.sessions.remove(id).is_some()
    }

    /// Processus hôtes d'une session (du plus proche au plus lointain) et fenêtre de sa
    /// console, si connue.
    pub fn host(&self, id: &str) -> (Vec<HostProc>, Option<isize>) {
        self.sessions.get(id).map(|s| (s.procs.clone(), s.window)).unwrap_or_default()
    }

    /// Dossier de travail d'une session.
    pub fn cwd(&self, id: &str) -> Option<String> {
        self.sessions.get(id).map(|s| s.view.cwd.clone()).filter(|c| !c.is_empty())
    }

    /// Dossiers de travail distincts (pour relire l'état git).
    pub fn cwds(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.sessions.values().map(|s| s.view.cwd.clone()).filter(|c| !c.is_empty()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// État git lu pour un dossier : reporté sur toutes les sessions de ce dossier.
    pub fn set_git(&mut self, cwd: &str, git: Option<GitInfo>) {
        for s in self.sessions.values_mut().filter(|s| s.view.cwd == cwd) {
            s.view.git = git.clone();
        }
    }
}

impl Session {
    /// Hôte envoyé par le hook ; les événements d'outils n'en envoient pas : le dernier connu
    /// est gardé.
    fn set_host(&mut self, mut procs: Vec<HostProc>, window: Option<i64>) {
        if procs.is_empty() {
            return;
        }
        procs.truncate(MAX_PROCS);
        self.procs = procs;
        self.window = window.and_then(|w| isize::try_from(w).ok());
        self.view.can_focus = true;
    }
}

/// Réglages d'alerte du widget « Sessions Claude Code » : îlot et lumière, actifs par défaut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertOptions {
    pub island: bool,
    pub light: bool,
}

impl AlertOptions {
    /// Lus dans les réglages d'un widget (`alertIsland`, `alertLight`) ; `None` = pas de
    /// widget Claude sur aucune barre : pas d'alerte.
    pub fn from_widget(settings: Option<&serde_json::Value>) -> AlertOptions {
        let Some(settings) = settings else { return AlertOptions { island: false, light: false } };
        let flag = |k: &str| settings.get(k).and_then(|v| v.as_bool()).unwrap_or(true);
        AlertOptions { island: flag("alertIsland"), light: flag("alertLight") }
    }
}
