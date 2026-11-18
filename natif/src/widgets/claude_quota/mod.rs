//! Quota de l'abonnement Claude, relu toutes les 5 minutes et à la sortie de veille. Le jeton OAuth est lu dans les
//! identifiants locaux de Claude Code, envoyé seulement à l'API d'Anthropic, jamais journalisé.
//!
//! Repris de `app/src-tauri/src/widgets/claude_quota.rs`. Partie interface : `ui` (barre) et `panel` (panneau).

mod panel;
pub mod ui;

use crate::util::{lock, spawn_named, start_thread};
use horizon_core::quota::{Quota, access_token, parse_usage};
use horizon_core::time::now_ms;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use crate::host::{AppHandle, Emitter, Manager};
use crate::widgets::Backend;

/// Quota Claude.
pub const EV_QUOTA: &str = "horizon://quota";

pub const BACKEND: Backend = Backend {
    id: "claude_quota",
    name: "quota Claude",
    manage: |app| crate::widgets::manage(app, QuotaState::default()),
    start: Some(start),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};


const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const BETA: &str = "oauth-2025-04-20";
const PERIOD: Duration = Duration::from_secs(300);

/// Le panneau redemande le quota à son ouverture, au plus une fois par minute.
const REFRESH_MIN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct QuotaPayload {
    pub quota: Option<Quota>,
    /// Dernière erreur (affichée discrètement), sans jamais le jeton.
    pub error: Option<String>,
    /// Heure (ms Unix) de la dernière réponse reçue.
    pub fetched_at_ms: Option<u64>,
}

#[derive(Default)]
pub struct QuotaState {
    payload: Mutex<QuotaPayload>,
    /// Dernière demande à l'API (réussie ou non) : limite les rafraîchissements du panneau.
    last_try: Mutex<Option<Instant>>,
}

/// Interroge l'API, garde le résultat et le diffuse aux fenêtres.
fn refresh(app: &AppHandle) {
    *lock(&app.state::<QuotaState>().inner().last_try) = Some(Instant::now());
    let result = fetch();
    let payload = {
        let state = app.state::<QuotaState>();
        let mut p = lock(&state.payload);
        match result {
            // On garde la dernière valeur connue en cas d'erreur passagère.
            Ok(q) => *p = QuotaPayload { quota: Some(q), error: None, fetched_at_ms: Some(now_ms()) },
            Err(e) => {
                log::info!("quota Claude : {e}");
                p.error = Some(e);
            }
        }
        p.clone()
    };
    let _ = app.emit(EV_QUOTA, payload);
}

fn credentials_path() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(".claude")))?;
    Some(dir.join(".credentials.json"))
}

fn fetch() -> Result<Quota, String> {
    let path = credentials_path().ok_or("dossier de Claude Code introuvable")?;
    let text = std::fs::read_to_string(&path).map_err(|_| "Claude Code n’est pas connecté".to_string())?;
    let token = access_token(&text)?;
    let body = ureq::get(URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", BETA)
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(401) => "jeton expiré : relance Claude Code".to_string(),
            ureq::Error::StatusCode(c) => format!("réponse {c} de l’API"),
            _ => "API injoignable".to_string(),
        })?
        .body_mut()
        .read_to_string()
        .map_err(|_| "réponse illisible".to_string())?;
    parse_usage(&body).map_err(|e| e.to_string())
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("claude-quota", move || {
        loop {
            refresh(&app);
            crate::resume::sleep(PERIOD);
        }
    })
}


/// Dernière erreur et heure de la dernière réponse reçue (page « État »).
pub fn health(app: &AppHandle) -> (Option<String>, Option<u64>) {
    let p = lock(&app.state::<QuotaState>().inner().payload);
    (p.error.clone(), p.fetched_at_ms)
}

pub fn quota_state(app: AppHandle) -> QuotaPayload {
    lock(&app.state::<QuotaState>().inner().payload).clone()
}

/// Ouverture du panneau : relit le quota s'il date de plus d'une minute (hors du fil appelant).
pub fn quota_refresh(app: AppHandle) {
    // Test et marque sous le même verrou : deux ouvertures rapprochées ne lancent qu'une requête.
    let claimed = claim(&mut lock(&app.state::<QuotaState>().inner().last_try), Instant::now());
    if claimed {
        spawn_named("quota-lecture", move || refresh(&app));
    }
}

/// Vrai (et `last` marqué à `now`) si la dernière demande date d'au moins `REFRESH_MIN`.
fn claim(last: &mut Option<Instant>, now: Instant) -> bool {
    if last.is_some_and(|t| now.saturating_duration_since(t) < REFRESH_MIN) {
        return false;
    }
    *last = Some(now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_is_claimed_once_per_minute() {
        let t0 = Instant::now();
        let mut last = None;
        assert!(claim(&mut last, t0));
        assert!(!claim(&mut last, t0 + Duration::from_secs(10)), "seconde ouverture rapprochée");
        assert!(claim(&mut last, t0 + REFRESH_MIN));
    }
}
