//! Page « Automatique » (`pages/Rules.svelte`) : ce qui choisit la scène tout seul, lu de haut en
//! bas (la première règle qui correspond l'emporte), et la scène par défaut. Global `PrismeRules`.

use super::effects::{order_scenes, scene_label};
use super::{Ctx, act, edited, select, sync_rows};
use crate::ui::{PrismeRule, PrismeRules, PrismeWindow};
use serde_json::{Value, json};
use slint::{ComponentHandle, VecModel};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct State {
    /// Par règle : section « Conditions supplémentaires » ouverte, et l'état « avancé » qui l'a
    /// ouverte ou fermée la dernière fois (`<details open={advanced(rule)}>`).
    open: Vec<(bool, bool)>,
    shown: Vec<Plain>,
    model: Option<Rc<VecModel<PrismeRule>>>,
}

#[derive(Debug, Clone, PartialEq)]
struct Plain {
    name: String,
    media: &'static str,
    scene: String,
    fullscreen: &'static str,
    advanced: bool,
    open: bool,
    apps: String,
    foreground: String,
    first: bool,
    last: bool,
}

impl Plain {
    fn row(&self) -> PrismeRule {
        PrismeRule {
            name: self.name.as_str().into(),
            media: self.media.into(),
            scene: self.scene.as_str().into(),
            fullscreen: self.fullscreen.into(),
            advanced: self.advanced,
            open: self.open,
            apps: self.apps.as_str().into(),
            foreground: self.foreground.as_str().into(),
            first: self.first,
            last: self.last,
        }
    }
}

const MEDIA: [(&str, &str); 3] = [("", "toujours"), ("video", "une vidéo joue"), ("music", "de la musique joue")];
const FULLSCREEN: [(&str, &str); 3] = [("any", "peu importe"), ("yes", "oui"), ("no", "non")];

/// Liste d'exécutables saisie comme texte séparé par des virgules (`normalize` de ListInput).
pub(super) fn normalize(t: &str) -> Vec<String> {
    t.split([',', '\n']).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect()
}

pub(super) fn joined(v: Option<&Value>) -> String {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default()
}

/// Nombre comme l'écrirait le navigateur (3 → « 3 », 2.5 → « 2.5 »).
pub(super) fn js(v: f64) -> String {
    format!("{v}")
}

fn list_len(v: Option<&Value>) -> usize {
    v.and_then(Value::as_array).map_or(0, Vec::len)
}

/// Une règle a-t-elle des conditions au-delà de « ce qui joue » ?
fn advanced(r: &Value) -> bool {
    let w = r.get("when");
    list_len(w.and_then(|w| w.get("app"))) > 0
        || list_len(w.and_then(|w| w.get("foreground"))) > 0
        || w.and_then(|w| w.get("fullscreen")).is_some_and(|f| !f.is_null())
}

fn rules(cfg: &Value) -> &[Value] {
    cfg.get("rules").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn rules_mut(c: &mut Ctx) -> Option<&mut Vec<Value>> {
    c.config.draft_mut()?.get_mut("rules")?.as_array_mut()
}

fn when_mut(c: &mut Ctx, i: usize) -> Option<&mut serde_json::Map<String, Value>> {
    let r = rules_mut(c)?.get_mut(i)?.as_object_mut()?;
    r.entry("when").or_insert_with(|| json!({})).as_object_mut()
}

fn scenes(cfg: &Value) -> Vec<String> {
    order_scenes(&cfg.get("scenes").and_then(Value::as_object).map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default())
}

pub(super) fn refresh(c: &mut Ctx) {
    let g = c.win.global::<PrismeRules>();
    let Some(cfg) = c.config.draft() else {
        g.set_ready(false);
        return;
    };
    g.set_ready(true);
    let delay = cfg.pointer("/general/exit_delay_s").and_then(Value::as_f64).unwrap_or(0.0);
    g.set_intro(
        format!(
            "Prisme choisit la scène tout seul : la première règle qui correspond l'emporte. Il attend {} s de calme avant de revenir en arrière.",
            js(delay)
        )
        .into(),
    );
    match &c.state {
        Some(st) => {
            g.set_has_now(true);
            g.set_now_scene(scene_label(&st.scene).into());
            g.set_now_reason(st.reason.as_str().into());
        }
        None => g.set_has_now(false),
    }
    let list = rules(cfg);
    let n = list.len();
    c.rules.open.truncate(n);
    let mut plains = Vec::with_capacity(n);
    for (i, r) in list.iter().enumerate() {
        let adv = advanced(r);
        match c.rules.open.get_mut(i) {
            Some(o) if o.1 != adv => *o = (adv, adv),
            Some(_) => {}
            None => c.rules.open.push((adv, adv)),
        }
        let w = r.get("when");
        let media = w.and_then(|w| w.get("media")).and_then(Value::as_str).unwrap_or_default();
        let fs = match w.and_then(|w| w.get("fullscreen")).and_then(Value::as_bool) {
            Some(true) => "yes",
            Some(false) => "no",
            None => "any",
        };
        let label = |opts: &[(&str, &'static str)], k: &str| opts.iter().find(|o| o.0 == k).map_or("", |o| o.1);
        plains.push(Plain {
            name: r.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
            media: label(&MEDIA, media),
            scene: r.get("scene").and_then(Value::as_str).map(scene_label).unwrap_or_default(),
            fullscreen: label(&FULLSCREEN, fs),
            advanced: adv,
            open: c.rules.open[i].0,
            apps: joined(w.and_then(|w| w.get("app"))),
            foreground: joined(w.and_then(|w| w.get("foreground"))),
            first: i == 0,
            last: i + 1 == n,
        });
    }
    g.set_default_scene(cfg.pointer("/general/default_scene").and_then(Value::as_str).map(scene_label).unwrap_or_default().into());
    let model = c.rules.model.get_or_insert_with(|| {
        let m = Rc::new(VecModel::default());
        g.set_rules(m.clone().into());
        m
    });
    sync_rows(model, &mut c.rules.shown, plains, Plain::row);
}

/// Modifie la règle `i` et enregistre.
fn edit(i: i32, f: impl FnOnce(&mut Ctx, usize) + 'static) {
    act(move |c| {
        if i >= 0 {
            f(c, i as usize);
            edited(c);
        }
    });
}

fn swap(c: &mut Ctx, i: usize, j: usize) {
    if let Some(r) = rules_mut(c)
        && i < r.len()
        && j < r.len()
    {
        r.swap(i, j);
        if i < c.rules.open.len() && j < c.rules.open.len() {
            c.rules.open.swap(i, j);
        }
    }
}

fn scene_choices(c: &Ctx) -> Vec<(String, String)> {
    c.config.draft().map(scenes).unwrap_or_default().into_iter().map(|s| (s.clone(), scene_label(&s))).collect()
}

pub(super) fn bind(win: &PrismeWindow) {
    let g = win.global::<PrismeRules>();
    g.on_rename(|i, t| {
        edit(i, move |c, i| {
            if let Some(r) = rules_mut(c).and_then(|r| r.get_mut(i)) {
                r["name"] = json!(t.as_str());
            }
        })
    });
    g.on_up(|i| edit(i, |c, i| swap(c, i, i.wrapping_sub(1))));
    g.on_down(|i| edit(i, |c, i| swap(c, i, i + 1)));
    g.on_remove(|i| {
        edit(i, |c, i| {
            if let Some(r) = rules_mut(c)
                && i < r.len()
            {
                r.remove(i);
                if i < c.rules.open.len() {
                    c.rules.open.remove(i);
                }
            }
        })
    });
    g.on_toggle(|i| {
        act(move |c| {
            if let Some(o) = c.rules.open.get_mut(i as usize) {
                o.0 = !o.0;
            }
            super::refresh(c);
        })
    });
    g.on_pick_media(|i, x, y| {
        act(move |c| {
            let cur = c.config.draft().and_then(|d| rules(d).get(i as usize)).and_then(|r| r.pointer("/when/media")).and_then(Value::as_str);
            let cur = cur.unwrap_or_default().to_owned();
            let choices = MEDIA.iter().map(|(k, l)| ((*k).to_owned(), (*l).to_owned())).collect();
            select(c, x, y, choices, &cur, move |c, k| {
                if let Some(w) = when_mut(c, i as usize) {
                    w.insert("media".into(), if k.is_empty() { Value::Null } else { json!(k) });
                    edited(c);
                }
            });
        })
    });
    g.on_pick_scene(|i, x, y| {
        act(move |c| {
            let cur = c.config.draft().and_then(|d| rules(d).get(i as usize)).and_then(|r| r.get("scene")).and_then(Value::as_str);
            let cur = cur.unwrap_or_default().to_owned();
            select(c, x, y, scene_choices(c), &cur, move |c, k| {
                if let Some(r) = rules_mut(c).and_then(|r| r.get_mut(i as usize)) {
                    r["scene"] = json!(k);
                    edited(c);
                }
            });
        })
    });
    g.on_pick_fullscreen(|i, x, y| {
        act(move |c| {
            let cur = c.config.draft().and_then(|d| rules(d).get(i as usize)).and_then(|r| r.pointer("/when/fullscreen")).and_then(Value::as_bool);
            let cur = match cur {
                Some(true) => "yes",
                Some(false) => "no",
                None => "any",
            };
            let choices = FULLSCREEN.iter().map(|(k, l)| ((*k).to_owned(), (*l).to_owned())).collect();
            select(c, x, y, choices, cur, move |c, k| {
                if let Some(w) = when_mut(c, i as usize) {
                    let v = match k.as_str() {
                        "yes" => json!(true),
                        "no" => json!(false),
                        _ => Value::Null,
                    };
                    w.insert("fullscreen".into(), v);
                    edited(c);
                }
            });
        })
    });
    g.on_set_apps(|i, t| {
        edit(i, move |c, i| {
            if let Some(w) = when_mut(c, i) {
                w.insert("app".into(), json!(normalize(&t)));
            }
        })
    });
    g.on_set_foreground(|i, t| {
        edit(i, move |c, i| {
            if let Some(w) = when_mut(c, i) {
                w.insert("foreground".into(), json!(normalize(&t)));
            }
        })
    });
    g.on_add(|| {
        act(|c| {
            let scene = c.config.draft().and_then(|d| d.pointer("/general/default_scene")).cloned().unwrap_or(Value::Null);
            let rule = json!({
                "name": "Nouvelle règle",
                "when": { "media": null, "app": [], "foreground": [], "fullscreen": null },
                "scene": scene,
            });
            if let Some(d) = c.config.draft_mut()
                && let Some(o) = d.as_object_mut()
                && let Some(r) = o.entry("rules").or_insert_with(|| json!([])).as_array_mut()
            {
                r.push(rule);
                edited(c);
            }
        })
    });
    g.on_pick_default(|x, y| {
        act(move |c| {
            let cur = c.config.draft().and_then(|d| d.pointer("/general/default_scene")).and_then(Value::as_str).unwrap_or_default().to_owned();
            select(c, x, y, scene_choices(c), &cur, |c, k| {
                if let Some(g) = c.config.draft_mut().and_then(|d| d.get_mut("general")).and_then(Value::as_object_mut) {
                    g.insert("default_scene".into(), json!(k));
                    edited(c);
                }
            });
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_are_normalized() {
        assert_eq!(normalize(" Spotify.exe, ,VLC.exe\nfoo "), ["spotify.exe", "vlc.exe", "foo"]);
        assert!(normalize("").is_empty());
        assert_eq!(joined(Some(&json!(["a.exe", "b.exe"]))), "a.exe, b.exe");
    }

    #[test]
    fn advanced_rules_open_their_conditions() {
        assert!(!advanced(&json!({ "when": { "media": "video", "app": [], "foreground": [], "fullscreen": null } })));
        assert!(advanced(&json!({ "when": { "app": ["vlc.exe"] } })));
        assert!(advanced(&json!({ "when": { "fullscreen": false } })));
        assert_eq!(js(3.0), "3");
        assert_eq!(js(2.5), "2.5");
    }
}
