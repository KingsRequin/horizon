//! Événements des hooks de Claude Code : ce qu'ils changent à une session, messages en
//! français, extraits courts.

use super::host::HostProc;
use super::tracker::SessionState;
use serde::{Deserialize, Serialize};

/// Longueur maximale d'un message gardé (la fin de la réponse de Claude peut être longue).
pub(super) const MAX_MESSAGE: usize = 200;
/// Entrée d'un hook Claude Code, telle que Claude Code l'envoie sur l'entrée standard (les
/// champs absents d'une ancienne version ou d'un ancien hook prennent leur valeur par défaut).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HookEvent {
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    pub hook_event_name: String,
    /// Notification : texte affiché par Claude Code.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub notification_type: Option<String>,
    /// Stop / StopFailure : fin de la dernière réponse.
    #[serde(default)]
    pub last_assistant_message: Option<String>,
    /// StopFailure : cause de l'échec.
    #[serde(default)]
    pub error: Option<String>,
    /// SessionStart : `startup`, `resume`, `clear`, `compact`, `fork`.
    #[serde(default)]
    pub source: Option<String>,
    /// PermissionRequest (et événements d'outils) : outil demandé.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// PermissionRequest : paramètres de l'outil (commande, fichier, adresse…).
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    /// Transcription de la session (`~/.claude/projects/<projet>/<session>.jsonl`).
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Ajouté par `horizon-hook` (absent d'un ancien hook).
    #[serde(default)]
    pub horizon_procs: Vec<HostProc>,
    /// Ajouté par `horizon-hook` : fenêtre de la console du hook (onglet Windows Terminal :
    /// la fenêtre du terminal qui l'affiche), si elle est visible.
    #[serde(default)]
    pub horizon_window: Option<i64>,
}

/// Ce qu'un événement change à la session.
#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    Remove,
    /// Nouvel état ; `alert` = l'événement mérite une alerte (attente, fin de réponse) ;
    /// `question` = Claude ne peut pas continuer sans toi (question, autorisation), à la
    /// différence d'une fin de réponse ; `permission` = autorisation demandée (attente).
    State { state: SessionState, message: Option<String>, alert: bool, question: bool, permission: Option<Permission> },
    /// Début de session : « terminé » si elle est nouvelle, sinon rien ne change (un
    /// `compact` automatique arrive en plein travail).
    Start,
    /// Signe de vie seulement.
    Alive,
}

/// Types de notification qui veulent dire « Claude attend quelqu'un ». Les autres
/// (connexion réussie, quota, extension…) ne changent pas l'état.
const WAITING_NOTIFICATIONS: [&str; 5] =
    ["permission_prompt", "idle_prompt", "elicitation_dialog", "elicitation_url_dialog", "agent_needs_input"];
/// Notification d'inactivité (60 s après une fin de réponse) : rien n'est demandé.
const IDLE_NOTIFICATION: &str = "idle_prompt";
/// Son message, seul repère d'une ancienne version de Claude Code sans `notification_type`.
const IDLE_MESSAGE: &str = "Claude is waiting for your input";
/// Outil de Claude Code qui pose une question à choix et attend la réponse.
const ASK_TOOL: &str = "AskUserQuestion";

pub(super) fn effect_of(ev: &HookEvent) -> Effect {
    let state = |state, message, alert, question| Effect::State { state, message, alert, question, permission: None };
    match ev.hook_event_name.as_str() {
        "SessionEnd" => Effect::Remove,
        "SessionStart" => Effect::Start,
        "PreToolUse" if ev.tool_name.as_deref() == Some(ASK_TOOL) => {
            let asked = ev.tool_input.as_ref().and_then(asked_question);
            state(SessionState::Waiting, Some(asked.unwrap_or_else(|| "Te pose une question".into())), true, true)
        }
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => state(SessionState::Working, None, false, false),
        "Notification" => match ev.notification_type.as_deref() {
            // Ancienne version sans type : toute notification était une attente.
            Some(t) if !WAITING_NOTIFICATIONS.contains(&t) => Effect::Alive,
            kind => {
                let idle = match kind {
                    Some(t) => t == IDLE_NOTIFICATION,
                    None => ev.message.as_deref().map(str::trim) == Some(IDLE_MESSAGE),
                };
                state(
                    SessionState::Waiting,
                    Some(ev.message.as_deref().map(french_message).unwrap_or_else(|| "Attend ta réponse".into())),
                    true,
                    !idle,
                )
            }
        },
        "PermissionRequest" => {
            let p = Permission::of(ev);
            Effect::State {
                state: SessionState::Waiting,
                message: Some(p.summary()),
                alert: true,
                question: true,
                permission: Some(p),
            }
        }
        "Stop" => state(SessionState::Done, ev.last_assistant_message.as_deref().and_then(first_line), true, false),
        "StopFailure" => state(
            SessionState::Done,
            Some(format!("Échec : {}", ev.error.as_deref().unwrap_or("erreur inconnue"))),
            true,
            false,
        ),
        _ => Effect::Alive,
    }
}

/// Première question posée par `AskUserQuestion` (`questions[0].question`), raccourcie.
fn asked_question(input: &serde_json::Value) -> Option<String> {
    let q = input.get("questions")?.get(0)?.get("question")?.as_str()?;
    first_line(q)
}

/// Longueur maximale du détail d'une autorisation (commande, chemin, adresse).
pub(super) const MAX_DETAIL: usize = 160;

/// Autorisation demandée par Claude Code (`PermissionRequest`) : l'outil et ce qu'il va faire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Permission {
    /// Nom de l'outil (« Bash », « Edit », « mcp__serveur__outil »).
    pub tool: String,
    /// Commande, fichier, adresse… sur une ligne, raccourci ; `None` si rien de parlant.
    pub detail: Option<String>,
}

impl Permission {
    pub(super) fn of(ev: &HookEvent) -> Permission {
        let tool = ev.tool_name.as_deref().map(str::trim).filter(|t| !t.is_empty()).unwrap_or("outil inconnu");
        Permission { tool: tool.to_owned(), detail: ev.tool_input.as_ref().and_then(|i| tool_detail(tool, i)) }
    }

    /// Message de la session et de l'alerte : « Autorisation : Bash · npm test ».
    pub(super) fn summary(&self) -> String {
        match &self.detail {
            Some(d) => clip(&format!("Autorisation : {} · {d}", self.tool)),
            None => format!("Autorisation : {}", self.tool),
        }
    }
}

/// Ce que l'outil va faire, d'après ses paramètres (format propre à chaque outil ; un outil
/// inconnu donne son premier paramètre texte).
pub(crate) fn tool_detail(tool: &str, input: &serde_json::Value) -> Option<String> {
    let field = |k: &str| input.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let keys: &[&str] = match tool {
        "Bash" | "PowerShell" | "Monitor" => &["command"],
        "Edit" | "MultiEdit" | "Write" | "Read" | "NotebookEdit" => &["file_path", "notebook_path"],
        "WebFetch" => &["url"],
        "WebSearch" => &["query"],
        "Glob" | "Grep" => &["pattern"],
        "Task" | "Agent" => &["description"],
        _ => &[],
    };
    let found = keys.iter().find_map(|k| field(k)).or_else(|| {
        input.as_object()?.values().find_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
    })?;
    // Une seule ligne : une commande sur plusieurs lignes est résumée à la première.
    let first = found.lines().map(str::trim).find(|l| !l.is_empty())?;
    let more = found.lines().filter(|l| !l.trim().is_empty()).count() > 1;
    let line = if more { format!("{first} …") } else { first.to_owned() };
    Some(clip_to(&line, MAX_DETAIL))
}

/// Messages connus de Claude Code, en français ; les autres restent tels quels.
pub(crate) fn french_message(msg: &str) -> String {
    let m = msg.trim();
    if m == "Claude is waiting for your input" {
        return "Attend ta réponse".into();
    }
    if let Some(tool) = m.strip_prefix("Claude needs your permission to use ") {
        return format!("Demande l’autorisation : {tool}");
    }
    if m == "Claude Code needs your attention" || m == "Claude needs your attention" {
        return "Demande ton attention".into();
    }
    clip(m)
}

/// Première ligne non vide d'un texte, sans marques Markdown (titre, citation, liste, gras,
/// code), raccourcie.
pub(crate) fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(|l| l.replace("**", "").replace('`', ""))
        .map(|l| l.trim().trim_start_matches(['#', '>', '*', '-', ' ']).trim().to_owned())
        .find(|l| !l.is_empty())
        .map(|l| clip(&l))
}

fn clip(s: &str) -> String {
    clip_to(s, MAX_MESSAGE)
}

fn clip_to(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}
