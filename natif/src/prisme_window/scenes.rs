//! Page « Effets » (`pages/Scenes.svelte` et `lib/ParamControl.svelte`) : une scène = un effet et
//! ses réglages, sous un nom réutilisé par les règles. Création, suppression (sauf scène utilisée),
//! changement d'effet (réglages par défaut), réglages selon leur type, essai sur les LED (scène
//! forcée). Global `PrismeScenes`.

use super::color;
use super::effects::{self, Kind, default_spec, effect, order_scenes, scene_label, valid_scene_name};
use super::{Ctx, act, call, edited, list, pick_color, select, sync_rows, with};
use crate::ui::{PrismeOption, PrismeParam, PrismeSceneRow, PrismeScenes, PrismeWindow};
use serde_json::{Map, Value, json};
use slint::{ComponentHandle, Model, VecModel};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct State {
    selected: Option<String>,
    new_name: String,
    /// Réglages montrés (lignes comparées : une ligne inchangée garde son élément).
    shown: Vec<Plain>,
    params: Option<Rc<VecModel<PrismeParam>>>,
}

/// Un réglage tel qu'il est montré.
#[derive(Debug, Clone, PartialEq)]
struct Plain {
    key: &'static str,
    label: &'static str,
    kind: &'static str,
    value: f64,
    minimum: f64,
    maximum: f64,
    step: f64,
    text: String,
    checked: bool,
    color: [u8; 3],
    options: Vec<(&'static str, &'static str, bool)>,
    palette: Vec<[u8; 3]>,
    hint: &'static str,
}

impl Plain {
    fn row(&self) -> PrismeParam {
        PrismeParam {
            key: self.key.into(),
            label: self.label.into(),
            kind: self.kind.into(),
            value: self.value as f32,
            minimum: self.minimum as f32,
            maximum: self.maximum as f32,
            step: self.step as f32,
            text: self.text.as_str().into(),
            checked: self.checked,
            color: rgb(self.color),
            options: list(self.options.iter().map(|(k, l, on)| PrismeOption { key: (*k).into(), label: (*l).into(), on: *on }).collect()),
            palette: list(self.palette.iter().map(|c| rgb(*c)).collect()),
            hint: self.hint.into(),
        }
    }
}

fn rgb(c: [u8; 3]) -> slint::Color {
    slint::Color::from_rgb_u8(c[0], c[1], c[2])
}

/// Nombre JSON : entier quand la valeur l'est (le moteur attend parfois un entier : fps,
/// mesures…), comme le ferait le navigateur.
pub(super) fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 { json!(v as i64) } else { json!(v) }
}

fn scenes(cfg: &Value) -> Option<&Map<String, Value>> {
    cfg.get("scenes")?.as_object()
}

/// Scène choisie (la première si aucune ou si elle a disparu).
fn selected(c: &mut Ctx) -> Option<String> {
    let names = order_scenes(&scenes(c.config.draft()?)?.keys().cloned().collect::<Vec<_>>());
    let ok = c.scenes.selected.as_ref().is_some_and(|s| names.contains(s));
    if !ok {
        c.scenes.selected = names.first().cloned();
    }
    c.scenes.selected.clone()
}

pub(super) fn has_selection(c: &Ctx) -> bool {
    let Some(cfg) = c.config.draft() else { return false };
    let Some(s) = scenes(cfg) else { return false };
    match &c.scenes.selected {
        Some(sel) => s.contains_key(sel),
        None => !s.is_empty(),
    }
}

/// Réglages de la scène choisie, à modifier.
fn spec_mut(c: &mut Ctx) -> Option<&mut Map<String, Value>> {
    let sel = c.scenes.selected.clone()?;
    c.config.draft_mut()?.get_mut("scenes")?.get_mut(&sel)?.as_object_mut()
}

fn set(key: String, v: Value) {
    act(move |c| {
        if let Some(spec) = spec_mut(c) {
            spec.insert(key, v);
            edited(c);
        }
    });
}

fn param(c: &Ctx, key: &str) -> Option<effects::Param> {
    let sel = c.scenes.selected.as_ref()?;
    let e = c.config.draft()?.get("scenes")?.get(sel)?.get("effect")?.as_str()?;
    effect(e)?.params.iter().find(|p| p.key == key).copied()
}

fn value(c: &Ctx, key: &str) -> Option<Value> {
    let sel = c.scenes.selected.as_ref()?;
    c.config.draft()?.get("scenes")?.get(sel)?.get(key).cloned()
}

/// « Erreur » du nom de la nouvelle scène.
fn name_error(cfg: &Value, name: &str) -> &'static str {
    if name.is_empty() {
        ""
    } else if !valid_scene_name(name) {
        "Lettres minuscules, chiffres, - et _ uniquement."
    } else if scenes(cfg).is_some_and(|s| s.contains_key(name)) {
        "Ce nom existe déjà."
    } else {
        ""
    }
}

/// Qui utilise cette scène (empêche de la supprimer).
fn used_by(cfg: &Value, scene: &str) -> Vec<String> {
    let mut u: Vec<String> = cfg
        .get("rules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter(|r| r.get("scene").and_then(Value::as_str) == Some(scene))
                .map(|r| {
                    let n = r.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("sans nom");
                    format!("la règle « {n} »")
                })
                .collect()
        })
        .unwrap_or_default();
    if cfg.pointer("/general/default_scene").and_then(Value::as_str) == Some(scene) {
        u.push("la scène par défaut".into());
    }
    u
}

fn plain(p: &effects::Param, v: Option<&Value>) -> Plain {
    let mut r = Plain {
        key: p.key,
        label: p.label,
        kind: "",
        value: 0.0,
        minimum: 0.0,
        maximum: 1.0,
        step: 0.01,
        text: String::new(),
        checked: false,
        color: [0, 0, 0],
        options: Vec::new(),
        palette: Vec::new(),
        hint: p.hint.unwrap_or_default(),
    };
    match p.kind {
        Kind::Range { min, max, step, unit, default } => {
            let x = v.and_then(Value::as_f64).unwrap_or(default);
            (r.kind, r.value, r.minimum, r.maximum, r.step) = ("range", x, min, max, step);
            r.text = effects::format_range(x, step, unit);
        }
        Kind::Hue { default } => {
            let x = v.and_then(Value::as_f64).unwrap_or(default);
            (r.kind, r.value, r.step) = ("hue", x, 0.005);
            r.color = color::hue(x);
        }
        Kind::Bool { .. } => {
            r.kind = "bool";
            r.checked = v.and_then(Value::as_bool).unwrap_or(false);
        }
        Kind::Color { default } => {
            r.kind = "color";
            r.text = v.and_then(Value::as_str).unwrap_or(default).to_owned();
            r.color = color::parse_hex(&r.text).unwrap_or([0, 0, 0]);
        }
        Kind::Select { options, .. } => {
            r.kind = "select";
            let cur = v.and_then(Value::as_str).unwrap_or_default();
            r.text = options.iter().find(|(k, _)| *k == cur).map(|(_, l)| (*l).to_owned()).unwrap_or_default();
        }
        Kind::Multi { options, .. } => {
            r.kind = "multi";
            let on: Vec<&str> = v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            r.options = options.iter().map(|(k, l)| (*k, *l, on.contains(k))).collect();
        }
        Kind::Palette => {
            r.kind = "palette";
            r.palette = v
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|c| c.as_str().and_then(color::parse_hex).unwrap_or([255, 255, 255])).collect())
                .unwrap_or_default();
        }
    }
    r
}

pub(super) fn refresh(c: &mut Ctx) {
    let sel = selected(c);
    let g = c.win.global::<PrismeScenes>();
    let Some(cfg) = c.config.draft() else {
        g.set_ready(false);
        return;
    };
    g.set_ready(true);
    let all = scenes(cfg).cloned().unwrap_or_default();
    let names = order_scenes(&all.keys().cloned().collect::<Vec<_>>());
    let rows: Vec<PrismeSceneRow> = names
        .iter()
        .map(|n| {
            let e = all[n].get("effect").and_then(Value::as_str).unwrap_or_default();
            PrismeSceneRow {
                key: n.as_str().into(),
                label: scene_label(n).into(),
                kind: effect(e).map_or(e, |i| i.label).into(),
                active: sel.as_ref() == Some(n),
            }
        })
        .collect();
    if g.get_scenes().iter().ne(rows.iter().cloned()) {
        g.set_scenes(list(rows));
    }
    g.set_new_name(c.scenes.new_name.as_str().into());
    g.set_name_error(name_error(cfg, &c.scenes.new_name).into());
    let spec = sel.as_ref().and_then(|s| all.get(s));
    g.set_has_selected(spec.is_some());
    let (Some(sel), Some(spec)) = (sel, spec) else { return };
    let e = spec.get("effect").and_then(Value::as_str).unwrap_or_default();
    let info = effect(e);
    g.set_title(scene_label(&sel).into());
    g.set_summary(info.map_or("", |i| i.summary).into());
    g.set_shown(c.state.as_ref().and_then(|s| s.override_scene.as_deref()) == Some(sel.as_str()));
    let used = used_by(cfg, &sel);
    g.set_can_remove(used.is_empty());
    g.set_remove_tip(if used.is_empty() { String::new() } else { format!("Utilisée par {}", used.join(" et ")) }.into());
    g.set_effect(info.map_or(e, |i| i.label).into());
    let plains: Vec<Plain> = info.map_or(&[][..], |i| i.params).iter().map(|p| plain(p, spec.get(p.key))).collect();
    let model = c.scenes.params.get_or_insert_with(|| {
        let m = Rc::new(VecModel::default());
        g.set_params(m.clone().into());
        m
    });
    sync_rows(model, &mut c.scenes.shown, plains, Plain::row);
}

pub(super) fn bind(win: &PrismeWindow) {
    let g = win.global::<PrismeScenes>();
    g.on_select(|k| {
        act(move |c| {
            c.scenes.selected = Some(k.to_string());
            super::refresh(c);
        })
    });
    g.on_edit_name(|t| {
        act(move |c| {
            c.scenes.new_name = t.to_string();
            super::refresh(c);
        })
    });
    g.on_create(|| act(create));
    g.on_remove(|| act(remove));
    g.on_try_now(|| {
        let sel = with(|c| c.scenes.selected.clone()).flatten();
        if let Some(s) = sel {
            call("set_override", json!({ "scene": s }), |r| {
                if let Err(e) = r {
                    log::info!("Prisme, « Essayer » : {e}");
                }
            });
        }
    });
    g.on_back_auto(|| super::now::choose(None));
    g.on_pick_effect(|x, y| {
        act(move |c| {
            let current = value(c, "effect").and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let choices = effects::EFFECTS.iter().map(|e| (e.key.to_owned(), e.label.to_owned())).collect();
            select(c, x, y, choices, &current, |c, key| {
                if let Some(spec) = spec_mut(c) {
                    *spec = default_spec(&key).as_object().cloned().unwrap_or_default();
                    edited(c);
                }
            });
        })
    });
    g.on_set_number(|k, v| {
        act(move |c| {
            let Some(p) = param(c, &k) else { return };
            let x = match p.kind {
                Kind::Range { min, max, step, .. } => effects::snap(v as f64, step, min, max),
                _ => effects::snap(v as f64, 0.005, 0.0, 1.0),
            };
            if let Some(spec) = spec_mut(c) {
                spec.insert(k.to_string(), number(x));
                edited(c);
            }
        })
    });
    g.on_set_bool(|k, v| set(k.to_string(), json!(v)));
    g.on_pick_color(|k, i| {
        act(move |c| {
            let key = k.to_string();
            let current = value(c, &key);
            let initial = if i < 0 {
                current.as_ref().and_then(Value::as_str).and_then(color::parse_hex)
            } else {
                current.as_ref().and_then(|v| v.get(i as usize)).and_then(Value::as_str).and_then(color::parse_hex)
            };
            pick_color(c, initial.unwrap_or([255, 255, 255]), move |c, rgb| {
                let hex = json!(color::hex(rgb));
                let Some(spec) = spec_mut(c) else { return };
                if i < 0 {
                    spec.insert(key, hex);
                } else if let Some(slot) = spec.get_mut(&key).and_then(Value::as_array_mut).and_then(|a| a.get_mut(i as usize)) {
                    *slot = hex;
                }
                edited(c);
            });
        })
    });
    g.on_pick_option(|k, x, y| {
        act(move |c| {
            let Some(p) = param(c, &k) else { return };
            let Kind::Select { options, .. } = p.kind else { return };
            let current = value(c, &k).and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let choices = options.iter().map(|(k, l)| ((*k).to_owned(), (*l).to_owned())).collect();
            let key = k.to_string();
            select(c, x, y, choices, &current, move |c, chosen| {
                if let Some(spec) = spec_mut(c) {
                    spec.insert(key, json!(chosen));
                    edited(c);
                }
            });
        })
    });
    g.on_toggle_option(|k, o, on| {
        act(move |c| {
            let Some(p) = param(c, &k) else { return };
            let Kind::Multi { options, .. } = p.kind else { return };
            let current = value(c, &k);
            let next = toggled(options, current.as_ref(), &o, on);
            if let Some(spec) = spec_mut(c) {
                spec.insert(k.to_string(), json!(next));
                edited(c);
            }
        })
    });
    g.on_add_color(|k| {
        act(move |c| {
            let Some(spec) = spec_mut(c) else { return };
            let mut v = spec.get(k.as_str()).and_then(Value::as_array).cloned().unwrap_or_default();
            v.push(json!("#ffffff"));
            spec.insert(k.to_string(), Value::Array(v));
            edited(c);
        })
    });
    g.on_remove_color(|k, i| {
        act(move |c| {
            let Some(spec) = spec_mut(c) else { return };
            if let Some(a) = spec.get_mut(k.as_str()).and_then(Value::as_array_mut)
                && (i as usize) < a.len()
            {
                a.remove(i as usize);
            }
            edited(c);
        })
    });
}

/// Coche / décoche une option d'une liste, en gardant l'ordre des options.
fn toggled(options: &[(&str, &str)], current: Option<&Value>, key: &str, on: bool) -> Vec<String> {
    let mut set: Vec<String> =
        current.and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    if on {
        set.push(key.to_owned());
    } else {
        set.retain(|k| k != key);
    }
    options.iter().map(|(k, _)| (*k).to_owned()).filter(|k| set.contains(k)).collect()
}

fn create(c: &mut Ctx) {
    let name = c.scenes.new_name.clone();
    let Some(cfg) = c.config.draft() else { return };
    if name.is_empty() || !name_error(cfg, &name).is_empty() {
        return;
    }
    let Some(all) = c.config.draft_mut().and_then(|d| d.get_mut("scenes")).and_then(Value::as_object_mut) else { return };
    all.insert(name.clone(), default_spec("ambient"));
    c.scenes.selected = Some(name);
    c.scenes.new_name.clear();
    edited(c);
}

fn remove(c: &mut Ctx) {
    let Some(sel) = c.scenes.selected.clone() else { return };
    let Some(cfg) = c.config.draft() else { return };
    if !used_by(cfg, &sel).is_empty() {
        return;
    }
    if let Some(all) = c.config.draft_mut().and_then(|d| d.get_mut("scenes")).and_then(Value::as_object_mut) {
        all.remove(&sel);
    }
    c.scenes.selected = None;
    edited(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_stay_integers_when_they_are() {
        assert_eq!(number(15.0), json!(15));
        assert_eq!(number(0.15), json!(0.15));
    }

    #[test]
    fn scenes_in_use_cannot_be_removed() {
        let cfg = json!({
            "general": { "default_scene": "idle" },
            "rules": [{ "name": "", "scene": "music" }, { "name": "jeux", "scene": "music" }],
            "scenes": { "idle": {}, "music": {} }
        });
        assert_eq!(used_by(&cfg, "music"), ["la règle « sans nom »", "la règle « jeux »"]);
        assert_eq!(used_by(&cfg, "idle"), ["la scène par défaut"]);
        assert!(used_by(&cfg, "zen").is_empty());
        assert_eq!(name_error(&cfg, "music"), "Ce nom existe déjà.");
        assert_eq!(name_error(&cfg, "Zen"), "Lettres minuscules, chiffres, - et _ uniquement.");
        assert_eq!(name_error(&cfg, "zen"), "");
    }

    #[test]
    fn multi_keeps_the_order_of_options() {
        let opts = [("energy", "Énergie"), ("ripple", "Ondes"), ("beat", "Battement")];
        let cur = json!(["beat"]);
        assert_eq!(toggled(&opts, Some(&cur), "energy", true), ["energy", "beat"]);
        assert_eq!(toggled(&opts, Some(&cur), "beat", false), Vec::<String>::new());
    }

    #[test]
    fn params_are_shown_by_kind() {
        let info = effect("spectrum").unwrap();
        let axis = info.params.iter().find(|p| p.key == "axis").unwrap();
        assert_eq!(plain(axis, Some(&json!("radial"))).text, "Centre → bords");
        let hue = info.params.iter().find(|p| p.key == "hue_low").unwrap();
        assert_eq!(plain(hue, None).value, 0.08);
        let rel = info.params.iter().find(|p| p.key == "release_ms").unwrap();
        assert_eq!(plain(rel, Some(&json!(220))).text, "220 ms");
    }
}
