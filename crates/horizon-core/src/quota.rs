//! Quota de l'abonnement Claude (réponse de `api.anthropic.com/api/oauth/usage`).

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaWindow {
    /// « 5 h », « Semaine », ou le nom d'un modèle (limite hebdomadaire propre).
    pub label: String,
    /// `five_hour`, `seven_day` ou `model` (limite propre à un modèle).
    pub kind: &'static str,
    pub used_pct: f32,
    /// Date de remise à zéro (ISO 8601), si connue.
    pub resets_at: Option<String>,
    /// Niveau donné par l'API (`normal`, `warning`, `critical`…), si connu.
    pub severity: Option<String>,
    /// Limite d'un modèle : part de la semaine générale déjà consommée par ce modèle, et part
    /// qui lui est allouée (en % de la semaine).
    pub share: Option<Share>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Share {
    pub used_pct: f32,
    pub allowance_pct: f32,
}

/// Part de la semaine consommée par une surface (Claude Code, chats…).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BreakdownRow {
    pub label: String,
    pub pct: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quota {
    pub windows: Vec<QuotaWindow>,
    /// Début de la fenêtre hebdomadaire (ISO 8601) : sert à calculer le rythme.
    pub week_started_at: Option<String>,
    /// Répartition de la semaine par surface, dans l'ordre de l'API.
    pub breakdown: Vec<BreakdownRow>,
    /// Crédits supplémentaires (au-delà des limites) activés.
    pub extra_enabled: bool,
}

/// Lit la réponse JSON. Une fenêtre absente (`null`) est omise ; une réponse sans aucune
/// fenêtre exploitable est une erreur. Les champs facultatifs absents restent vides.
pub fn parse_usage(json: &str) -> anyhow::Result<Quota> {
    let v: Value = serde_json::from_str(json)?;
    let resets = |w: &Value| w["resets_at"].as_str().map(str::to_owned);
    let limits: &[Value] = v["limits"].as_array().map(Vec::as_slice).unwrap_or_default();
    // Niveau de la limite générale de la semaine (la fenêtre `seven_day` n'en porte pas).
    let weekly_severity =
        limits.iter().find(|l| l["kind"] == "weekly_all").and_then(|l| l["severity"].as_str()).map(str::to_owned);
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "5 h"), ("seven_day", "Semaine")] {
        let w = &v[key];
        if w.is_null() {
            continue;
        }
        let pct = w["utilization"]
            .as_f64()
            .ok_or_else(|| anyhow::anyhow!("{key}.utilization manquant"))?;
        let severity = if key == "seven_day" { weekly_severity.clone() } else { None };
        windows.push(QuotaWindow {
            label: label.into(),
            kind: key,
            used_pct: pct as f32,
            resets_at: resets(w),
            severity,
            share: None,
        });
    }
    // Limites hebdomadaires propres à un modèle (Fable, Opus…), toutes : `is_active` dit
    // seulement si c'est la limite qui bloque en ce moment, pas si elle est consommée.
    let shares: &[Value] = v["weekly_scoped_shares"].as_array().map(Vec::as_slice).unwrap_or_default();
    for (i, l) in limits.iter().enumerate() {
        let name = l["scope"]["model"]["display_name"].as_str();
        if let (Some("weekly_scoped"), Some(name), Some(pct)) = (l["kind"].as_str(), name, l["percent"].as_f64()) {
            let share = shares.iter().find(|s| s["limit_index"].as_u64() == Some(i as u64)).and_then(|s| {
                Some(Share {
                    used_pct: s["used_percent_of_weekly"].as_f64()? as f32,
                    allowance_pct: s["allowance_percent_of_weekly"].as_f64()? as f32,
                })
            });
            windows.push(QuotaWindow {
                label: name.into(),
                kind: "model",
                used_pct: pct as f32,
                resets_at: resets(l),
                severity: l["severity"].as_str().map(str::to_owned),
                share,
            });
        }
    }
    anyhow::ensure!(!windows.is_empty(), "aucune fenêtre d'usage dans la réponse");
    let bd = &v["seven_day_breakdown"];
    let breakdown = bd["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| Some(BreakdownRow { label: r["display_name"].as_str()?.into(), pct: r["percent"].as_f64()? as f32 }))
        .collect();
    Ok(Quota {
        windows,
        week_started_at: bd["window_started_at"].as_str().map(str::to_owned),
        breakdown,
        extra_enabled: v["extra_usage"]["is_enabled"].as_bool().unwrap_or(false),
    })
}

/// Jeton OAuth lu dans les identifiants locaux de Claude Code (`.credentials.json`). L'erreur
/// est un texte court affichable, sans rien du fichier.
pub fn access_token(credentials: &str) -> Result<String, &'static str> {
    let v: serde_json::Value = serde_json::from_str(credentials).map_err(|_| "identifiants illisibles")?;
    v["claudeAiOauth"]["accessToken"].as_str().filter(|t| !t.is_empty()).map(str::to_owned).ok_or("jeton absent")
}

#[cfg(test)]
mod tests {
    #[test]
    fn token_is_read_from_credentials() {
        assert_eq!(access_token(r#"{"claudeAiOauth":{"accessToken":"abc","expiresAt":1}}"#), Ok("abc".into()));
        assert_eq!(access_token(r#"{"claudeAiOauth":{}}"#), Err("jeton absent"));
        assert_eq!(access_token(r#"{"claudeAiOauth":{"accessToken":""}}"#), Err("jeton absent"));
        assert_eq!(access_token("pas du json"), Err("identifiants illisibles"));
    }

    use super::*;

    /// Extrait d'une vraie réponse (10 novembre 2026), sans donnée personnelle.
    const REAL: &str = r#"{
      "five_hour": null,
      "seven_day": { "utilization": 20.0, "resets_at": "2026-11-16T15:59:59.705734+01:00",
                     "limit_dollars": null, "locked_reason": null },
      "limits": [
        { "kind": "weekly_all", "percent": 20, "resets_at": "2026-11-16T15:59:59+01:00",
          "scope": null, "is_active": true },
        { "kind": "weekly_scoped", "percent": 0, "resets_at": "2026-11-16T16:00:00+01:00",
          "scope": { "model": { "id": null, "display_name": "Fable" } }, "is_active": false },
        { "kind": "weekly_scoped", "percent": 42.5, "resets_at": "2026-11-16T16:00:00+01:00",
          "scope": { "model": { "id": null, "display_name": "Opus" } }, "is_active": true }
      ]
    }"#;

    #[test]
    fn parses_real_response() {
        let q = parse_usage(REAL).unwrap();
        let labels: Vec<_> = q.windows.iter().map(|w| (w.label.as_str(), w.kind, w.used_pct)).collect();
        // Fable compte même quand ce n'est pas la limite qui bloque (`is_active: false`).
        assert_eq!(labels, vec![("Semaine", "seven_day", 20.0), ("Fable", "model", 0.0), ("Opus", "model", 42.5)]);
        assert_eq!(q.windows[0].resets_at.as_deref(), Some("2026-11-16T15:59:59.705734+01:00"));
        assert_eq!(q.week_started_at, None);
        assert!(q.breakdown.is_empty());
        assert!(!q.extra_enabled);
    }

    /// Extrait d'une vraie réponse (12 novembre 2026) : niveaux, parts, répartition, crédits.
    const RICH: &str = r#"{
      "five_hour": null,
      "seven_day": { "utilization": 88.0, "resets_at": "2026-11-16T15:00:00.174954+00:00" },
      "extra_usage": { "is_enabled": false, "user_disabled": true },
      "limits": [
        { "kind": "weekly_all", "group": "weekly", "percent": 88, "severity": "warning",
          "resets_at": "2026-11-16T15:00:00.174954+00:00", "scope": null, "is_active": true },
        { "kind": "weekly_scoped", "group": "weekly", "percent": 65, "severity": "normal",
          "resets_at": "2026-11-16T15:00:00.175089+00:00",
          "scope": { "model": { "id": null, "display_name": "Fable" }, "surface": null }, "is_active": false }
      ],
      "seven_day_breakdown": {
        "as_of": "2026-11-12T00:17:08.184877+00:00",
        "window_started_at": "2026-11-09T15:00:00.174954+00:00",
        "rows": [
          { "key": "claude_code", "display_name": "Claude Code", "percent": 100 },
          { "key": "chat", "display_name": "Chats", "percent": 0 }
        ]
      },
      "weekly_scoped_shares": [
        { "limit_index": 1, "used_percent_of_weekly": 33, "allowance_percent_of_weekly": 50 }
      ]
    }"#;

    #[test]
    fn parses_severity_shares_breakdown_and_window_start() {
        let q = parse_usage(RICH).unwrap();
        assert_eq!(q.windows.len(), 2);
        assert_eq!(q.windows[0].severity.as_deref(), Some("warning"));
        let fable = &q.windows[1];
        assert_eq!((fable.label.as_str(), fable.used_pct), ("Fable", 65.0));
        assert_eq!(fable.severity.as_deref(), Some("normal"));
        assert_eq!(fable.share, Some(Share { used_pct: 33.0, allowance_pct: 50.0 }));
        assert_eq!(q.week_started_at.as_deref(), Some("2026-11-09T15:00:00.174954+00:00"));
        assert_eq!(
            q.breakdown,
            vec![
                BreakdownRow { label: "Claude Code".into(), pct: 100.0 },
                BreakdownRow { label: "Chats".into(), pct: 0.0 },
            ]
        );
        assert!(!q.extra_enabled);
    }

    #[test]
    fn five_hour_window_comes_first() {
        let q = parse_usage(
            r#"{"five_hour":{"utilization":63.4,"resets_at":"2026-11-10T05:00:00Z"},
                "seven_day":{"utilization":21,"resets_at":null}}"#,
        )
        .unwrap();
        assert_eq!(q.windows[0].label, "5 h");
        assert_eq!(q.windows[0].used_pct, 63.4);
        assert_eq!(q.windows[1].resets_at, None);
    }

    #[test]
    fn missing_fields_are_a_clean_error() {
        assert!(parse_usage("{}").is_err());
        assert!(parse_usage(r#"{"five_hour":null,"seven_day":null}"#).is_err());
        assert!(parse_usage("pas du json").is_err());
        assert!(parse_usage(r#"{"seven_day":{"resets_at":"x"}}"#).is_err());
    }
}
