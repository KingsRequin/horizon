//! Configuration partagée par les pages d'édition, enregistrée automatiquement (repris de
//! `apps/prisme/lib/config.svelte.ts`). Chaque modification du brouillon part au moteur après une
//! courte pause (`DEBOUNCE`) ; le moteur la valide, l'applique et l'écrit. Pas de bouton
//! « Enregistrer » : « Annuler » revient en arrière (modifications rapprochées regroupées).
//!
//! Le brouillon est le JSON de `get_config` (`serde_json::Value`) : les champs que l'interface ne
//! connaît pas repartent tels quels. Logique pure (le temps est passé en argument) ; la page
//! (`mod.rs`) tient le minuteur et la connexion.

use serde_json::Value;
use std::time::{Duration, Instant};

/// Pause avant l'envoi d'une modification.
pub const DEBOUNCE: Duration = Duration::from_millis(180);
/// Modifications rapprochées (glisser, curseur) regroupées en une seule étape d'annulation.
const UNDO_GROUP: Duration = Duration::from_millis(1200);
const UNDO_MAX: usize = 50;
/// Durée du message « enregistré ».
pub const SAVED_FOR: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Idle,
    Saving,
    Saved,
    Error(String),
}

#[derive(Debug)]
pub struct ConfigStore {
    draft: Option<Value>,
    saved: String,
    history: Vec<String>,
    last_push: Option<Instant>,
    undoing: bool,
    /// Révision de la config que reflète le brouillon.
    rev: u64,
    pub status: Status,
}

impl Default for ConfigStore {
    fn default() -> Self {
        ConfigStore { draft: None, saved: String::new(), history: Vec::new(), last_push: None, undoing: false, rev: 0, status: Status::Idle }
    }
}

/// Le moteur stocke des f32 : 0.8 arrive en 0.800000011920929. On arrondit à l'affichage.
pub fn clean(v: Value) -> Value {
    match v {
        Value::Number(n) if n.is_f64() => {
            let f = n.as_f64().unwrap_or_default();
            serde_json::Number::from_f64((f * 1e5).round() / 1e5).map_or(Value::Number(n), Value::Number)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(clean).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (k, clean(v))).collect()),
        v => v,
    }
}

impl ConfigStore {
    pub fn draft(&self) -> Option<&Value> {
        self.draft.as_ref()
    }

    /// Brouillon à modifier (la page relance ensuite la pause d'envoi).
    pub fn draft_mut(&mut self) -> Option<&mut Value> {
        self.draft.as_mut()
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Config reçue du moteur (`get_config`).
    pub fn loaded(&mut self, json: Value, rev: u64) {
        let clean = clean(json);
        self.rev = rev;
        self.saved = clean.to_string();
        self.draft = Some(clean);
        self.status = Status::Idle;
    }

    pub fn failed(&mut self, error: String) {
        self.status = Status::Error(error);
        self.undoing = false;
    }

    /// Brouillon à envoyer, s'il diffère de ce qui est enregistré.
    pub fn pending(&self) -> Option<String> {
        let json = self.draft.as_ref()?.to_string();
        (json != self.saved).then_some(json)
    }

    /// L'envoi commence.
    pub fn saving(&mut self) {
        self.status = Status::Saving;
    }

    /// Le moteur a accepté `json` (`rev` : sa nouvelle révision, si elle est donnée).
    pub fn saved(&mut self, json: String, rev: Option<u64>, now: Instant) {
        let previous = std::mem::replace(&mut self.saved, json);
        if let Some(r) = rev {
            self.rev = r;
        }
        let grouped = self.last_push.is_some_and(|t| now.duration_since(t) <= UNDO_GROUP);
        if !self.undoing && !previous.is_empty() && !grouped {
            self.history.push(previous);
            if self.history.len() > UNDO_MAX {
                self.history.remove(0);
            }
        }
        self.last_push = Some(now);
        self.undoing = false;
        self.status = Status::Saved;
    }

    /// Le message « enregistré » s'efface tout seul.
    pub fn saved_expired(&mut self) {
        if self.status == Status::Saved {
            self.status = Status::Idle;
        }
    }

    /// Revient à l'état enregistré précédent (vrai si le brouillon a changé).
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.history.pop() else { return false };
        let Ok(v) = serde_json::from_str(&prev) else { return false };
        self.undoing = true;
        self.last_push = None;
        self.draft = Some(v);
        true
    }

    /// Config modifiée ailleurs (icône, autre fenêtre, script) : le brouillon est à recharger,
    /// sauf si une modification locale est en attente (elle partira et fera foi).
    pub fn should_reload(&self, daemon_rev: u64, waiting: bool) -> bool {
        daemon_rev > self.rev && self.rev > 0 && !waiting && self.status != Status::Saving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> ConfigStore {
        let mut s = ConfigStore::default();
        s.loaded(json!({ "general": { "brightness": 0.800000011920929, "transition_ms": 600 } }), 3);
        s
    }

    #[test]
    fn floats_are_rounded_on_load() {
        let s = store();
        assert_eq!(s.draft().unwrap()["general"]["brightness"], json!(0.8));
        assert_eq!(s.draft().unwrap()["general"]["transition_ms"], json!(600));
        assert!(s.pending().is_none());
    }

    #[test]
    fn edits_are_sent_and_grouped_for_undo() {
        let mut s = store();
        let t0 = Instant::now();
        s.draft_mut().unwrap()["general"]["transition_ms"] = json!(700);
        let j = s.pending().expect("modification à envoyer");
        s.saving();
        s.saved(j, Some(4), t0);
        assert_eq!(s.status, Status::Saved);
        assert!(s.can_undo());
        // Une seconde plus tard : même étape d'annulation.
        s.draft_mut().unwrap()["general"]["transition_ms"] = json!(800);
        let j = s.pending().unwrap();
        s.saved(j, Some(5), t0 + Duration::from_secs(1));
        assert!(s.undo());
        assert_eq!(s.draft().unwrap()["general"]["transition_ms"], json!(600));
        assert!(!s.can_undo());
        // L'annulation part comme une modification, sans s'ajouter à l'historique.
        let j = s.pending().unwrap();
        s.saved(j, Some(6), t0 + Duration::from_secs(5));
        assert!(!s.can_undo());
    }

    #[test]
    fn distant_edits_are_separate_steps() {
        let mut s = store();
        let t0 = Instant::now();
        for (i, v) in [700, 800].into_iter().enumerate() {
            s.draft_mut().unwrap()["general"]["transition_ms"] = json!(v);
            let j = s.pending().unwrap();
            s.saved(j, Some(4 + i as u64), t0 + Duration::from_secs(5 * i as u64));
        }
        assert!(s.undo());
        assert_eq!(s.draft().unwrap()["general"]["transition_ms"], json!(700));
        assert!(s.can_undo());
    }

    #[test]
    fn reload_only_when_changed_elsewhere_and_nothing_waits() {
        let mut s = store();
        assert!(s.should_reload(4, false));
        assert!(!s.should_reload(4, true));
        s.saving();
        assert!(!s.should_reload(4, false));
        assert!(!ConfigStore::default().should_reload(4, false));
    }
}
