//! Affichage des plugins (fil de l'interface) : un exemplaire rendu hors écran par widget de
//! plugin posé sur une barre, et par panneau de plugin montré (fenêtre surgissante ou tuile).
//! Chaque fenêtre a son global `PluginFrames` : images, tailles et erreurs lues par la clé de
//! l'exemplaire (identifiant du widget dans la disposition ; `#panel` en plus pour son panneau),
//! souris et clavier renvoyés à l'exemplaire.
//!
//! Rendu : un minuteur de 50 ms (20 images/s au plus) tant qu'un exemplaire existe ; un
//! exemplaire n'est redessiné que s'il a changé, et pas du tout quand sa barre est cachée. Les
//! plugins rechargés (nouvelle génération du registre) sont recompilés ; s'ils ne compilent
//! plus, l'ancienne interface reste et l'erreur est montrée au survol.

use super::Plugins;
use super::slint_host::{self, Compiled, Pointer, StdProps, Surface};
use crate::host::{AppHandle, Manager};
use crate::ui::{PluginFrames, StyleTokens, Styles};
use slint::{ComponentHandle, Image, Timer, TimerMode};
use slint_interpreter::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Cadence du rendu.
const FRAME: Duration = Duration::from_millis(50);
/// Suffixe de la clé du panneau d'un exemplaire.
pub const PANEL_SUFFIX: &str = "#panel";
/// Taille logique maximale d'un widget et d'un panneau de plugin.
const WIDGET_MAX: (f32, f32) = (1200.0, 200.0);
const PANEL_MAX: (f32, f32) = (900.0, 1200.0);
/// Création ou rendu plus long : compté comme un plantage (le fil de l'interface a gelé).
const SLOW: Duration = Duration::from_millis(250);

/// Taille voulue bornée (largeur, hauteur) d'un exemplaire.
fn bounded(p: (f32, f32), panel: bool) -> (f32, f32) {
    let max = if panel { PANEL_MAX } else { WIDGET_MAX };
    (slint_host::bound(p.0, max.0), slint_host::bound(p.1, max.1))
}

/// Interface compilée d'un plugin, pour une génération du registre.
struct Ui {
    generation: u64,
    compiled: Option<Rc<Compiled>>,
    /// Erreur de compilation (l'interface précédente reste si elle existait).
    error: Option<String>,
}

/// Un exemplaire affiché.
struct View {
    plugin: String,
    panel: bool,
    /// Génération de l'interface dont il est issu.
    generation: u64,
    /// Partagé : les appels à Slint (rendu, souris) se font sans tenir `STATE` (Slint peut
    /// rappeler Horizon pendant ces appels).
    surface: Option<Rc<RefCell<Surface>>>,
    image: Image,
    /// Taille de l'emplacement (px logiques), posée par la fenêtre.
    placed: (f32, f32),
    /// Taille voulue par le composant (px logiques).
    preferred: (f32, f32),
    /// Le widget garde ses clics (propriété `own-clicks` vraie) : pas de panneau épinglé au clic.
    own_clicks: bool,
    /// Échelle de la fenêtre.
    scale: f32,
    /// Premier rendu fait (la sentinelle de plantage le couvre).
    rendered: bool,
}

#[derive(Default)]
struct State {
    app: Option<AppHandle>,
    uis: HashMap<String, Ui>,
    /// (fenêtre, clé) → exemplaire.
    views: HashMap<(String, String), View>,
    timer: Option<Timer>,
    /// Erreur au chargement du script de chaque plugin (montrée au survol).
    script_errors: HashMap<String, String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

/// Fenêtre qui montre des plugins : barre ou hôte de panneaux.
fn frames_do(label: &str, f: impl FnOnce(&PluginFrames<'_>, &Styles<'_>, bool)) {
    if let Some(w) = crate::ui::bar::window(label) {
        f(&w.global::<PluginFrames>(), &w.global::<Styles>(), w.get_graphic());
    } else if let Some(h) = crate::ui::panel_hosts::host(label) {
        f(&h.global::<PluginFrames>(), &h.global::<Styles>(), h.graphic());
    }
}

/// Fait relire les images de la fenêtre `label`.
fn bump(label: &str) {
    frames_do(label, |f, _, _| f.set_tick(f.get_tick().wrapping_add(1)));
}

/// Relie le global `PluginFrames` d'une fenêtre (une fois, à sa création).
pub fn bind(app: &AppHandle, label: &str, f: &PluginFrames<'_>) {
    STATE.with_borrow_mut(|s| {
        if s.app.is_none() {
            s.app = Some(app.clone());
            let a = app.clone();
            crate::host::listen(super::EV_PLUGINS, None, move |_| reload(&a));
        }
    });
    let label = label.to_owned();
    let key = move |k: &slint::SharedString| (label.clone(), k.to_string());
    {
        let key = key.clone();
        f.on_frame(move |k, _| read(&key(&k), |v| v.image.clone()).unwrap_or_default());
    }
    {
        let key = key.clone();
        f.on_width(move |k, _| read(&key(&k), |v| v.preferred.0).unwrap_or(0.0));
    }
    {
        let key = key.clone();
        f.on_height(move |k, _| read(&key(&k), |v| v.preferred.1).unwrap_or(0.0));
    }
    {
        let key = key.clone();
        f.on_error(move |k, _| STATE.with_borrow(|s| error_of(s, &key(&k))).into());
    }
    {
        let key = key.clone();
        f.on_has_panel(move |k, _| read(&key(&k), |v| has_panel(&v.plugin)).unwrap_or(false));
    }
    {
        let key = key.clone();
        f.on_pin_on_click(move |k, _| read(&key(&k), |v| !v.own_clicks).unwrap_or(true));
    }
    {
        let key = key.clone();
        f.on_placed(move |k, w, h| {
            STATE.with_borrow_mut(|s| {
                if let Some(v) = s.views.get_mut(&key(&k)) {
                    v.placed = (w, h);
                }
            });
        });
    }
    {
        let key = key.clone();
        f.on_pointer(move |k, kind, x, y, n| {
            let ev = match kind {
                0 => Pointer::Move(x, y),
                1 => Pointer::Down(x, y),
                2 => Pointer::Up(x, y),
                3 => Pointer::Wheel(x, y, n),
                _ => Pointer::Leave,
            };
            if let Some(surface) = surface_of(&key(&k)) {
                surface.borrow().pointer(ev);
            }
        });
    }
    f.on_key(move |k, text, pressed| {
        if let Some(surface) = surface_of(&key(&k)) {
            surface.borrow().key(&text, pressed);
        }
    });
}

/// Exemplaire rendu de `k` (à utiliser sans tenir `STATE`).
fn surface_of(k: &(String, String)) -> Option<Rc<RefCell<Surface>>> {
    STATE.with_borrow(|s| s.views.get(k).and_then(|v| v.surface.clone()))
}

/// Lit l'exemplaire `k`.
fn read<T>(k: &(String, String), f: impl FnOnce(&View) -> T) -> Option<T> {
    STATE.with_borrow(|s| s.views.get(k).map(f))
}

/// Erreur montrée pour un exemplaire : compilation, lecture du manifeste, création.
fn error_of(s: &State, k: &(String, String)) -> String {
    let Some(v) = s.views.get(k) else { return String::new() };
    if let Some(e) = s.uis.get(&v.plugin).and_then(|u| u.error.clone()) {
        return e;
    }
    if let Some(e) = s.script_errors.get(&v.plugin) {
        return format!("main.js : {e}");
    }
    if v.surface.is_none() {
        return format!("{} : interface indisponible", v.plugin);
    }
    String::new()
}

/// Erreur au chargement du script du plugin (`None` : chargé), montrée au survol.
pub fn set_script_error(plugin: &str, error: Option<String>) {
    let labels: Vec<String> = STATE.with_borrow_mut(|s| {
        match error {
            Some(e) => s.script_errors.insert(plugin.to_owned(), e),
            None => s.script_errors.remove(plugin),
        };
        s.views.iter().filter(|(_, v)| v.plugin == plugin).map(|(k, _)| k.0.clone()).collect()
    });
    for l in labels {
        bump(&l);
    }
}

/// L'interface compilée du plugin (widget ou panneau) a la propriété `prop`.
pub fn has_property(plugin: &str, prop: &str) -> bool {
    let want = prop.replace('_', "-");
    STATE.with_borrow(|s| {
        s.uis.get(plugin).and_then(|u| u.compiled.clone()).is_some_and(|c| {
            [&c.widget, &c.panel].into_iter().flatten().any(|d| d.properties().any(|(n, _)| n.replace('_', "-") == want))
        })
    })
}

/// Plantage compté pour une création ou un rendu trop long.
fn too_slow(plugin: &str, what: &str, took: Duration) {
    if took > SLOW
        && let Some(app) = STATE.with_borrow(|s| s.app.clone())
    {
        let message = format!("{what} trop long ({} ms)", took.as_millis());
        log::warn!("plugin {plugin} : {message}");
        app.state::<Plugins>().script_crashed(&app, plugin, message);
    }
}

fn has_panel(plugin: &str) -> bool {
    crate::plugins::has_panel(&horizon_core::plugin::kind_of(plugin))
}

/// Exemplaires des widgets de plugins de la barre `label` : `(identifiant, plugin)`. Les autres
/// widgets de plugin de cette barre sont retirés.
pub fn sync_bar(label: &str, scale: f32, instances: &[(String, String)]) {
    let keep: Vec<(String, String)> = instances.iter().map(|(id, _)| (label.to_owned(), id.clone())).collect();
    STATE.with_borrow_mut(|s| {
        s.views.retain(|k, v| k.0 != label || v.panel || keep.contains(k));
        for (id, plugin) in instances {
            let k = (label.to_owned(), id.clone());
            s.views.entry(k).or_insert_with(|| new_view(plugin, false, scale));
        }
    });
    refresh(label);
}

/// Le panneau de l'exemplaire `id` (plugin `plugin`) est montré dans la fenêtre `label`.
pub fn panel_shown(label: &str, id: &str, plugin: &str, scale: f32) {
    STATE.with_borrow_mut(|s| {
        s.views.entry((label.to_owned(), format!("{id}{PANEL_SUFFIX}"))).or_insert_with(|| new_view(plugin, true, scale));
    });
    refresh(label);
}

/// Il ne l'est plus.
pub fn panel_hidden(label: &str, id: &str) {
    STATE.with_borrow_mut(|s| s.views.remove(&(label.to_owned(), format!("{id}{PANEL_SUFFIX}"))));
    stop_if_idle();
}

/// La fenêtre `label` se ferme : ses exemplaires disparaissent.
pub fn forget(label: &str) {
    STATE.with_borrow_mut(|s| s.views.retain(|k, _| k.0 != label));
    stop_if_idle();
}

fn new_view(plugin: &str, panel: bool, scale: f32) -> View {
    View {
        plugin: plugin.to_owned(),
        panel,
        generation: 0,
        surface: None,
        image: Image::default(),
        placed: (0.0, 0.0),
        preferred: (0.0, 0.0),
        own_clicks: false,
        scale,
        rendered: false,
    }
}

/// Le registre a changé : interfaces recompilées si leur génération a changé, exemplaires
/// recréés, ceux des plugins devenus indisponibles retirés.
fn reload(app: &AppHandle) {
    let labels: Vec<String> = STATE.with_borrow(|s| s.views.keys().map(|k| k.0.clone()).collect());
    let plugins: Vec<String> = STATE.with_borrow(|s| s.views.values().map(|v| v.plugin.clone()).collect());
    for p in plugins {
        ensure_ui(app, &p);
    }
    for l in labels {
        refresh(&l);
    }
    // Aucun exemplaire affiché : les scripts suivent quand même la disposition.
    super::script::reconcile(app);
}

/// Interface à jour du plugin `id` (recompilée si le registre a une nouvelle génération).
fn ensure_ui(app: &AppHandle, id: &str) -> Option<(u64, Option<Rc<Compiled>>)> {
    let Some((manifest, dir, generation, manifest_error)) = app.state::<Plugins>().loadable(id) else {
        STATE.with_borrow_mut(|s| s.uis.remove(id));
        return None;
    };
    if let Some(u) = STATE.with_borrow(|s| s.uis.get(id).filter(|u| u.generation == generation).map(|u| (u.generation, u.compiled.clone()))) {
        return Some(u);
    }
    let result = slint_host::compile(&dir, manifest.widget.is_some(), manifest.panel);
    let ui = STATE.with_borrow_mut(|s| {
        let previous = s.uis.remove(id).and_then(|u| u.compiled);
        let ui = match result {
            Ok(c) => Ui { generation, compiled: Some(Rc::new(c)), error: manifest_error },
            Err(errors) => {
                let text = errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
                log::warn!("plugin {id} : {text}");
                Ui { generation, compiled: previous, error: Some(text) }
            }
        };
        let out = (ui.generation, ui.compiled.clone());
        s.uis.insert(id.to_owned(), ui);
        out
    });
    Some(ui)
}

/// Crée ce qui manque aux exemplaires de la fenêtre `label`, retire ceux des plugins devenus
/// indisponibles, et lance le rendu.
fn refresh(label: &str) {
    let Some(app) = STATE.with_borrow(|s| s.app.clone()) else { return };
    let keys: Vec<(String, String)> = STATE.with_borrow(|s| s.views.keys().filter(|k| k.0 == label).cloned().collect());
    let mut tokens = None;
    frames_do(label, |_, styles, graphic| tokens = Some(if graphic { styles.get_graphic() } else { styles.get_islands() }));
    for k in keys {
        let Some(plugin) = STATE.with_borrow(|s| s.views.get(&k).map(|v| v.plugin.clone())) else { continue };
        let Some((generation, compiled)) = ensure_ui(&app, &plugin) else {
            STATE.with_borrow_mut(|s| s.views.remove(&k));
            continue;
        };
        let stale = STATE.with_borrow(|s| s.views.get(&k).is_some_and(|v| v.surface.is_none() || v.generation != generation));
        if !stale {
            continue;
        }
        let Some((panel, scale, name)) = STATE.with_borrow(|s| s.views.get(&k).map(|v| (v.panel, v.scale, v.plugin.clone()))) else {
            continue;
        };
        let def = compiled.as_ref().and_then(|c| if panel { c.panel.clone() } else { c.widget.clone() });
        let theme = |inst: &slint_interpreter::ComponentInstance| {
            if let Some(t) = &tokens {
                apply_theme(inst, t);
            }
        };
        // Sentinelle : un plantage pendant la création désactivera ce plugin au démarrage suivant.
        let sentinel = super::sentinel_path();
        super::sentinel_arm(&sentinel, &name);
        let t = std::time::Instant::now();
        let surface = def.and_then(|d| match Surface::new(&d, scale, theme) {
            Ok(surface) => Some(surface),
            Err(e) => {
                log::warn!("plugin {name} : {e:#}");
                None
            }
        });
        super::sentinel_disarm(&sentinel);
        too_slow(&name, "création", t.elapsed());
        let mut own_clicks = false;
        let mut preferred = (0.0, 0.0);
        if let Some(surface) = &surface {
            let id = k.1.trim_end_matches(PANEL_SUFFIX).to_owned();
            surface.set_std(&StdProps { instance: id, ..StdProps::default() });
            own_clicks = matches!(surface.instance().get_property("own-clicks"), Ok(Value::Bool(true)));
            preferred = bounded((surface.preferred_width(), surface.preferred_height()), panel);
        }
        if let Some(surface) = &surface {
            super::script::surface_created(&name, &k.1, surface);
        }
        STATE.with_borrow_mut(|s| {
            if let Some(v) = s.views.get_mut(&k) {
                v.generation = generation;
                v.rendered = false;
                v.surface = surface.map(|x| Rc::new(RefCell::new(x)));
                v.own_clicks = own_clicks;
                v.preferred = preferred;
            }
        });
    }
    super::script::reconcile(&app);
    bump(label);
    start_timer();
}

/// Exemplaires rendus du plugin `plugin` : (clé, surface).
pub(super) fn surfaces_of(plugin: &str) -> Vec<(String, Rc<RefCell<Surface>>)> {
    STATE.with_borrow(|s| {
        s.views.iter().filter(|(_, v)| v.plugin == plugin).filter_map(|(k, v)| Some((k.1.clone(), v.surface.clone()?))).collect()
    })
}

/// Thème d'Horizon (style de la fenêtre) recopié dans le global `Theme` de `@horizon`.
fn apply_theme(inst: &slint_interpreter::ComponentInstance, t: &StyleTokens) {
    let color = |c: slint::Color| Value::Brush(slint::Brush::SolidColor(c));
    let props: [(&str, Value); 19] = [
        ("graphic", Value::Bool(t.graphic)),
        ("font", Value::String(t.font.clone())),
        ("text", color(t.text)),
        ("text-dim", color(t.text_dim)),
        ("accent", color(t.info)),
        ("ok", color(t.ok)),
        ("warn", color(t.warn)),
        ("bad", color(t.bad)),
        ("info", color(t.info)),
        ("island-bg", color(t.island_bg)),
        ("island-border", color(t.island_border)),
        ("popup-bg", color(t.popup_bg)),
        ("hover", color(t.hover)),
        ("press", color(t.press)),
        ("track", color(t.track)),
        ("divider", color(t.divider)),
        ("island-h", Value::Number(f64::from(t.island_h))),
        ("island-radius", Value::Number(f64::from(t.island_radius))),
        ("inner-h", Value::Number(f64::from(t.inner_h))),
    ];
    // Une propriété que le plugin n'utilise pas est retirée par le compilateur : erreur ignorée.
    for (name, v) in props {
        let _ = inst.set_global_property(slint_host::THEME_EXPORT, name, v);
    }
    let _ = inst.set_global_property(slint_host::THEME_EXPORT, "inner-radius", Value::Number(f64::from(t.inner_radius)));
    let _ = inst.set_global_property(slint_host::THEME_EXPORT, "fs-body", Value::Number(f64::from(t.fs_body)));
    let icons = inst.set_global_callback(slint_host::THEME_EXPORT, "icon", |args| {
        let name = match args.first() {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        };
        Value::Image(crate::ui::icons::image(&name))
    });
    if let Err(e) = icons {
        log::warn!("plugin : Theme.icon : {e}");
    }
}

fn start_timer() {
    STATE.with_borrow_mut(|s| {
        if s.timer.is_some() || s.views.is_empty() {
            return;
        }
        let t = Timer::default();
        t.start(TimerMode::Repeated, FRAME, tick);
        s.timer = Some(t);
    });
}

fn stop_if_idle() {
    STATE.with_borrow_mut(|s| {
        if s.views.is_empty() {
            s.timer = None;
        }
    });
}

/// Une image : les exemplaires qui ont changé sont redessinés, leurs fenêtres relisent.
fn tick() {
    struct Job {
        key: (String, String),
        surface: Rc<RefCell<Surface>>,
        panel: bool,
        placed: (f32, f32),
        preferred: (f32, f32),
        plugin: String,
        rendered: bool,
    }
    let jobs: Vec<Job> = STATE.with_borrow(|s| {
        s.views
            .iter()
            .filter_map(|(k, v)| {
                Some(Job {
                    key: k.clone(),
                    surface: v.surface.clone()?,
                    panel: v.panel,
                    placed: v.placed,
                    preferred: v.preferred,
                    plugin: v.plugin.clone(),
                    rendered: v.rendered,
                })
            })
            .collect()
    });
    let mut changed: Vec<String> = Vec::new();
    for job in jobs {
        let label = &job.key.0;
        if !job.panel && crate::ui::bar::window(label).is_some_and(|w| !w.window().is_visible()) {
            continue;
        }
        let mut surface = job.surface.borrow_mut();
        let preferred = bounded((surface.preferred_width(), surface.preferred_height()), job.panel);
        let size = if job.panel {
            (if job.placed.0 > 0.0 { job.placed.0 } else { preferred.0 }, if job.placed.1 > 0.0 { job.placed.1 } else { preferred.1 })
        } else {
            (preferred.0, job.placed.1)
        };
        let max = if job.panel { PANEL_MAX } else { WIDGET_MAX };
        let sentinel = (!job.rendered).then(super::sentinel_path);
        if let Some(f) = &sentinel {
            super::sentinel_arm(f, &job.plugin);
        }
        let t = std::time::Instant::now();
        let image = if size.0 > 0.0 && size.1 > 0.0 { surface.render(size.0, size.1, max) } else { None };
        drop(surface);
        let took = t.elapsed();
        if let Some(f) = &sentinel {
            super::sentinel_disarm(f);
        }
        too_slow(&job.plugin, "rendu", took);
        if image.is_some()
            && let Some(app) = STATE.with_borrow(|s| s.app.clone())
        {
            app.state::<Plugins>().record_cost(&job.plugin, took.as_micros() as u64);
        }
        let dirty = image.is_some() || preferred != job.preferred;
        if image.is_some() && !job.rendered {
            STATE.with_borrow_mut(|s| {
                if let Some(v) = s.views.get_mut(&job.key) {
                    v.rendered = true;
                }
            });
        }
        if dirty {
            STATE.with_borrow_mut(|s| {
                if let Some(v) = s.views.get_mut(&job.key) {
                    v.preferred = preferred;
                    if let Some(img) = image {
                        v.image = img;
                    }
                }
            });
            if !changed.contains(label) {
                changed.push(label.clone());
            }
        }
    }
    for l in changed {
        bump(&l);
    }
}

/// Plugin d'un exemplaire de la disposition (`plugin:<id>` de ce widget).
pub fn plugin_of(app: &AppHandle, instance: &str) -> Option<String> {
    let kind = app.state::<crate::state::AppState>().read(|l| l.widgets().find(|w| w.id == instance).map(|w| w.kind))?;
    horizon_core::plugin::id_of_kind(kind.as_str()).map(str::to_owned)
}
