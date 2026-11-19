//! Côté interface des scripts (fil de l'interface) : un script est chargé quand son plugin a au
//! moins un exemplaire dans la disposition, rechargé quand le plugin change, retiré sinon. Ses
//! `horizon.ui.set` sont posés sur les exemplaires (et rejoués sur ceux créés ensuite), ses
//! `horizon.ui.on` branchés sur leurs rappels Slint.

use super::Plugins;
use super::convert::{json_to_value, value_to_json};
use super::js::{self, FromJs, Instance, ToJs};
use super::slint_host::Surface;
use crate::host::{AppHandle, Manager};
use crossbeam_channel::Sender;
use horizon_core::plugin::{coerce, defaults, id_of_kind};
use serde_json::Value as Json;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Canal vers le fil des scripts (posé au démarrage des plugins).
static JS: OnceLock<Sender<ToJs>> = OnceLock::new();
/// Le fil des scripts a fini (réponse à `ToJs::Quit`).
static QUIT_DONE: AtomicBool = AtomicBool::new(false);

/// Un script chargé.
struct Loaded {
    generation: u64,
    instances: Vec<Instance>,
    dir: PathBuf,
    /// Valeurs posées par le script : (exemplaire ou tous, propriété) → valeur.
    values: Vec<(Option<String>, String, Json)>,
    /// Rappels Slint écoutés.
    callbacks: Vec<String>,
}

thread_local! {
    static LOADED: RefCell<HashMap<String, Loaded>> = RefCell::default();
}

fn send(msg: ToJs) {
    if let Some(tx) = JS.get() {
        let _ = tx.send(msg);
    }
}

/// Lance le fil des scripts.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    let tx = js::spawn(move |m| {
        if m == FromJs::Quit {
            QUIT_DONE.store(true, Ordering::SeqCst);
            return;
        }
        let a = a.clone();
        let _ = slint::invoke_from_event_loop(move || from_js(&a, m));
    })?;
    JS.set(tx).map_err(|_| anyhow::anyhow!("fil des scripts déjà lancé"))
}

/// Fermeture d'Horizon : `unload` des scripts (500 ms au plus).
pub fn quit() {
    send(ToJs::Quit);
    let end = Instant::now() + js::UNLOAD_LIMIT + Duration::from_millis(100);
    while !QUIT_DONE.load(Ordering::SeqCst) && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Exemplaires de chaque plugin dans la disposition, avec leurs réglages (défauts complétés).
fn instances(app: &AppHandle) -> HashMap<String, Vec<Instance>> {
    let plugins = app.state::<Plugins>();
    let widgets: Vec<(String, String, Json)> = app.state::<crate::state::AppState>().read(|l| {
        l.widgets()
            .filter_map(|w| Some((id_of_kind(w.kind.as_str())?.to_owned(), w.id.clone(), w.settings.clone())))
            .collect()
    });
    let mut out: HashMap<String, Vec<Instance>> = HashMap::new();
    for (plugin, id, stored) in widgets {
        let Some(m) = plugins.manifest(&plugin) else { continue };
        let mut values = defaults(&m.settings);
        for spec in &m.settings {
            if spec.kind == horizon_core::plugin::SettingKind::Secret {
                // Lu dans le Gestionnaire d'identifiants (jamais dans la disposition).
                let target = crate::ui::widget_settings::plugin::secret_target(&plugin, &id, &spec.key);
                if let Some(secret) = horizon_sys::credentials::read(&target) {
                    values.insert(spec.key.clone(), Json::String(secret));
                }
            } else if let Some(v) = stored.get(&spec.key) {
                values.insert(spec.key.clone(), coerce(spec, v));
            }
        }
        out.entry(plugin).or_default().push(Instance { id, settings: Json::Object(values) });
    }
    out
}

/// Scripts chargés, rechargés ou retirés d'après la disposition et le registre.
pub fn reconcile(app: &AppHandle) {
    let wanted = instances(app);
    let plugins = app.state::<Plugins>();
    let gone: Vec<String> = LOADED.with_borrow(|l| l.keys().filter(|k| !wanted.contains_key(*k)).cloned().collect());
    for id in gone {
        LOADED.with_borrow_mut(|l| l.remove(&id));
        send(ToJs::Unload(id));
    }
    for (id, list) in wanted {
        let Some((manifest, dir, generation, _)) = plugins.loadable(&id) else {
            if LOADED.with_borrow_mut(|l| l.remove(&id)).is_some() {
                send(ToJs::Unload(id));
            }
            continue;
        };
        let current = LOADED.with_borrow(|l| l.get(&id).map(|x| (x.generation, x.instances.clone())));
        match current {
            Some((g, old)) if g == generation => {
                if old != list {
                    LOADED.with_borrow_mut(|l| {
                        if let Some(x) = l.get_mut(&id) {
                            x.instances = list.clone();
                        }
                    });
                    send(ToJs::Instances { id, list });
                }
            }
            _ => {
                let main = std::fs::read_to_string(dir.join("main.js")).ok();
                LOADED.with_borrow_mut(|l| {
                    l.insert(id.clone(), Loaded { generation, instances: list.clone(), dir, values: Vec::new(), callbacks: Vec::new() })
                });
                match main {
                    Some(main) => {
                        let permissions = manifest.permissions.clone();
                        let replay = super::events::replay(&permissions);
                        send(ToJs::Load { id: id.clone(), main, instances: list, permissions });
                        for (name, payload) in replay {
                            send(ToJs::Event { id: id.clone(), name, payload });
                        }
                    }
                    // Plugin sans script (interface seule).
                    None => send(ToJs::Unload(id)),
                }
            }
        }
    }
}

/// Exemplaire de la clé d'une surface (`w1`, `w1#panel` → `w1`).
fn instance_of(key: &str) -> &str {
    key.trim_end_matches(super::view::PANEL_SUFFIX)
}

/// Pose `value` sur la propriété `prop` d'une surface, au type de la propriété.
fn set_on(surface: &Surface, dir: &std::path::Path, prop: &str, value: &Json) {
    let inst = surface.instance();
    let Some(ty) = inst.definition().properties().find(|(n, _)| n.replace('_', "-") == prop.replace('_', "-")).map(|(_, t)| t)
    else {
        return;
    };
    match json_to_value(value, ty, dir) {
        Some(v) => {
            if let Err(e) = inst.set_property(prop, v) {
                log::debug!("plugin : {prop} : {e}");
            }
        }
        None => log::debug!("plugin : {prop} : valeur {value} de type inattendu"),
    }
}

/// Branche le rappel Slint `name` d'une surface sur le script.
fn listen_on(surface: &Surface, plugin: &str, key: &str, name: &str) {
    let (id, instance, cb) = (plugin.to_owned(), instance_of(key).to_owned(), name.to_owned());
    let _ = surface.instance().set_callback(name, move |args| {
        let args = args.iter().map(value_to_json).collect();
        send(ToJs::Callback { id: id.clone(), instance: instance.clone(), name: cb.clone(), args });
        slint_interpreter::Value::Void
    });
}

/// Une surface vient d'être créée : valeurs et rappels du script rejoués.
pub fn surface_created(plugin: &str, key: &str, surface: &Surface) {
    let Some((dir, values, callbacks)) =
        LOADED.with_borrow(|l| l.get(plugin).map(|x| (x.dir.clone(), x.values.clone(), x.callbacks.clone())))
    else {
        return;
    };
    let me = instance_of(key);
    // Valeurs pour tous d'abord, puis celles de cet exemplaire (elles l'emportent).
    for (_, prop, value) in values.iter().filter(|v| v.0.is_none()).chain(values.iter().filter(|v| v.0.as_deref() == Some(me))) {
        set_on(surface, &dir, prop, value);
    }
    for name in &callbacks {
        listen_on(surface, plugin, key, name);
    }
}

/// Message du fil des scripts.
fn from_js(app: &AppHandle, m: FromJs) {
    match m {
        FromJs::UiSet { id, instance, prop, value } => {
            let Some(dir) = LOADED.with_borrow_mut(|l| {
                let x = l.get_mut(&id)?;
                x.values.retain(|v| !(v.0 == instance && v.1 == prop));
                x.values.push((instance.clone(), prop.clone(), value.clone()));
                Some(x.dir.clone())
            }) else {
                return;
            };
            for (key, surface) in super::view::surfaces_of(&id) {
                if instance.as_deref().is_none_or(|i| i == instance_of(&key)) {
                    set_on(&surface.borrow(), &dir, &prop, &value);
                }
            }
        }
        FromJs::UiOn { id, name } => {
            let fresh = LOADED.with_borrow_mut(|l| {
                let x = l.get_mut(&id)?;
                (!x.callbacks.contains(&name)).then(|| x.callbacks.push(name.clone()))
            });
            if fresh.is_some() {
                for (key, surface) in super::view::surfaces_of(&id) {
                    listen_on(&surface.borrow(), &id, &key, &name);
                }
            }
        }
        FromJs::Log { id, level, text } => log::log!(level, "plugin {id} : {text}"),
        FromJs::Crash { id, message } => {
            log::warn!("plugin {id} : {message}");
            app.state::<Plugins>().script_crashed(app, &id, message);
        }
        FromJs::Cost { id, us } => app.state::<Plugins>().record_cost(&id, us),
        FromJs::Island { id, key, spec } => {
            let activity = spec.map(|s| crate::ui::island::Activity {
                icon: crate::ui::icons::static_name(s["icon"].as_str().unwrap_or_default()).unwrap_or("squaresFour"),
                title: text(&s["title"], 120),
                subtitle: text(&s["subtitle"], 160),
                since_ms: s["since"].as_u64(),
                attention: s["severity"].as_str() == Some("attention"),
            });
            crate::ui::island::set_activity(format!("{id}:{key}"), activity);
        }
        FromJs::Alert { id, spec } => {
            let icon = crate::ui::icons::static_name(spec["icon"].as_str().unwrap_or_default()).unwrap_or("squaresFour");
            let alert = crate::alerts::Alert::icon(icon, crate::alerts::Severity::Info, text(&spec["title"], 120), text(&spec["body"], 200));
            log::info!("plugin {id} : alerte « {} »", alert.title);
            crate::alerts::show(app, horizon_core::dnd::AlertKind::Plugin, alert);
        }
        FromJs::Palette { id, entries } => {
            let list = entries
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            let key = e["id"].as_str()?.chars().take(64).collect();
                            let icon = crate::ui::icons::static_name(e["icon"].as_str().unwrap_or_default()).unwrap_or("squaresFour");
                            Some(super::PaletteEntry { id: key, label: text(&e["label"], 80), icon })
                        })
                        .collect()
                })
                .unwrap_or_default();
            app.state::<Plugins>().set_palette(&id, list);
        }
        FromJs::Open { url } => {
            if let Err(e) = crate::web::url_open(&url) {
                log::warn!("plugin : lien {url} : {e}");
            }
        }
        FromJs::Quit => {}
    }
}

/// Texte d'un champ JSON, raccourci à `max` caractères.
fn text(v: &Json, max: usize) -> String {
    v.as_str().unwrap_or_default().chars().take(max).collect()
}

/// Événement d'Horizon vers les scripts chargés qui l'ont déclaré (`events::emit`).
pub fn event(app: &AppHandle, name: &str, payload: &Json) {
    let plugins = app.state::<Plugins>();
    let ids: Vec<String> = LOADED.with_borrow(|l| l.keys().cloned().collect());
    for id in ids {
        if plugins.manifest(&id).is_some_and(|m| m.permissions.allows_event(name)) {
            send(ToJs::Event { id, name: name.to_owned(), payload: payload.clone() });
        }
    }
}

/// Entrée de palette d'un plugin choisie.
pub fn palette_choice(id: &str, entry: &str) {
    send(ToJs::Event { id: id.to_owned(), name: "palette".to_owned(), payload: Json::String(entry.to_owned()) });
}
