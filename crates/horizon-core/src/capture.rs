//! Captures d'écran proposées à une session Claude : nouveaux fichiers du dossier des
//! captures, et session à qui les proposer.

use crate::claude_sessions::{SessionState, SessionView};
use std::collections::HashSet;

/// Une capture plus ancienne n'est pas proposée (fichier copié dans le dossier, horloge).
pub const FRESH_MS: u64 = 30_000;
/// Fichiers retenus au plus (le dossier peut en contenir des milliers).
const MAX_SEEN: usize = 4096;

/// Image que Windows (Win+Maj+S, Impr. écran) ou un outil de capture dépose.
pub fn is_capture(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg"].iter().any(|x| lower.ends_with(x))
}

/// Session à qui proposer la capture : celle qui attend (la plus récente), sinon la dernière
/// terminée ; il faut pouvoir ramener son terminal. Une session au travail n'est pas dérangée.
pub fn pick_target(sessions: &[SessionView]) -> Option<&SessionView> {
    let newest = |state| {
        sessions.iter().filter(|s| s.state == state && s.can_focus).max_by_key(|s| s.since_ms)
    };
    newest(SessionState::Waiting).or_else(|| newest(SessionState::Done))
}

/// Fichiers déjà vus du dossier des captures.
#[derive(Debug, Default)]
pub struct NewCaptures {
    seen: HashSet<String>,
    primed: bool,
}

impl NewCaptures {
    /// Liste du dossier (nom, date de modification en ms Unix) ; renvoie les captures apparues
    /// depuis la liste précédente et récentes. La première liste ne propose rien (captures
    /// faites avant le démarrage).
    pub fn scan(&mut self, files: &[(String, u64)], now_ms: u64) -> Vec<String> {
        let mut out = Vec::new();
        for (name, mtime) in files.iter().filter(|(n, _)| is_capture(n)) {
            if self.seen.insert(name.clone()) && self.primed && now_ms.saturating_sub(*mtime) <= FRESH_MS {
                out.push(name.clone());
            }
        }
        self.primed = true;
        if self.seen.len() > MAX_SEEN {
            // Oublie ce qui n'est plus dans le dossier.
            let present: HashSet<&String> = files.iter().map(|(n, _)| n).collect();
            self.seen.retain(|n| present.contains(n));
        }
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: &str, state: SessionState, since_ms: u64, can_focus: bool) -> SessionView {
        SessionView {
            id: id.into(),
            name: id.into(),
            cwd: String::new(),
            state,
            since_ms,
            message: None,
            can_focus,
            git: None,
            permission: None,
        }
    }

    #[test]
    fn waiting_session_first_then_last_done() {
        use SessionState::*;
        let list = [s("a", Done, 50, true), s("b", Waiting, 10, true), s("c", Waiting, 20, false), s("d", Working, 99, true)];
        assert_eq!(pick_target(&list).unwrap().id, "b");
        let list = [s("a", Done, 50, true), s("e", Done, 60, true), s("d", Working, 99, true)];
        assert_eq!(pick_target(&list).unwrap().id, "e");
        assert!(pick_target(&[s("d", Working, 1, true)]).is_none());
        assert!(pick_target(&[]).is_none());
    }

    #[test]
    fn only_new_recent_images_are_offered() {
        let mut n = NewCaptures::default();
        let f = |name: &str, t: u64| (name.to_owned(), t);
        assert!(n.scan(&[f("old.png", 1_000)], 1_000).is_empty(), "première liste : rien");
        let list = [f("old.png", 1_000), f("Capture 1.PNG", 99_000), f("notes.txt", 99_000), f("vieille.jpg", 10_000)];
        assert_eq!(n.scan(&list, 100_000), ["Capture 1.PNG"]);
        assert!(n.scan(&list, 101_000).is_empty(), "déjà vue");
    }
}
