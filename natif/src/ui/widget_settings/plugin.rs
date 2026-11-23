//! Réglages d'un widget de plugin : page générée depuis `settings` de son `plugin.json`. Les
//! valeurs vont dans les réglages de l'exemplaire (comme les autres widgets) ; un secret va dans
//! le Gestionnaire d'identifiants de Windows ([`secret_target`]), jamais dans la disposition.
//!
//! Une liste (`type: "list"`) donne une suite de lignes : son titre, puis pour chaque élément
//! un en-tête et ses champs (clé « liste/rang/champ »), puis « Ajouter ».

use super::super::{PluginSettingRow, PluginSettingsData, SettingsWindow};
use horizon_core::plugin::{LIST_MAX, SettingKind, SettingSpec, coerce, defaults, id_of_kind, list_blank};
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

/// Clé d'une ligne : réglage simple, élément d'une liste (`liste/rang`) ou champ d'un élément
/// (`liste/rang/champ`). Les clés déclarées n'ont ni « / » ni espace.
fn split_key(key: &str) -> (&str, Option<usize>, Option<&str>) {
    let mut parts = key.splitn(3, '/');
    let name = parts.next().unwrap_or_default();
    (name, parts.next().and_then(|i| i.parse().ok()), parts.next())
}

/// Réglage visé par la clé d'une ligne : le réglage lui-même, ou le champ d'une liste.
fn spec_of<'a>(specs: &'a [SettingSpec], key: &str) -> Option<&'a SettingSpec> {
    let (name, _, field) = split_key(key);
    let spec = specs.iter().find(|s| s.key == name)?;
    match (field, &spec.kind) {
        (Some(f), SettingKind::List(fields)) => fields.iter().find(|x| x.key == f),
        (None, _) => Some(spec),
        _ => None,
    }
}

/// Valeur enregistrée du réglage `spec` (nettoyée), ou son défaut.
fn value_of(spec: &SettingSpec, stored: &Value) -> Value {
    stored.get(&spec.key).map_or_else(|| spec.default.clone(), |v| coerce(spec, v))
}

/// Changement d'une liste.
#[derive(Debug, Clone, PartialEq)]
pub enum ListEdit<'a> {
    Add,
    Remove(usize),
    Field(usize, &'a str, Value),
}

/// La liste `spec` après `edit` ; `None` si le changement ne s'applique pas (rang ou champ
/// inconnu, liste pleine).
pub fn edit_list(spec: &SettingSpec, stored: &Value, edit: ListEdit) -> Option<Value> {
    let SettingKind::List(fields) = &spec.kind else { return None };
    let Value::Array(mut items) = value_of(spec, stored) else { return None };
    match edit {
        ListEdit::Add if items.len() < LIST_MAX => items.push(list_blank(fields)),
        ListEdit::Add => return None,
        ListEdit::Remove(i) if i < items.len() => {
            items.remove(i);
        }
        ListEdit::Remove(_) => return None,
        ListEdit::Field(i, field, v) => {
            let f = fields.iter().find(|f| f.key == field)?;
            items.get_mut(i)?.as_object_mut()?.insert(field.to_owned(), coerce(f, &v));
        }
    }
    Some(Value::Array(items))
}

/// Ligne d'un réglage simple (ou d'un champ de liste) à la valeur `v`.
fn row(s: &SettingSpec, key: String, v: &Value, nested: bool, saved: bool) -> PluginSettingRow {
    let help = s.help.as_deref().unwrap_or_default();
    let mut row = PluginSettingRow { key: key.into(), label: s.label.as_str().into(), hint: help.into(), nested, ..Default::default() };
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
            if help.is_empty() {
                row.hint = "Couleur #rrggbb".into();
            }
        }
        SettingKind::Secret => {
            row.kind = 5;
            row.saved = saved;
            if help.is_empty() {
                row.hint = if saved {
                    "Enregistré dans le Gestionnaire d’identifiants de Windows. Saisir pour remplacer.".into()
                } else {
                    "Gardé dans le Gestionnaire d’identifiants de Windows.".into()
                };
            }
        }
        SettingKind::List(_) => row.kind = 6,
    }
    row
}

/// Nom affiché d'un élément de liste : son premier champ de texte rempli, sinon son rang.
fn item_title(fields: &[SettingSpec], item: &Value, i: usize) -> String {
    fields
        .iter()
        .filter(|f| f.kind == SettingKind::Text)
        .filter_map(|f| item.get(&f.key).and_then(Value::as_str))
        .map(str::trim)
        .find(|t| !t.is_empty())
        .map_or_else(|| format!("Élément {}", i + 1), str::to_owned)
}

/// Lignes de la page pour ces réglages et ces valeurs.
pub fn rows(specs: &[SettingSpec], stored: &Value, secret_saved: &dyn Fn(&str) -> bool) -> Vec<PluginSettingRow> {
    let base = defaults(specs);
    let mut out = Vec::new();
    for s in specs {
        let v = stored.get(&s.key).map(|v| coerce(s, v)).or_else(|| base.get(&s.key).cloned()).unwrap_or(Value::Null);
        out.push(row(s, s.key.clone(), &v, false, secret_saved(&s.key)));
        let SettingKind::List(fields) = &s.kind else { continue };
        let items = v.as_array().map(Vec::as_slice).unwrap_or_default();
        for (i, item) in items.iter().enumerate() {
            let key = format!("{}/{i}", s.key);
            out.push(PluginSettingRow { key: key.as_str().into(), label: item_title(fields, item, i).into(), kind: 7, ..Default::default() });
            out.extend(fields.iter().map(|f| row(f, format!("{key}/{}", f.key), item.get(&f.key).unwrap_or(&f.default), true, false)));
        }
        if items.len() < LIST_MAX {
            out.push(PluginSettingRow { key: s.key.as_str().into(), label: "Ajouter".into(), kind: 8, ..Default::default() });
        }
    }
    out
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

/// Enregistre la valeur `v` de la ligne `key` (contrôlée par son genre) : un réglage simple, ou
/// le champ d'un élément de liste.
fn set(key: &str, v: Value) {
    let (kind, stored) = super::with(|s| (s.kind.clone(), s.settings.clone()));
    let specs = specs(&kind);
    let (name, index, field) = split_key(key);
    let Some(spec) = specs.iter().find(|s| s.key == name) else { return };
    let value = match (index, field) {
        (Some(i), Some(f)) => edit_list(spec, &stored, ListEdit::Field(i, f, v)),
        (None, None) if !matches!(spec.kind, SettingKind::List(_)) => Some(coerce(spec, &v)),
        _ => None,
    };
    if let Some(value) = value {
        save_value(name, value);
    }
}

/// Ajoute ou retire un élément de la liste d'une ligne (`liste`, `liste/rang`).
fn edit(key: &str, edit: ListEdit) {
    let (kind, stored) = super::with(|s| (s.kind.clone(), s.settings.clone()));
    let name = split_key(key).0;
    let value = specs(&kind).iter().find(|s| s.key == name).and_then(|spec| edit_list(spec, &stored, edit));
    if let Some(value) = value {
        save_value(name, value);
    }
}

fn save_value(name: &str, value: Value) {
    let mut patch = serde_json::Map::new();
    patch.insert(name.to_owned(), value);
    super::save(Value::Object(patch));
    refresh();
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<PluginSettingsData>();
    g.on_set_bool(|k, v| set(&k, json!(v)));
    g.on_set_number(|k, v| set(&k, json!(v)));
    g.on_set_text(|k, v| set(&k, json!(v.as_str())));
    g.on_list_add(|k| edit(&k, ListEdit::Add));
    g.on_list_remove(|k| {
        if let Some(i) = split_key(&k).1 {
            edit(&k, ListEdit::Remove(i));
        }
    });
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
        let Some(SettingKind::Choice(options)) = spec_of(&specs(&kind), &k).map(|s| s.kind.clone()) else { return };
        let entries = options.iter().map(|(value, label)| crate::ui::menu::Entry::item(value.clone(), label.clone())).collect();
        let key = k.to_string();
        super::menu_at(x, y, entries, move |chosen| set(&key, json!(chosen)));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str, label: &str, kind: SettingKind, default: Value) -> SettingSpec {
        SettingSpec { key: key.into(), label: label.into(), help: None, kind, default }
    }

    #[test]
    fn generated_rows_from_specs() {
        let specs = vec![
            spec("on", "Actif", SettingKind::Bool, json!(true)),
            SettingSpec { help: Some("De 1 à 50.".into()), ..spec("pas", "Pas", SettingKind::Number { min: Some(1.0), max: Some(50.0), step: None }, json!(10)) },
            spec("mode", "Mode", SettingKind::Choice(vec![("a".into(), "Auto".into()), ("m".into(), "Manuel".into())]), json!("a")),
            spec("jeton", "Jeton", SettingKind::Secret, Value::Null),
        ];
        let rows = rows(&specs, &json!({ "pas": 12, "mode": "m" }), &|k| k == "jeton");
        assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(), [0, 1, 3, 5]);
        assert!(rows[0].checked);
        assert_eq!((rows[1].number, rows[1].minimum, rows[1].maximum, rows[1].step), (12, 1, 50, 1));
        assert_eq!(rows[1].hint, "De 1 à 50.");
        assert_eq!(rows[2].choice, "Manuel");
        assert!(rows[3].saved && rows[3].hint.contains("Saisir pour remplacer"));
    }

    fn hosts() -> SettingSpec {
        let fields = vec![
            spec("nom", "Nom", SettingKind::Text, json!("")),
            spec("test", "Test", SettingKind::Choice(vec![("tcp".into(), "Port TCP".into()), ("http".into(), "HTTP(S)".into())]), json!("tcp")),
            spec("port", "Port", SettingKind::Number { min: Some(0.0), max: Some(65535.0), step: None }, json!(0)),
        ];
        spec("hotes", "Serveurs", SettingKind::List(fields), json!([]))
    }

    #[test]
    fn list_gives_a_heading_then_its_items_then_add() {
        let specs = [hosts()];
        let stored = json!({ "hotes": [{ "nom": "NAS", "test": "http", "port": 5000 }, { "nom": " " }] });
        let rows = rows(&specs, &stored, &|_| false);
        assert_eq!(rows.iter().map(|r| r.kind).collect::<Vec<_>>(), [6, 7, 2, 3, 1, 7, 2, 3, 1, 8]);
        assert_eq!(rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>()[..5], ["hotes", "hotes/0", "hotes/0/nom", "hotes/0/test", "hotes/0/port"]);
        assert_eq!((rows[1].label.as_str(), rows[5].label.as_str()), ("NAS", "Élément 2"));
        assert_eq!((rows[3].choice.as_str(), rows[4].number, rows[7].choice.as_str()), ("HTTP(S)", 5000, "Port TCP"));
        assert!(rows[2].nested && !rows[1].nested);
        assert_eq!((rows[9].key.as_str(), rows[9].label.as_str()), ("hotes", "Ajouter"));
        // Liste vide : son titre et « Ajouter ».
        assert_eq!(super::rows(&specs, &Value::Null, &|_| false).iter().map(|r| r.kind).collect::<Vec<_>>(), [6, 8]);
    }

    #[test]
    fn list_edits() {
        let s = hosts();
        let one = edit_list(&s, &Value::Null, ListEdit::Add).unwrap();
        assert_eq!(one, json!([{ "nom": "", "test": "tcp", "port": 0 }]));
        let stored = json!({ "hotes": one });
        let named = edit_list(&s, &stored, ListEdit::Field(0, "nom", json!("NAS"))).unwrap();
        assert_eq!(named[0]["nom"], "NAS");
        // Valeur qui ne convient pas au champ : son défaut.
        assert_eq!(edit_list(&s, &stored, ListEdit::Field(0, "port", json!(70000))).unwrap()[0]["port"], 0);
        assert_eq!(edit_list(&s, &stored, ListEdit::Field(1, "nom", json!("x"))), None);
        assert_eq!(edit_list(&s, &stored, ListEdit::Field(0, "inconnu", json!("x"))), None);
        assert_eq!(edit_list(&s, &stored, ListEdit::Remove(0)), Some(json!([])));
        assert_eq!(edit_list(&s, &stored, ListEdit::Remove(3)), None);
        let full = json!({ "hotes": vec![json!({}); LIST_MAX] });
        assert_eq!(edit_list(&s, &full, ListEdit::Add), None);
        assert_eq!(edit_list(&spec("on", "Actif", SettingKind::Bool, json!(true)), &Value::Null, ListEdit::Add), None);
    }

    #[test]
    fn keys_of_list_rows() {
        assert_eq!(split_key("pas"), ("pas", None, None));
        assert_eq!(split_key("hotes/3"), ("hotes", Some(3), None));
        assert_eq!(split_key("hotes/3/nom"), ("hotes", Some(3), Some("nom")));
        let specs = [hosts(), spec("on", "Actif", SettingKind::Bool, json!(true))];
        assert_eq!(spec_of(&specs, "hotes/0/test").map(|s| s.key.as_str()), Some("test"));
        assert_eq!(spec_of(&specs, "on").map(|s| s.key.as_str()), Some("on"));
        assert!(spec_of(&specs, "on/0/x").is_none() && spec_of(&specs, "hotes/0/inconnu").is_none());
    }
}
