//! Page « Réglages » (`pages/Settings.svelte`) : bascule entre les scènes, reconnaissance des
//! applis, fichier de configuration brut (TOML : lu par `get_config`, enregistré par
//! `set_config`). Global `PrismeSettings`.

use super::rules::{joined, js, normalize};
use super::scenes::number;
use super::screens;
use super::{Ctx, act, call, edited, load_config, select, with};
use crate::ui::{PrismeSettings, PrismeWindow};
use serde_json::{Value, json};
use slint::ComponentHandle;
use std::time::Duration;

#[derive(Default)]
pub(super) struct State {
    raw_open: bool,
    raw: String,
    path: String,
    raw_error: String,
    raw_saved: bool,
}

thread_local! {
    static SAVED: slint::Timer = slint::Timer::default();
}

const LISTS: [&str; 4] = ["video_apps", "music_apps", "browsers", "ignore_apps"];

fn general(cfg: &Value, k: &str) -> f64 {
    cfg.get("general").and_then(|g| g.get(k)).and_then(Value::as_f64).unwrap_or(0.0)
}

fn set_general(c: &mut Ctx, k: &str, v: Value) {
    if let Some(g) = c.config.draft_mut().and_then(|d| d.get_mut("general")).and_then(Value::as_object_mut) {
        g.insert(k.into(), v);
        edited(c);
    }
}

/// Écrans proposés pour l'ambilight : « Écran principal », puis les autres.
fn monitors(c: &Ctx) -> Vec<(String, String)> {
    let screens = c.state.as_ref().map_or(&[][..], |s| &s.screens);
    let mut v = vec![(String::new(), "Écran principal".to_owned())];
    v.extend(screens.iter().filter(|s| !s.primary).map(|s| (s.name.clone(), screens::label(Some(&s.name), screens))));
    v
}

pub(super) fn refresh(c: &mut Ctx) {
    let g = c.win.global::<PrismeSettings>();
    let Some(cfg) = c.config.draft() else {
        g.set_ready(false);
        return;
    };
    g.set_ready(true);
    let tr = general(cfg, "transition_ms");
    g.set_transition(tr as f32);
    g.set_transition_text(format!("{:.2} s", tr / 1000.0).into());
    let ex = general(cfg, "exit_delay_s");
    g.set_exit_delay(ex as f32);
    g.set_exit_text(format!("{} s", js(ex)).into());
    let mon = cfg.pointer("/general/ambilight_monitor").and_then(Value::as_str).unwrap_or_default();
    let label = monitors(c).into_iter().find(|(k, _)| k == mon).map(|(_, l)| l).unwrap_or_default();
    g.set_monitor(label.into());
    let list = |k: &str| joined(cfg.get("classify").and_then(|cl| cl.get(k))).into();
    g.set_video_apps(list("video_apps"));
    g.set_music_apps(list("music_apps"));
    g.set_browsers(list("browsers"));
    g.set_ignore_apps(list("ignore_apps"));
    let s = &c.settings;
    g.set_raw_open(s.raw_open);
    g.set_raw(s.raw.as_str().into());
    g.set_path(s.path.as_str().into());
    g.set_raw_error(s.raw_error.as_str().into());
    g.set_raw_saved(s.raw_saved);
}

pub(super) fn bind(win: &PrismeWindow) {
    let g = win.global::<PrismeSettings>();
    g.on_set_transition(|v| act(move |c| set_general(c, "transition_ms", number(super::effects::snap(v as f64, 50.0, 0.0, 3000.0)))));
    g.on_set_exit(|v| act(move |c| set_general(c, "exit_delay_s", number(super::effects::snap(v as f64, 0.5, 0.0, 30.0)))));
    g.on_pick_monitor(|x, y| {
        act(move |c| {
            let cur = c.config.draft().and_then(|d| d.pointer("/general/ambilight_monitor")).and_then(Value::as_str).unwrap_or_default().to_owned();
            select(c, x, y, monitors(c), &cur, |c, k| set_general(c, "ambilight_monitor", json!(k)));
        })
    });
    g.on_set_list(|k, t| {
        act(move |c| {
            if !LISTS.contains(&k.as_str()) {
                return;
            }
            let Some(d) = c.config.draft_mut().and_then(Value::as_object_mut) else { return };
            if let Some(cl) = d.entry("classify").or_insert_with(|| json!({})).as_object_mut() {
                cl.insert(k.to_string(), json!(normalize(&t)));
                edited(c);
            }
        })
    });
    g.on_open_raw(|| {
        call("get_config", json!({}), |r| {
            with(|c| {
                if let Ok(v) = r {
                    c.settings.raw = v.get("toml").and_then(Value::as_str).unwrap_or_default().to_owned();
                    c.settings.path = v.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
                    c.settings.raw_error.clear();
                    c.settings.raw_open = true;
                }
                super::refresh(c);
            });
        })
    });
    g.on_close_raw(|| {
        act(|c| {
            c.settings.raw_open = false;
            super::refresh(c);
        })
    });
    g.on_save_raw(|text| {
        let text = text.to_string();
        with(|c| c.settings.raw = text.clone());
        call("set_config", json!({ "toml": text }), |r| {
            let ok = r.is_ok();
            with(|c| {
                match r {
                    Ok(_) => {
                        c.settings.raw_error.clear();
                        c.settings.raw_saved = true;
                    }
                    Err(e) => c.settings.raw_error = e,
                }
                super::refresh(c);
            });
            if ok {
                SAVED.with(|t| {
                    t.start(slint::TimerMode::SingleShot, Duration::from_secs(2), || {
                        with(|c| {
                            c.settings.raw_saved = false;
                            super::refresh(c);
                        });
                    })
                });
                load_config();
            }
        });
    });
}
