//! Réglages déclarés par un plugin (`settings` de `plugin.json`) : la fenêtre de réglages est
//! générée à partir de cette liste, les valeurs vivent dans `WidgetInstance.settings`.

use serde_json::{Map, Value};

/// Genre d'un réglage.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingKind {
    Bool,
    Number { min: Option<f64>, max: Option<f64>, step: Option<f64> },
    Text,
    /// Choix dans une liste : (valeur, nom affiché).
    Choice(Vec<(String, String)>),
    /// Couleur `#rrggbb`.
    Color,
    /// Texte secret, gardé dans le Gestionnaire d'identifiants de Windows (jamais dans le layout).
    Secret,
}

/// Un réglage déclaré.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    pub kind: SettingKind,
    pub default: Value,
}

/// Valeurs par défaut de tous les réglages (sauf les secrets, absents du layout).
pub fn defaults(specs: &[SettingSpec]) -> Map<String, Value> {
    specs.iter().filter(|s| s.kind != SettingKind::Secret).map(|s| (s.key.clone(), s.default.clone())).collect()
}

/// Couleur `#rrggbb`.
pub(super) fn is_color(v: &str) -> bool {
    v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// Valeur gardée pour `v` : celle-ci si elle convient au réglage, sinon le défaut.
pub fn coerce(spec: &SettingSpec, v: &Value) -> Value {
    let fits = match &spec.kind {
        SettingKind::Bool => v.is_boolean(),
        SettingKind::Number { min, max, .. } => {
            v.as_f64().is_some_and(|n| min.is_none_or(|m| n >= m) && max.is_none_or(|m| n <= m))
        }
        SettingKind::Text => v.is_string(),
        SettingKind::Choice(options) => v.as_str().is_some_and(|s| options.iter().any(|(value, _)| value == s)),
        SettingKind::Color => v.as_str().is_some_and(is_color),
        SettingKind::Secret => return Value::Null,
    };
    if fits { v.clone() } else { spec.default.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn number() -> SettingSpec {
        SettingSpec {
            key: "pas".into(),
            label: "Pas".into(),
            kind: SettingKind::Number { min: Some(1.0), max: Some(50.0), step: None },
            default: json!(10),
        }
    }

    #[test]
    fn coerce_number_bounds() {
        let s = number();
        assert_eq!(coerce(&s, &json!(0)), json!(10));
        assert_eq!(coerce(&s, &json!(12)), json!(12));
        assert_eq!(coerce(&s, &json!(51)), json!(10));
        assert_eq!(coerce(&s, &json!("x")), json!(10));
    }

    #[test]
    fn coerce_other_kinds() {
        let choice = SettingSpec {
            key: "c".into(),
            label: "C".into(),
            kind: SettingKind::Choice(vec![("a".into(), "A".into()), ("b".into(), "B".into())]),
            default: json!("a"),
        };
        assert_eq!(coerce(&choice, &json!("b")), json!("b"));
        assert_eq!(coerce(&choice, &json!("z")), json!("a"));
        let color = SettingSpec { key: "k".into(), label: "K".into(), kind: SettingKind::Color, default: json!("#ffffff") };
        assert_eq!(coerce(&color, &json!("#12ab3C")), json!("#12ab3C"));
        assert_eq!(coerce(&color, &json!("rouge")), json!("#ffffff"));
        let secret = SettingSpec { key: "t".into(), label: "T".into(), kind: SettingKind::Secret, default: Value::Null };
        assert_eq!(coerce(&secret, &json!("jeton")), Value::Null);
    }

    #[test]
    fn defaults_skip_secrets() {
        let secret = SettingSpec { key: "t".into(), label: "T".into(), kind: SettingKind::Secret, default: Value::Null };
        let d = defaults(&[number(), secret]);
        assert_eq!(Value::Object(d), json!({ "pas": 10 }));
    }
}
