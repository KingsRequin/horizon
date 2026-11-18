//! Sessions Claude Code suivies grâce aux hooks : en cours, en attente d'une réponse, terminée.
//! Alerte « Claude attend » (une seule par attente), processus hôtes pour retrouver la fenêtre
//! du terminal, état git du dossier.
//!
//! `hook` : lecture des événements ; `tracker` : état des sessions et alertes ; `host` :
//! fenêtre du terminal d'une session.

mod hook;
mod host;
mod tracker;

pub use hook::{HookEvent, Permission};
pub use host::{HostProc, pick_host_window};
pub use tracker::{folder_name, Alert, AlertOptions, SessionState, SessionTracker, SessionView};

#[cfg(test)]
mod tests;
