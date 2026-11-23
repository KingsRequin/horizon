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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Canal vers le fil des scripts (posé au démarrage des plugins).
static JS: OnceLock<Sender<ToJs>> = OnceLock::new();
/// Le fil des scripts a fini (réponse à `ToJs::Quit`).
static QUIT_DONE: AtomicBool = AtomicBool::new(false);
/// Messages des scripts en attente du fil de l'interface, traités par lots (un seul réveil).
#[derive(Default)]
struct Pending {
    msgs: Vec<FromJs>,
    /// Messages en attente de chaque plugin (sa part de la file).
    per_plugin: HashMap<String, usize>,
    /// Plugins déjà comptés comme plantés pour avoir dépassé leur part, pendant ce lot.
    blamed: std::collections::HashSet<String>,
}

impl Pending {
    /// Garde un seul message de contrôle de chaque sorte (plantage, chargement) par plugin.
    fn push_control(&mut self, m: FromJs) {
        let same = |x: &FromJs| match (x, &m) {
            (FromJs::Crash { id: a, .. }, FromJs::Crash { id: b, .. }) | (FromJs::Loaded { id: a }, FromJs::Loaded { id: b }) => a == b,
            _ => false,
        };
        self.msgs.retain(|x| !same(x));
        self.msgs.push(m);
    }
}

static QUEUE: std::sync::LazyLock<Mutex<Pending>> = std::sync::LazyLock::new(Mutex::default);
static SCHEDULED: AtomicBool = AtomicBool::new(false);
/// Messages en attente au plus, en tout et par plugin.
const MAX_QUEUE: usize = 5000;
const MAX_PER_PLUGIN: usize = 1000;
/// Longueur maximale du texte d'un plantage.
const MAX_CRASH_TEXT: usize = 1000;
/// Taille maximale de `main.js` (lu sur le fil de l'interface).
const MAX_MAIN: u64 = 1024 * 1024;
/// Valeurs gardées au plus tant que l'interface du plugin n'est pas compilée (on ne peut pas
/// encore vérifier que la propriété existe ; triées à la création du premier exemplaire).
const MAX_UNCHECKED_VALUES: usize = 64;

/// Seau à jetons : `capacity` d'un coup, puis `per_sec` par seconde.
struct Bucket {
    capacity: f64,
    per_sec: f64,
    tokens: f64,
    at: Instant,
}

impl Bucket {
    fn new(capacity: f64, per_sec: f64, now: Instant) -> Bucket {
        Bucket { capacity, per_sec, tokens: capacity, at: now }
    }

    /// Prend un jeton s'il en reste.
    fn take(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.tokens = (self.tokens + dt * self.per_sec).min(self.capacity);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Débits d'un plugin : lignes de journal, alertes, ce qu'il ouvre (liens, page de réglages),
/// terminaux SSH.
struct Rates {
    log: Bucket,
    alert: Bucket,
    open: Bucket,
    ssh: Bucket,
}

impl Rates {
    fn new(now: Instant) -> Rates {
        Rates {
            log: Bucket::new(40.0, 20.0, now),
            alert: Bucket::new(1.0, 0.2, now),
            open: Bucket::new(1.0, 1.0, now),
            ssh: Bucket::new(1.0, 1.0, now),
        }
    }
}

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
    static RATES: RefCell<HashMap<String, Rates>> = RefCell::default();
}

/// Le débit `which` du plugin `id` permet encore un message.
fn allowed(id: &str, which: fn(&mut Rates) -> &mut Bucket) -> bool {
    let now = Instant::now();
    RATES.with_borrow_mut(|r| which(r.entry(id.to_owned()).or_insert_with(|| Rates::new(now))).take(now))
}

/// Le plugin `id` n'est plus chargé : son activité dans l'îlot et ses entrées de palette partent.
fn forget(app: &AppHandle, id: &str) {
    LOADED.with_borrow_mut(|l| l.remove(id));
    RATES.with_borrow_mut(|r| r.remove(id));
    crate::ui::island::clear_plugin(id);
    app.state::<Plugins>().set_palette(id, Vec::new());
    super::view::set_script_error(id, None);
    send(ToJs::Unload(id.to_owned()));
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
        // Coût, plantages et remise à zéro vont au budget directement, dans l'ordre où le fil
        // des scripts les produit (jamais perdus dans la file).
        let plugins = a.state::<Plugins>();
        let m = match m {
            FromJs::Quit => {
                QUIT_DONE.store(true, Ordering::SeqCst);
                return;
            }
            FromJs::Cost { id, us } => {
                plugins.record_cost(&id, us);
                return;
            }
            FromJs::Crash { id, message, import } => {
                let message: String = message.chars().take(MAX_CRASH_TEXT).collect();
                plugins.script_crashed(&a, &id, message.clone());
                FromJs::Crash { id, message, import }
            }
            FromJs::Loaded { id } => {
                // Nouvelle version chargée : coût et plantages comptés de zéro.
                plugins.reset_counts(&id);
                FromJs::Loaded { id }
            }
            other => other,
        };
        {
            let mut q = crate::util::lock(&QUEUE);
            if matches!(m, FromJs::Crash { .. } | FromJs::Loaded { .. }) {
                q.push_control(m);
            } else if let Some(id) = plugin_of_message(&m) {
                let mine = q.per_plugin.get(&id).copied().unwrap_or(0);
                if mine >= MAX_PER_PLUGIN || q.msgs.len() >= MAX_QUEUE {
                    // Seul le plugin qui dépasse sa propre part est compté comme planté (une fois
                    // par lot) ; les autres messages en trop sont perdus.
                    if mine >= MAX_PER_PLUGIN && q.blamed.insert(id.clone()) {
                        plugins.script_crashed(&a, &id, "trop de messages pour l'interface".to_owned());
                    }
                    return;
                }
                *q.per_plugin.entry(id).or_default() += 1;
                q.msgs.push(m);
            }
        }
        if !SCHEDULED.swap(true, Ordering::SeqCst) {
            let a = a.clone();
            let _ = slint::invoke_from_event_loop(move || drain(&a));
        }
    })?;
    JS.set(tx).map_err(|_| anyhow::anyhow!("fil des scripts déjà lancé"))
}

/// Plugin qui a produit un message destiné à l'interface.
fn plugin_of_message(m: &FromJs) -> Option<String> {
    match m {
        FromJs::UiSet { id, .. }
        | FromJs::UiOn { id, .. }
        | FromJs::Island { id, .. }
        | FromJs::Alert { id, .. }
        | FromJs::Palette { id, .. }
        | FromJs::Log { id, .. }
        | FromJs::Open { id, .. }
        | FromJs::Ssh { id, .. }
        | FromJs::SettingsOpen { id, .. } => Some(id.clone()),
        _ => None,
    }
}

/// Traite les messages en attente ; le temps processeur passé pour chaque message compte dans
/// le budget du plugin qui l'a produit (pas les attentes : Windows qui tarde à répondre).
fn drain(app: &AppHandle) {
    SCHEDULED.store(false, Ordering::SeqCst);
    let batch = std::mem::take(&mut *crate::util::lock(&QUEUE)).msgs;
    let plugins = app.state::<Plugins>();
    for m in batch {
        let charged = plugin_of_message(&m);
        let meter = horizon_sys::thread_time::CpuMeter::start();
        from_js(app, m);
        if let Some(id) = charged {
            plugins.record_cost(&id, meter.elapsed().as_micros() as u64);
        }
    }
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
        forget(app, &id);
    }
    for (id, list) in wanted {
        let Some((manifest, dir, generation, _)) = plugins.loadable(&id) else {
            if LOADED.with_borrow(|l| l.contains_key(&id)) {
                forget(app, &id);
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
                let path = dir.join("main.js");
                let main = match std::fs::metadata(&path) {
                    Ok(m) if m.len() > MAX_MAIN => {
                        super::view::set_script_error(&id, Some("fichier trop gros (1 Mo au plus)".into()));
                        None
                    }
                    Ok(_) => std::fs::read_to_string(&path).ok(),
                    Err(_) => None,
                };
                // Valeurs et rappels de la version en place gardés : si la nouvelle ne se
                // charge pas, l'ancienne continue (le fil des scripts la garde aussi).
                LOADED.with_borrow_mut(|l| {
                    let (values, callbacks) = l.remove(&id).map(|x| (x.values, x.callbacks)).unwrap_or_default();
                    l.insert(id.clone(), Loaded { generation, instances: list.clone(), dir, values, callbacks })
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

/// `value` est déjà la dernière valeur posée sur `prop` pour les mêmes exemplaires : rien ne
/// change, rien à redessiner (un script qui repose tout son état à chaque message ne coûte pas
/// un rendu par exemplaire).
fn unchanged(values: &[(Option<String>, String, Json)], instance: Option<&str>, prop: &str, value: &Json) -> bool {
    values.iter().rev().find(|v| v.1 == prop).is_some_and(|v| v.0.as_deref() == instance && &v.2 == value)
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
            surface.touch();
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
    // Valeurs posées avant que l'interface soit compilée : celles qui ne visent aucune propriété
    // partent maintenant qu'on peut le savoir.
    let unknown: Vec<String> = LOADED.with_borrow(|l| l.get(plugin).map(|x| x.values.iter().map(|v| v.1.clone()).collect()).unwrap_or_default());
    let unknown: Vec<String> = unknown.into_iter().filter(|p| super::view::has_property(plugin, p) == Some(false)).collect();
    let Some((dir, values, callbacks)) = LOADED.with_borrow_mut(|l| {
        l.get_mut(plugin).map(|x| {
            x.values.retain(|v| !unknown.contains(&v.1));
            (x.dir.clone(), x.values.clone(), x.callbacks.clone())
        })
    }) else {
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
            // Gardée (pour les exemplaires créés ensuite) seulement si la propriété existe et
            // que l'exemplaire visé existe : sinon le cache grandirait sans fin. Interface pas
            // encore compilée (script chargé avant les barres) : gardée quand même, en nombre
            // borné, sinon l'exemplaire naîtrait vide jusqu'à la prochaine pose du script.
            let known = super::view::has_property(&id, &prop);
            if known == Some(false) {
                return;
            }
            let Some(dir) = LOADED.with_borrow_mut(|l| {
                let x = l.get_mut(&id)?;
                if instance.as_ref().is_some_and(|i| !x.instances.iter().any(|k| &k.id == i)) {
                    return None;
                }
                if known.is_none() && x.values.len() >= MAX_UNCHECKED_VALUES && !x.values.iter().any(|v| v.0 == instance && v.1 == prop) {
                    return None;
                }
                if unchanged(&x.values, instance.as_deref(), &prop, &value) {
                    return None;
                }
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
        FromJs::Log { id, level, text } => {
            if allowed(&id, |r| &mut r.log) {
                log::log!(level, "plugin {id} : {text}");
            }
        }
        FromJs::Crash { id, message, import } => {
            log::warn!("plugin {id} : {message}");
            // Déjà compté dans le budget par le fil des scripts.
            if import {
                super::view::set_script_error(&id, Some(message));
            }
        }
        FromJs::Loaded { id } => super::view::set_script_error(&id, None),
        FromJs::Cost { .. } => {}
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
        FromJs::Alert { id, .. } if !allowed(&id, |r| &mut r.alert) => log::debug!("plugin {id} : alerte ignorée (trop fréquente)"),
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
        FromJs::Open { id, .. } if !allowed(&id, |r| &mut r.open) => log::debug!("plugin {id} : lien ignoré (trop fréquent)"),
        FromJs::Open { url, .. } => {
            if let Err(e) = crate::web::url_open(&url) {
                log::warn!("plugin : lien {url} : {e}");
            }
        }
        FromJs::Ssh { id, .. } if !allowed(&id, |r| &mut r.ssh) => log::debug!("plugin {id} : terminal ignoré (trop fréquent)"),
        FromJs::Ssh { id, target } => {
            log::info!("plugin {id} : terminal SSH vers {target}");
            if let Err(e) = super::terminal::open_ssh(&target) {
                log::warn!("plugin {id} : terminal SSH vers {target} : {e}");
            }
        }
        FromJs::SettingsOpen { id, .. } if !allowed(&id, |r| &mut r.open) => log::debug!("plugin {id} : réglages ignorés (trop fréquent)"),
        FromJs::SettingsOpen { id, instance } => {
            // Seulement un exemplaire de ce plugin, et s'il a des réglages.
            let own = LOADED.with_borrow(|l| {
                let list = &l.get(&id)?.instances;
                match &instance {
                    Some(i) => list.iter().find(|x| &x.id == i),
                    None => list.first(),
                }
                .map(|x| x.id.clone())
            });
            let has_settings = app.state::<Plugins>().manifest(&id).is_some_and(|m| !m.settings.is_empty());
            if let Some(instance) = own.filter(|_| has_settings) {
                crate::settings_window::open(app, crate::settings_window::SettingsPage::Widget(instance));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_messages_are_merged() {
        let mut p = Pending::default();
        for i in 0..10 {
            p.push_control(FromJs::Crash { id: "a".into(), message: format!("{i}"), import: false });
            p.push_control(FromJs::Loaded { id: "a".into() });
        }
        p.push_control(FromJs::Crash { id: "b".into(), message: "x".into(), import: false });
        assert_eq!(p.msgs.len(), 3);
        assert!(p.msgs.contains(&FromJs::Crash { id: "a".into(), message: "9".into(), import: false }));
    }

    #[test]
    fn bucket_limits_the_rate() {
        let t0 = Instant::now();
        let mut b = Bucket::new(2.0, 1.0, t0);
        assert!(b.take(t0) && b.take(t0));
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(1100)));
        assert!(!b.take(t0 + Duration::from_millis(1100)));
    }

    #[test]
    fn same_value_again_changes_nothing() {
        let v = |i: Option<&str>, p: &str, x: i64| (i.map(str::to_owned), p.to_owned(), Json::from(x));
        let values = vec![v(None, "a", 1), v(Some("w1"), "b", 2), v(None, "b", 3)];
        assert!(unchanged(&values, None, "a", &Json::from(1)));
        assert!(!unchanged(&values, None, "a", &Json::from(5)));
        assert!(!unchanged(&values, Some("w1"), "a", &Json::from(1)), "un autre public");
        // Posée pour tous après w1 : la reposer pour tous ne change rien.
        assert!(unchanged(&values, None, "b", &Json::from(3)));
        // w1 a reçu 3 depuis ses 2 : remettre 2 change quelque chose.
        assert!(!unchanged(&values, Some("w1"), "b", &Json::from(2)));
        assert!(!unchanged(&values, None, "c", &Json::from(0)));
    }
}
