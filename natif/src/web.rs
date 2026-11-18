//! Accès au web, partagé par les widgets en ligne (Météo, Agenda) et la mise à jour : requête
//! HTTP bloquante bornée, ouverture d'un lien dans le navigateur.
//!
//! Repris de `app/src-tauri/src/web.rs`. Différence : pas de `blocking` (runtime asynchrone de
//! Tauri) ; les appelants sont déjà sur leur propre fil.

use std::time::Duration;

/// Lit `url` (15 s au plus, `limit` octets au plus) ; l'erreur est un texte court pour
/// l'interface, sans l'adresse (les adresses d'agenda sont secrètes).
pub fn get(url: &str, limit: u64) -> Result<String, String> {
    call(url, Duration::from_secs(15))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|_| "réponse illisible ou trop grosse".to_string())
}

/// Comme `get`, en octets et en 5 minutes au plus (exécutables de la mise à jour).
pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    call(url, Duration::from_secs(300))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|_| "réponse interrompue ou trop grosse".to_string())
}

fn call(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, String> {
    ureq::get(url)
        .header("User-Agent", concat!("Horizon/", env!("CARGO_PKG_VERSION")))
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "adresse introuvable (404)".to_string(),
            ureq::Error::StatusCode(c) => format!("réponse {c}"),
            _ => "serveur injoignable".to_string(),
        })
}

/// Ouvre un lien http(s) dans le navigateur par défaut (lien de visioconférence, interface web
/// d'un serveur).
pub fn url_open(url: &str) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("lien refusé".into());
    }
    horizon_sys::launch::launch(url);
    Ok(())
}
