//! Réglages d'un widget de plugin : page générée depuis `settings` de son `plugin.json`. Les
//! valeurs vont dans les réglages de l'exemplaire (comme les autres widgets) ; un secret va dans
//! le Gestionnaire d'identifiants de Windows ([`secret_target`]), jamais dans la disposition.

use super::super::{PluginSettingRow, PluginSettingsData, SettingsWindow};
use horizon_core::plugin::{SettingKind, SettingSpec, coerce, defaults, id_of_kind};
use serde_json::{Value, json};
use slint::{ComponentHandle, ModelRc, VecModel};

/// Nom du secret `key` de l'exemplaire `instance` du plugin `plugin`.
pub fn secret_target(plugin: &str, instance: &str, key: &str) -> String {
    format!("Horizon/plugin/{plugin}/{instance}/{key}")
}

/// Réglages déclarés du plugin du genre `kind`.
fn specs(kind: &str) -> Vec<SettingSpec> {
    let Some(id) = id_of_kind(kind) else { return Vec::new() };
    let Some(app) = super::with(|s| s.app.clone()) else { return Vec::new() };
    use crate::host::Manager;
    app.state::<crate::plugins::Plugins>().manifest(id).map(|m| m.settings.clone()).unwrap_or_default()
}

/// Lignes de la page pour ces réglages et ces valeurs.
pub fn rows(specs: &[SettingSpec], stored: &Value, secret_saved: &dyn Fn(&str) -> bool) -> Vec<PluginSettingRow> {
    let base = defaults(specs);
    specs
        .iter()
        .map(|s| {
            let v = stored.get(&s.key).map(|v| coerce(s, v)).or_else(|| base.get(&s.key).cloned()).unwrap_or(Value::Null);
            let mut row = PluginSettingRow { key: s.key.as_str().into(), label: s.label.as_str().into(), ..Default::default() };
            match &s.kind {
                SettingKind::Bool => {
                    row.kind = 0;
                    row.checked = v.as_bool().unwrap_or(false);
                }
                SettingKind::Number { min, max, step } => {
                    row.kind = 1;
                    row.number = v.as_f64().unwrap_or(0.0).round() as i32;
                    row.minimum = min.map_or(i32::MIN, |m| m.ceil() as i32);
                    row.maximum = max.map_or(i32::MAX, |m| m.floor() as i32);
                    row.step = step.map_or(1, |s| (s.round() as i32).max(1));
                }
                SettingKind::Text => {
                    row.kind = 2;
                    row.text = v.as_str().unwrap_or_default().into();
                }
                SettingKind::Choice(options) => {
                    row.kind = 3;
                    let cur = v.as_str().unwrap_or_default();
                    row.choice = options.iter().find(|(value, _)| value == cur).map_or(cur, |(_, l)| l.as_str()).into();
                }
                SettingKind::Color => {
                    row.kind = 4;
                    row.text = v.as_str().unwrap_or_default().into();
                }
                SettingKind::Secret => {
                    row.kind = 5;
                    row.saved = secret_saved(&s.key);
                }
            }
            row
        })
        .collect()
}

/// Affiche la page de l'exemplaire courant.
pub fn show(window: &SettingsWindow, kind: &str, settings: &Value) {
    let specs = specs(kind);
    let (plugin, instance) = (id_of_kind(kind).unwrap_or_default().to_owned(), super::with(|s| s.id.clone()));
    let saved = |key: &str| horizon_sys::credentials::read(&secret_target(&plugin, &instance, key)).is_some();
    let list = rows(&specs, settings, &saved);
    window.global::<PluginSettingsData>().set_rows(ModelRc::new(VecModel::from(list)));
}

/// Réaffiche la page après une modification.
fn refresh() {
    let Some(w) = super::window() else { return };
    let (kind, settings) = super::with(|s| (s.kind.clone(), s.settings.clone()));
    show(&w, &kind, &settings);
}

/// Enregistre la valeur `v` du réglage `key` (contrôlée par son genre).
fn set(key: &str, v: Value) {
    let kind = super::with(|s| s.kind.clone());
    let Some(spec) = specs(&kind).into_iter().find(|s| s.key == key) else { return };
    let v = coerce(&spec, &v);
    let mut patch = serde_json::Map::new();
    patch.insert(key.to_owned(), v);
    super::save(Value::Object(patch));
    refresh();
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<PluginSettingsData>();
    g.on_set_bool(|k, v| set(&k, json!(v)));
    g.on_set_number(|k, v| set(&k, json!(v)));
    g.on_set_text(|k, v| set(&k, json!(v.as_str())));
    g.on_set_secret(|k, v| {
        let (kind, instance, app) = super::with(|s| (s.kind.clone(), s.id.clone(), s.app.clone()));
        let Some(plugin) = id_of_kind(&kind) else { return };
        let target = secret_target(plugin, &instance, &k);
        let result = if v.is_empty() {
            horizon_sys::credentials::delete(&target);
            Ok(())
        } else {
            horizon_sys::credentials::write(&target, &v)
        };
        if let Err(e) = result {
            super::set_error(&format!("secret non enregistré : {e:#}"));
        }
        // Le script reçoit le nouveau secret (rien n'a changé dans la disposition).
        if let Some(app) = app {
            crate::plugins::script::reconcile(&app);
        }
        refresh();
    });
    g.on_open_choice(|k, x, y| {
        let kind = super::with(|s| s.kind.clone());
        let Some(SettingKind::Choice(options)) = specs(&kind).into_iter().find(|s| s.key == k.as_str()).map(|s| s.kind) else { return };
        let entries = options.iter().map(|(value, label)| crate::ui::menu::Entry::item(value.clone(), label.clone())).collect();
        let key = k.to_string();
        super::menu_at(x, y, entries, move |chosen| set(&key, json!(chosen)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_rows_from_specs() {
        let specs = vec![
            SettingSpec { key: "on".into(), label: "Actif".into(), kind: SettingKind::Bool, default: json!(true) },
            SettingSpec { key: "pas".into(), label: "Pas".into(), kind: SettingKind::Number { min: Some(1.0), max: Some(50.0), step: None }, default: json!(10) },
            SettingSpec {
                key: "mode".into(),
                label: "Mode".into(),
                kind: SettingKind::Choice(vec![("a".into(), "Auto".into()), ("m".into(), "Manuel".into())]),
                default: json!("a"),
            },
            SettingSpec { key: "jeton".into(), label: "Jeton".into(), kind: SettingKind::Secret, default: Value::Null },
        ];
        let rows = rows(&specs, &json!({ "pas": 12, "mode": "m" }), &|k| k == "jeton");
        assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(), [0, 1, 3, 5]);
        assert!(rows[0].checked);
        assert_eq!((rows[1].number, rows[1].minimum, rows[1].maximum, rows[1].step), (12, 1, 50, 1));
        assert_eq!(rows[2].choice, "Manuel");
        assert!(rows[3].saved);
    }
}
