//! `plugin.json` : lecture et validation du manifeste d'un plugin.

use super::permissions::{self, Permissions};
use super::settings::{SettingKind, SettingSpec};
use super::{API_MAX, API_MIN, EVENTS};
use serde::Deserialize;
use serde_json::Value;

/// Place du widget dans la barre (`widget` de `plugin.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct WidgetSpec {
    /// Barre trop chargée : rang de masquage (1 = masqué le premier).
    pub squeeze: Option<u64>,
    /// Grandit avec son contenu : sa plus petite largeur (px logiques).
    pub grows: Option<u16>,
    /// Reste cliquable sur une barre « Survol seul ».
    pub clickable_on_hover: bool,
}

/// Manifeste validé.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api: u32,
    pub author: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    /// Icône Phosphor (nom) ; `None` : `icon.svg` du dossier s'il existe.
    pub icon: Option<String>,
    pub widget: Option<WidgetSpec>,
    pub panel: bool,
    pub permissions: Permissions,
    pub settings: Vec<SettingSpec>,
}

/// Erreur de manifeste : le champ en cause et le message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "plugin.json, {} : {}", self.field, self.message)
    }
}

impl std::error::Error for ManifestError {}

/// Lit et valide `plugin.json`. `builtin` : identifiants des widgets intégrés (interdits).
pub fn parse_manifest(json: &str, builtin: &[&str]) -> Result<PluginManifest, ManifestError> {
    let raw: Raw = serde_json::from_str(json).map_err(|e| err("plugin.json", e.to_string()))?;
    let id_ok = (1..=40).contains(&raw.id.len())
        && raw.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !id_ok {
        return Err(err("id", "1 à 40 caractères parmi a-z, 0-9, « _ » et « - »"));
    }
    if builtin.contains(&raw.id.as_str()) {
        return Err(err("id", format!("« {} » est déjà un widget d'Horizon", raw.id)));
    }
    if raw.name.trim().is_empty() {
        return Err(err("name", "nom vide"));
    }
    if raw.api > API_MAX {
        return Err(err("api", format!("API {} : demande une version plus récente d'Horizon (celle-ci va jusqu'à {API_MAX})", raw.api)));
    }
    if raw.api < API_MIN {
        return Err(err("api", format!("API {} trop ancienne (Horizon demande au moins {API_MIN})", raw.api)));
    }
    let p = &raw.permissions;
    p.connect.iter().try_for_each(|e| permissions::valid_connect(e)).map_err(|m| err("permissions.connect", m))?;
    p.http.iter().try_for_each(|e| permissions::valid_http(e)).map_err(|m| err("permissions.http", m))?;
    p.files.iter().try_for_each(|e| permissions::valid_file(e)).map_err(|m| err("permissions.files", m))?;
    if let Some(e) = p.events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
        return Err(err("permissions.events", format!("« {e} » inconnu (connus : {})", EVENTS.join(", "))));
    }
    let mut settings: Vec<SettingSpec> = Vec::with_capacity(raw.settings.len());
    for (i, r) in raw.settings.into_iter().enumerate() {
        let spec = setting(r).map_err(|m| err(&format!("settings[{i}]"), m))?;
        if settings.iter().any(|s| s.key == spec.key) {
            return Err(err(&format!("settings[{i}]"), format!("clé « {} » en double", spec.key)));
        }
        settings.push(spec);
    }
    Ok(PluginManifest {
        id: raw.id,
        name: raw.name,
        version: raw.version,
        api: raw.api,
        author: raw.author,
        description: raw.description,
        homepage: raw.homepage,
        icon: raw.icon,
        widget: raw.widget,
        panel: raw.panel,
        permissions: raw.permissions,
        settings,
    })
}

fn err(field: &str, message: impl Into<String>) -> ManifestError {
    ManifestError { field: field.to_owned(), message: message.into() }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Raw {
    id: String,
    name: String,
    version: String,
    api: u32,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    homepage: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    widget: Option<WidgetSpec>,
    #[serde(default)]
    panel: bool,
    #[serde(default)]
    permissions: Permissions,
    #[serde(default)]
    settings: Vec<RawSetting>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSetting {
    key: String,
    #[serde(rename = "type")]
    kind: String,
    label: String,
    #[serde(default)]
    default: Option<Value>,
    #[serde(default)]
    min: Option<f64>,
    #[serde(default)]
    max: Option<f64>,
    #[serde(default)]
    step: Option<f64>,
    #[serde(default)]
    options: Vec<RawOption>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawOption {
    Plain(String),
    Pair { value: String, label: String },
}

fn setting(r: RawSetting) -> Result<SettingSpec, String> {
    if r.key.is_empty() || !r.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("clé « {} » : lettres, chiffres et « _ » seulement", r.key));
    }
    let kind = match r.kind.as_str() {
        "bool" => SettingKind::Bool,
        "number" => SettingKind::Number { min: r.min, max: r.max, step: r.step },
        "string" => SettingKind::Text,
        "color" => SettingKind::Color,
        "secret" => SettingKind::Secret,
        "choice" => {
            let options: Vec<(String, String)> = r
                .options
                .into_iter()
                .map(|o| match o {
                    RawOption::Plain(v) => (v.clone(), v),
                    RawOption::Pair { value, label } => (value, label),
                })
                .collect();
            if options.is_empty() {
                return Err("« options » vide".into());
            }
            SettingKind::Choice(options)
        }
        other => return Err(format!("type « {other} » inconnu (bool, number, string, choice, color, secret)")),
    };
    let fallback = match &kind {
        SettingKind::Bool => Value::Bool(false),
        SettingKind::Number { min, .. } => serde_json::json!(min.unwrap_or(0.0)),
        SettingKind::Text => Value::String(String::new()),
        SettingKind::Choice(options) => Value::String(options[0].0.clone()),
        SettingKind::Color => Value::String("#ffffff".into()),
        SettingKind::Secret => Value::Null,
    };
    let mut spec = SettingSpec { key: r.key, label: r.label, kind, default: fallback };
    if let Some(d) = r.default {
        if spec.kind != SettingKind::Secret && super::settings::coerce(&spec, &d) != d {
            return Err(format!("« default » ne convient pas au type « {} »", r.kind));
        }
        if spec.kind != SettingKind::Secret {
            spec.default = d;
        }
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"{
      "id": "prisme",
      "name": "Éclairage (Prisme)",
      "version": "1.0.0",
      "api": 0,
      "author": "KingsRequin",
      "description": "Scène et luminosité de Prisme, signaux d'Horizon vers les LED.",
      "homepage": "https://github.com/KingsRequin/rgb-engine",
      "icon": "lightbulb",
      "widget": { "squeeze": 4, "grows": null, "clickableOnHover": false },
      "panel": true,
      "permissions": {
        "connect": ["ws://127.0.0.1:28650"],
        "files": [],
        "http": [],
        "events": ["accent", "dnd", "alert", "timer"]
      },
      "settings": [
        { "key": "brightnessStep", "type": "number", "label": "Pas de luminosité", "default": 10, "min": 1, "max": 50 }
      ]
    }"#;

    fn with(field: &str, value: &str) -> String {
        let mut v: Value = serde_json::from_str(EXAMPLE).unwrap();
        v[field] = serde_json::from_str(value).unwrap();
        v.to_string()
    }

    fn err_field(json: &str) -> String {
        parse_manifest(json, &["clock"]).unwrap_err().field
    }

    #[test]
    fn parses_the_spec_example() {
        let m = parse_manifest(EXAMPLE, &["clock"]).unwrap();
        assert_eq!(m.id, "prisme");
        assert_eq!(m.widget, Some(WidgetSpec { squeeze: Some(4), grows: None, clickable_on_hover: false }));
        assert!(m.panel);
        assert_eq!(m.permissions.events, ["accent", "dnd", "alert", "timer"]);
        assert_eq!(m.settings.len(), 1);
        assert_eq!(m.settings[0].kind, SettingKind::Number { min: Some(1.0), max: Some(50.0), step: None });
        assert_eq!(m.settings[0].default, serde_json::json!(10));
    }

    #[test]
    fn rejects_bad_id() {
        for id in [r#""Prisme""#, r#""a b""#, r#""""#, r#""clock""#] {
            assert_eq!(err_field(&with("id", id)), "id", "{id}");
        }
        let long = format!("\"{}\"", "a".repeat(41));
        assert_eq!(err_field(&with("id", &long)), "id");
    }

    #[test]
    fn rejects_newer_api() {
        let e = parse_manifest(&with("api", "1"), &[]).unwrap_err();
        assert_eq!(e.field, "api");
        assert!(e.message.contains("plus récente"), "{}", e.message);
    }

    #[test]
    fn rejects_remote_connect() {
        let json = with("permissions", r#"{ "connect": ["ws://192.168.1.2:1"] }"#);
        assert_eq!(err_field(&json), "permissions.connect");
    }

    #[test]
    fn rejects_unknown_event() {
        let json = with("permissions", r#"{ "events": ["clavier"] }"#);
        assert_eq!(err_field(&json), "permissions.events");
    }

    #[test]
    fn rejects_unknown_field_and_bad_setting() {
        assert_eq!(err_field(&with("couleur", "1")), "plugin.json");
        let json = with("settings", r#"[{ "key": "a", "type": "slider", "label": "A" }]"#);
        assert_eq!(err_field(&json), "settings[0]");
        let json = with("settings", r#"[{ "key": "a", "type": "choice", "label": "A", "options": [] }]"#);
        assert_eq!(err_field(&json), "settings[0]");
    }

    #[test]
    fn choice_options_accept_strings_and_pairs() {
        let json = with(
            "settings",
            r#"[{ "key": "c", "type": "choice", "label": "C", "options": ["a", { "value": "b", "label": "Bé" }] }]"#,
        );
        let m = parse_manifest(&json, &[]).unwrap();
        assert_eq!(m.settings[0].kind, SettingKind::Choice(vec![("a".into(), "a".into()), ("b".into(), "Bé".into())]));
        assert_eq!(m.settings[0].default, serde_json::json!("a"));
    }

    #[test]
    fn widget_is_optional() {
        let mut v: Value = serde_json::from_str(EXAMPLE).unwrap();
        v.as_object_mut().unwrap().remove("widget");
        let m = parse_manifest(&v.to_string(), &[]).unwrap();
        assert_eq!(m.widget, None);
    }
}
