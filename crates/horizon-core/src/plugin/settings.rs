//! Réglages déclarés par un plugin (`settings` de `plugin.json`) : la fenêtre de réglages est
//! générée à partir de cette liste, les valeurs vivent dans `WidgetInstance.settings`.

use serde_json::{Map, Value};

/// Éléments d'un réglage `list`, au plus.
pub const LIST_MAX: usize = 64;

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
    /// Liste d'objets (serveurs, villes…) : les champs de chaque élément (ni secret, ni liste).
    List(Vec<SettingSpec>),
}

/// Un réglage déclaré.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub key: String,
    pub label: String,
    /// Ligne d'aide sous le libellé.
    pub help: Option<String>,
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

/// Valeur gardée pour `v` : celle-ci si elle convient au réglage, sinon le défaut. Une liste est
/// nettoyée élément par élément (champ absent ou faux : son défaut ; ce qui n'est pas un objet et
/// ce qui dépasse [`LIST_MAX`] est écarté).
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
        SettingKind::List(fields) => {
            let Some(items) = v.as_array() else { return spec.default.clone() };
            return items.iter().filter_map(Value::as_object).take(LIST_MAX).map(|item| list_item(fields, item)).collect();
        }
    };
    if fits { v.clone() } else { spec.default.clone() }
}

/// Élément d'une liste : chaque champ déclaré, à sa valeur ou à son défaut.
fn list_item(fields: &[SettingSpec], item: &Map<String, Value>) -> Value {
    Value::Object(fields.iter().map(|f| (f.key.clone(), item.get(&f.key).map_or_else(|| f.default.clone(), |v| coerce(f, v)))).collect())
}

/// Nouvel élément d'une liste : les défauts de ses champs.
pub fn list_blank(fields: &[SettingSpec]) -> Value {
    list_item(fields, &Map::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(key: &str, kind: SettingKind, default: Value) -> SettingSpec {
        SettingSpec { key: key.into(), label: key.to_uppercase(), help: None, kind, default }
    }

    fn number() -> SettingSpec {
        spec("pas", SettingKind::Number { min: Some(1.0), max: Some(50.0), step: None }, json!(10))
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
        let choice = spec("c", SettingKind::Choice(vec![("a".into(), "A".into()), ("b".into(), "B".into())]), json!("a"));
        assert_eq!(coerce(&choice, &json!("b")), json!("b"));
        assert_eq!(coerce(&choice, &json!("z")), json!("a"));
        let color = spec("k", SettingKind::Color, json!("#ffffff"));
        assert_eq!(coerce(&color, &json!("#12ab3C")), json!("#12ab3C"));
        assert_eq!(coerce(&color, &json!("rouge")), json!("#ffffff"));
        let secret = spec("t", SettingKind::Secret, Value::Null);
        assert_eq!(coerce(&secret, &json!("jeton")), Value::Null);
    }

    #[test]
    fn defaults_skip_secrets() {
        let secret = spec("t", SettingKind::Secret, Value::Null);
        let d = defaults(&[number(), secret]);
        assert_eq!(Value::Object(d), json!({ "pas": 10 }));
    }

    fn hosts() -> SettingSpec {
        let fields = vec![
            spec("nom", SettingKind::Text, json!("")),
            spec("port", SettingKind::Number { min: Some(0.0), max: Some(65535.0), step: None }, json!(0)),
            spec("cache", SettingKind::Bool, json!(false)),
        ];
        spec("hotes", SettingKind::List(fields), json!([]))
    }

    #[test]
    fn list_is_cleaned_item_by_item() {
        let s = hosts();
        let v = json!([
            { "nom": "nas", "port": 5000, "cache": true, "inconnu": 1 },
            { "nom": 12, "port": 70000 },
            "pas un objet",
            {}
        ]);
        assert_eq!(
            coerce(&s, &v),
            json!([
                { "nom": "nas", "port": 5000, "cache": true },
                { "nom": "", "port": 0, "cache": false },
                { "nom": "", "port": 0, "cache": false }
            ])
        );
        assert_eq!(coerce(&s, &json!("x")), json!([]));
        assert_eq!(coerce(&s, &json!([])), json!([]));
        let SettingKind::List(fields) = &s.kind else { unreachable!() };
        assert_eq!(list_blank(fields), json!({ "nom": "", "port": 0, "cache": false }));
    }

    #[test]
    fn list_is_bounded() {
        let many: Vec<Value> = (0..LIST_MAX + 5).map(|i| json!({ "nom": i.to_string() })).collect();
        assert_eq!(coerce(&hosts(), &Value::Array(many)).as_array().unwrap().len(), LIST_MAX);
        assert_eq!(defaults(&[hosts()]).get("hotes"), Some(&json!([])));
    }
}
