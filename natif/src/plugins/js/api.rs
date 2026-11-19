//! Boucle du fil des scripts et objet `horizon` exposé à chaque plugin.
//!
//! Chaque plugin a son runtime QuickJS (limite de mémoire propre) et son contexte. Les fonctions
//! de l'API ne font que noter ce qu'elles produisent ([`Hooks::out`]) : c'est envoyé après le
//! rappel, avec son coût. Les fonctions JS gardées (`ui.on`, `on`, minuteurs) sont des
//! [`Persistent`], vidées avant la destruction du runtime.

use super::io::{self, Conn};
use super::{CALL_LIMIT, FromJs, Instance, Io, MEMORY_LIMIT, TIMER_MIN, ToJs, UNLOAD_LIMIT};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use horizon_core::plugin::Permissions;
use std::path::PathBuf;
use std::time::SystemTime;
use rquickjs::context::EvalOptions;
use rquickjs::function::{Opt, Rest};
use rquickjs::{Context, Ctx, Function, Object, Persistent, Runtime, Value};
use serde_json::Value as Json;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

type Saved = Persistent<Function<'static>>;

/// Gestionnaires gardés au plus (par événement, rappels Slint, `onChange`), minuteurs au plus.
const MAX_HANDLERS: usize = 64;
const MAX_TIMERS: usize = 32;
/// Intervalle maximal d'un minuteur.
const TIMER_MAX: Duration = Duration::from_secs(24 * 3600);
/// Taille maximale d'une valeur de `ui.set` (JSON) et d'un texte de journal.
const MAX_VALUE: usize = 1024 * 1024;
const MAX_LOG: usize = 4096;
/// Connexions et requêtes en cours au plus, fichiers surveillés au plus.
const MAX_CONNS: usize = 8;
const MAX_PENDING: usize = 16;
const MAX_WATCHES: usize = 8;
/// Taille maximale d'un fichier lu.
const MAX_FILE: u64 = 1024 * 1024;
/// Relecture des fichiers surveillés.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// Entrées de palette au plus.
const MAX_ENTRIES: usize = 50;

/// Messages produits au plus par rappel, et leur taille totale.
const MAX_OUT: usize = 1000;
const MAX_OUT_BYTES: usize = 2 * 1024 * 1024;
/// Longueur maximale d'un nom (propriété, rappel, événement, exemplaire).
const MAX_NAME: usize = 256;
/// Gestionnaires d'événements au plus, tous noms confondus.
const MAX_ALL_HANDLERS: usize = 256;

/// Minuteur d'un script.
struct Timer {
    id: u32,
    every: Duration,
    next: Instant,
    f: Saved,
}

/// Connexion d'un script et ses gestionnaires.
struct Link {
    /// Gardée pour sa durée de vie : lâchée, elle ferme la connexion.
    _conn: Conn,
    on_message: Vec<Saved>,
    on_state: Vec<Saved>,
}

/// Fichier surveillé (`files.watch`).
struct Watch {
    path: PathBuf,
    /// Date vue à la dernière lecture (`None` : pas encore lu, ou absent).
    mtime: Option<SystemTime>,
    read_once: bool,
    f: Saved,
}

/// Ce que le script a enregistré, et ce qu'il produit.
#[derive(Default)]
struct Hooks {
    /// Permissions du plugin.
    perms: Permissions,
    /// Retour des fils d'entrées/sorties vers le fil des scripts.
    back: Option<Sender<ToJs>>,
    next_handle: u32,
    links: HashMap<u32, Link>,
    /// Requêtes HTTP en cours : (résoudre, rejeter).
    pending: HashMap<u32, (Saved, Saved)>,
    watches: Vec<Watch>,
    next_watch: Option<Instant>,
    /// Rappels Slint écoutés (`ui.on`).
    ui: HashMap<String, Saved>,
    /// Événements (`on`) : ceux d'Horizon, `unload`, `instances`.
    on: HashMap<String, Vec<Saved>>,
    settings_change: Vec<Saved>,
    timers: Vec<Timer>,
    next_timer: u32,
    instances: Vec<Instance>,
    /// Messages produits pendant le rappel en cours, et leur taille.
    out: Vec<FromJs>,
    out_bytes: usize,
}

/// Un plugin chargé. Ordre des champs : les `Persistent` (dans `hooks`) avant le runtime.
struct Plugin {
    id: String,
    hooks: Rc<RefCell<Hooks>>,
    deadline: Rc<Cell<Option<Instant>>>,
    ctx: Context,
    rt: Runtime,
}

/// Boucle du fil : messages, minuteurs, jusqu'à `ToJs::Quit`.
pub(super) fn run(rx: &Receiver<ToJs>, me: &Sender<ToJs>, out: &dyn Fn(FromJs)) {
    let mut plugins: HashMap<String, Plugin> = HashMap::new();
    loop {
        let due = plugins
            .values()
            .filter_map(|p| {
                let h = p.hooks.borrow();
                h.timers.iter().map(|t| t.next).chain(h.next_watch).min()
            })
            .min();
        let msg = match due {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            },
        };
        if let Some(msg) = msg
            && handle(&mut plugins, msg, me, out)
        {
            out(FromJs::Quit);
            return;
        }
        fire_timers(&plugins, out);
        check_watches(&plugins, out);
    }
}

/// Traite un message ; vrai pour `Quit`.
fn handle(plugins: &mut HashMap<String, Plugin>, msg: ToJs, me: &Sender<ToJs>, out: &dyn Fn(FromJs)) -> bool {
    match msg {
        ToJs::Load { id, main, instances, permissions } => {
            if let Some(old) = plugins.remove(&id) {
                old.unload(out);
            }
            match Plugin::new(&id, instances, permissions, me.clone()) {
                Ok(p) => {
                    p.invoke(out, |ctx| {
                        let mut opts = EvalOptions::default();
                        opts.global = true;
                        opts.strict = true;
                        opts.filename = Some("main.js".to_owned());
                        ctx.eval_with_options::<(), _>(main, opts)
                    });
                    plugins.insert(id, p);
                }
                Err(e) => out(FromJs::Crash { id, message: format!("moteur JavaScript : {e}") }),
            }
        }
        ToJs::Unload(id) => {
            if let Some(p) = plugins.remove(&id) {
                p.unload(out);
            }
        }
        ToJs::Callback { id, instance, name, args } => {
            if let Some(p) = plugins.get(&id) {
                let f = p.hooks.borrow().ui.get(&name).cloned();
                if let Some(f) = f {
                    p.invoke(out, |ctx| {
                        let args: Vec<Value> = args.iter().map(|a| to_js(ctx, a)).collect::<rquickjs::Result<_>>()?;
                        f.restore(ctx)?.call::<_, ()>((instance, Rest(args)))
                    });
                }
            }
        }
        ToJs::Instances { id, list } => {
            if let Some(p) = plugins.get(&id) {
                p.set_instances(list, out);
            }
        }
        ToJs::Event { id, name, payload } => {
            if let Some(p) = plugins.get(&id) {
                p.emit(&name, &[payload], out);
            }
        }
        ToJs::Io { id, handle, io } => {
            if let Some(p) = plugins.get(&id) {
                p.io(handle, io, out);
            }
        }
        ToJs::Quit => {
            let end = Instant::now() + UNLOAD_LIMIT;
            for (_, p) in plugins.drain() {
                if Instant::now() < end {
                    p.unload_until(end, out);
                }
            }
            return true;
        }
    }
    false
}

/// Lance les minuteurs arrivés à échéance.
fn fire_timers(plugins: &HashMap<String, Plugin>, out: &dyn Fn(FromJs)) {
    let now = Instant::now();
    for p in plugins.values() {
        let due: Vec<Saved> = {
            let mut h = p.hooks.borrow_mut();
            h.timers
                .iter_mut()
                .filter(|t| t.next <= now)
                .map(|t| {
                    t.next = now.checked_add(t.every).unwrap_or(now + TIMER_MAX);
                    t.f.clone()
                })
                .collect()
        };
        for f in due {
            p.invoke(out, |ctx| f.restore(ctx)?.call::<_, ()>(()));
        }
    }
}

/// Relit les fichiers surveillés dont la date a changé.
fn check_watches(plugins: &HashMap<String, Plugin>, out: &dyn Fn(FromJs)) {
    let now = Instant::now();
    for p in plugins.values() {
        let changed: Vec<(Saved, Option<String>)> = {
            let mut h = p.hooks.borrow_mut();
            if h.next_watch.is_none_or(|t| t > now) {
                continue;
            }
            h.next_watch = (!h.watches.is_empty()).then(|| now + WATCH_EVERY);
            h.watches
                .iter_mut()
                .filter_map(|w| {
                    let mtime = std::fs::metadata(&w.path).and_then(|m| m.modified()).ok();
                    if w.read_once && mtime == w.mtime {
                        return None;
                    }
                    w.mtime = mtime;
                    w.read_once = true;
                    Some((w.f.clone(), read_small(&w.path)))
                })
                .collect()
        };
        for (f, text) in changed {
            p.invoke(out, |ctx| f.restore(ctx)?.call::<_, ()>((text,)));
        }
    }
}

/// Contenu d'un petit fichier texte (`None` : absent, illisible ou trop gros).
fn read_small(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.len() <= MAX_FILE).then(|| std::fs::read_to_string(path).ok()).flatten()
}

impl Plugin {
    fn new(id: &str, instances: Vec<Instance>, perms: Permissions, back: Sender<ToJs>) -> rquickjs::Result<Plugin> {
        let rt = Runtime::new()?;
        rt.set_memory_limit(MEMORY_LIMIT);
        rt.set_max_stack_size(512 * 1024);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
        let d = deadline.clone();
        rt.set_interrupt_handler(Some(Box::new(move || d.get().is_some_and(|t| Instant::now() > t))));
        let ctx = Context::full(&rt)?;
        let hooks = Rc::new(RefCell::new(Hooks { instances, perms, back: Some(back), ..Hooks::default() }));
        ctx.with(|ctx| install(&ctx, id, &hooks))?;
        Ok(Plugin { id: id.to_owned(), hooks, deadline, ctx, rt })
    }

    /// Exécute `f` dans le contexte du plugin, limité dans le temps ; envoie ce qu'il a produit,
    /// son coût, et son plantage éventuel.
    fn invoke(&self, out: &dyn Fn(FromJs), f: impl FnOnce(&Ctx) -> rquickjs::Result<()>) {
        self.invoke_until(Instant::now() + CALL_LIMIT, out, f);
    }

    /// Comme [`Plugin::invoke`], interrompu à `deadline` (au plus [`CALL_LIMIT`] plus tard).
    fn invoke_until(&self, deadline: Instant, out: &dyn Fn(FromJs), f: impl FnOnce(&Ctx) -> rquickjs::Result<()>) {
        let start = Instant::now();
        self.deadline.set(Some(deadline.min(start + CALL_LIMIT)));
        let mut crash = self.ctx.with(|ctx| f(&ctx).err().map(|e| describe(&ctx, e)));
        // Promesses résolues pendant le rappel.
        while crash.is_none() && self.rt.is_job_pending() {
            match self.rt.execute_pending_job() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => crash = Some(e.0.with(|ctx| describe(&ctx, rquickjs::Error::Exception))),
            }
        }
        self.deadline.set(None);
        let produced = {
            let mut h = self.hooks.borrow_mut();
            h.out_bytes = 0;
            std::mem::take(&mut h.out)
        };
        for m in produced {
            out(m);
        }
        out(FromJs::Cost { id: self.id.clone(), us: start.elapsed().as_micros() as u64 });
        if let Some(message) = crash {
            out(FromJs::Crash { id: self.id.clone(), message });
        }
    }

    /// Appelle les gestionnaires `on(name)` avec `args`.
    fn emit(&self, name: &str, args: &[Json], out: &dyn Fn(FromJs)) {
        self.emit_until(Instant::now() + CALL_LIMIT, name, args, out);
    }

    /// Comme [`Plugin::emit`], tous les gestionnaires ensemble avant `deadline`.
    fn emit_until(&self, deadline: Instant, name: &str, args: &[Json], out: &dyn Fn(FromJs)) {
        let handlers = self.hooks.borrow().on.get(name).cloned().unwrap_or_default();
        for f in handlers {
            if Instant::now() >= deadline {
                out(FromJs::Crash { id: self.id.clone(), message: format!("on(\"{name}\") : temps dépassé") });
                return;
            }
            self.invoke_until(deadline, out, |ctx| {
                let args: Vec<Value> = args.iter().map(|a| to_js(ctx, a)).collect::<rquickjs::Result<_>>()?;
                f.restore(ctx)?.call::<_, ()>((Rest(args),))
            });
        }
    }

    /// Résultat d'une entrée/sortie : gestionnaires de la connexion, promesse de la requête.
    fn io(&self, handle: u32, io: Io, out: &dyn Fn(FromJs)) {
        match io {
            Io::WsOpen | Io::WsClosed => {
                let state = if matches!(io, Io::WsOpen) { "open" } else { "closed" };
                let handlers = self.hooks.borrow().links.get(&handle).map(|l| l.on_state.clone()).unwrap_or_default();
                for f in handlers {
                    self.invoke(out, |ctx| f.restore(ctx)?.call::<_, ()>((state,)));
                }
            }
            Io::WsText(text) => {
                let handlers = self.hooks.borrow().links.get(&handle).map(|l| l.on_message.clone()).unwrap_or_default();
                for f in handlers {
                    self.invoke(out, |ctx| f.restore(ctx)?.call::<_, ()>((text.clone(),)));
                }
            }
            Io::Http(result) => {
                let Some((resolve, reject)) = self.hooks.borrow_mut().pending.remove(&handle) else { return };
                self.invoke(out, |ctx| match result {
                    Ok((status, text)) => {
                        let r = Object::new(ctx.clone())?;
                        r.set("status", status)?;
                        r.set("text", text)?;
                        resolve.restore(ctx)?.call::<_, ()>((r,))
                    }
                    Err(e) => reject.restore(ctx)?.call::<_, ()>((rquickjs::String::from_str(ctx.clone(), &e)?,)),
                });
            }
        }
    }

    /// Nouveaux exemplaires et réglages : `settings.onChange` pour chaque exemplaire changé,
    /// `on("instances")` si la liste a changé.
    fn set_instances(&self, list: Vec<Instance>, out: &dyn Fn(FromJs)) {
        let (changed, ids_changed) = {
            let h = self.hooks.borrow();
            let changed: Vec<Instance> = list.iter().filter(|i| !h.instances.contains(i)).cloned().collect();
            let ids = |l: &[Instance]| l.iter().map(|i| i.id.clone()).collect::<Vec<_>>();
            (changed, ids(&h.instances) != ids(&list))
        };
        let ids: Vec<Json> = list.iter().map(|i| Json::String(i.id.clone())).collect();
        self.hooks.borrow_mut().instances = list;
        let handlers = self.hooks.borrow().settings_change.clone();
        let deadline = Instant::now() + CALL_LIMIT;
        for i in &changed {
            for f in &handlers {
                self.invoke_until(deadline, out, |ctx| f.clone().restore(ctx)?.call::<_, ()>((i.id.clone(), to_js(ctx, &i.settings)?)));
            }
        }
        if ids_changed {
            self.emit("instances", &[Json::Array(ids)], out);
        }
    }

    /// Gestionnaires `unload`, puis le plugin est détruit (`Drop`).
    fn unload(self, out: &dyn Fn(FromJs)) {
        self.unload_until(Instant::now() + CALL_LIMIT, out);
    }

    /// Comme [`Plugin::unload`], avant `deadline` (fermeture d'Horizon : temps commun).
    fn unload_until(self, deadline: Instant, out: &dyn Fn(FromJs)) {
        self.emit_until(deadline, "unload", &[], out);
    }
}

impl Drop for Plugin {
    /// Les fonctions JS gardées sont lâchées avant le runtime : QuickJS refuse (arrêt du
    /// programme) de détruire un runtime qui a encore des objets vivants.
    fn drop(&mut self) {
        let hooks = std::mem::take(&mut *self.hooks.borrow_mut());
        drop(hooks);
        self.rt.run_gc();
    }
}

/// Valeur JS d'une valeur JSON.
fn to_js<'js>(ctx: &Ctx<'js>, v: &Json) -> rquickjs::Result<Value<'js>> {
    ctx.json_parse(v.to_string())
}

/// Valeur JSON d'une valeur JS (`undefined`, fonctions : `null`).
fn to_json<'js>(ctx: &Ctx<'js>, v: Value<'js>) -> Json {
    match ctx.json_stringify(v) {
        Ok(Some(s)) => s.to_string().ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Json::Null),
        _ => Json::Null,
    }
}

/// Texte d'une erreur de script : type, message et place dans `main.js` si QuickJS la donne.
fn describe(ctx: &Ctx, e: rquickjs::Error) -> String {
    if !e.is_exception() {
        return e.to_string();
    }
    let v = ctx.catch();
    let Some(obj) = v.as_object() else {
        return match v.as_string().and_then(|s| s.to_string().ok()) {
            Some(s) => s,
            None => format!("exception : {v:?}"),
        };
    };
    let name: String = obj.get("name").unwrap_or_else(|_| "Error".to_owned());
    let message: String = obj.get("message").unwrap_or_default();
    let stack: String = obj.get("stack").unwrap_or_default();
    let place = stack
        .find("main.js:")
        .map(|at| stack[at..].split([')', '\n']).next().unwrap_or_default().to_owned())
        .map(|p| {
            // « main.js:3:7 » → « main.js:3 » (la colonne n'aide guère).
            let mut parts = p.splitn(3, ':');
            format!("{}:{}", parts.next().unwrap_or_default(), parts.next().unwrap_or_default())
        });
    match place {
        Some(p) => format!("{p} : {name}: {message}"),
        None => format!("{name}: {message}"),
    }
}

/// Texte d'un argument de `horizon.log.*` : chaîne telle quelle, le reste en JSON.
fn log_text<'js>(ctx: &Ctx<'js>, args: Rest<Value<'js>>) -> String {
    args.0
        .into_iter()
        .map(|v| match v.as_string().and_then(|s| s.to_string().ok()) {
            Some(s) => s,
            None => to_json(ctx, v).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Intervalle d'un minuteur : [`TIMER_MIN`] à [`TIMER_MAX`] (valeur non finie : le minimum).
fn timer_interval(ms: f64) -> Duration {
    if !ms.is_finite() {
        return TIMER_MIN;
    }
    Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(TIMER_MAX).clamp(TIMER_MIN, TIMER_MAX)
}

/// Note un message produit par le script (au plus [`MAX_OUT`] par rappel).
fn push(ctx: &Ctx, h: &Rc<RefCell<Hooks>>, m: FromJs) -> rquickjs::Result<()> {
    let size = match &m {
        FromJs::UiSet { prop, instance, value, .. } => {
            if prop.len() > MAX_NAME || instance.as_ref().is_some_and(|i| i.len() > MAX_NAME) {
                return Err(rquickjs::Exception::throw_range(ctx, "nom trop long (256 octets au plus)"));
            }
            prop.len() + instance.as_ref().map_or(0, String::len) + value.to_string().len()
        }
        FromJs::UiOn { name, .. } => name.len(),
        FromJs::Log { text, .. } => text.len(),
        FromJs::Island { spec, .. } => spec.as_ref().map_or(0, |s| s.to_string().len()),
        FromJs::Alert { spec, .. } => spec.to_string().len(),
        FromJs::Palette { entries, .. } => entries.to_string().len(),
        FromJs::Open { url } => url.len(),
        _ => 0,
    };
    let mut hk = h.borrow_mut();
    if hk.out.len() >= MAX_OUT || hk.out_bytes + size > MAX_OUT_BYTES {
        return Err(rquickjs::Exception::throw_range(ctx, "trop de données dans un même rappel"));
    }
    hk.out_bytes += size;
    hk.out.push(m);
    Ok(())
}

/// Refuse un nom trop long.
fn check_name(ctx: &Ctx, name: &str) -> rquickjs::Result<()> {
    if name.len() > MAX_NAME {
        return Err(rquickjs::Exception::throw_range(ctx, "nom trop long (256 octets au plus)"));
    }
    Ok(())
}

/// Erreur JS « permission manquante ».
fn missing<'js>(ctx: &Ctx<'js>, what: &str) -> rquickjs::Error {
    rquickjs::Exception::throw_type(ctx, &format!("permission manquante : {what} (à déclarer dans plugin.json)"))
}

/// Texte d'une valeur à envoyer : chaîne telle quelle, le reste en JSON.
fn text_of<'js>(ctx: &Ctx<'js>, v: Value<'js>) -> String {
    match v.as_string().and_then(|s| s.to_string().ok()) {
        Some(s) => s,
        None => to_json(ctx, v).to_string(),
    }
}

/// Installe l'objet `horizon` dans le contexte.
fn install<'js>(ctx: &Ctx<'js>, id: &str, hooks: &Rc<RefCell<Hooks>>) -> rquickjs::Result<()> {
    let horizon = Object::new(ctx.clone())?;
    install_io(ctx, &horizon, id, hooks)?;
    install_shell(ctx, &horizon, id, hooks)?;

    let ui = Object::new(ctx.clone())?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    ui.set(
        "set",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, prop: String, value: Value<'js>, opts: Opt<Object<'js>>| -> rquickjs::Result<()> {
            let instance = opts.0.and_then(|o| o.get::<_, Option<String>>("instance").ok().flatten());
            let value = to_json(&ctx, value);
            if value.to_string().len() > MAX_VALUE {
                return Err(rquickjs::Exception::throw_range(&ctx, "ui.set : valeur trop grande (1 Mo au plus)"));
            }
            push(&ctx, &h, FromJs::UiSet { id: pid.clone(), instance, prop, value })
        })?,
    )?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    ui.set(
        "on",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String, f: Function<'js>| -> rquickjs::Result<()> {
            check_name(&ctx, &name)?;
            {
                let mut hk = h.borrow_mut();
                if hk.ui.len() >= MAX_HANDLERS && !hk.ui.contains_key(&name) {
                    return Err(rquickjs::Exception::throw_range(&ctx, "ui.on : trop de rappels"));
                }
                hk.ui.insert(name.clone(), Persistent::save(&ctx, f));
            }
            push(&ctx, &h, FromJs::UiOn { id: pid.clone(), name })
        })?,
    )?;
    horizon.set("ui", ui)?;

    let settings = Object::new(ctx.clone())?;
    let h = hooks.clone();
    settings.set(
        "get",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String, instance: Opt<String>| -> rquickjs::Result<Value<'js>> {
            let hk = h.borrow();
            let inst = match &instance.0 {
                Some(id) => hk.instances.iter().find(|i| &i.id == id),
                None => hk.instances.first(),
            };
            match inst.and_then(|i| i.settings.get(&key)) {
                Some(v) => to_js(&ctx, v),
                None => Ok(Value::new_undefined(ctx.clone())),
            }
        })?,
    )?;
    let h = hooks.clone();
    settings.set(
        "onChange",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, f: Function<'js>| -> rquickjs::Result<()> {
            let mut hk = h.borrow_mut();
            if hk.settings_change.len() >= MAX_HANDLERS {
                return Err(rquickjs::Exception::throw_range(&ctx, "settings.onChange : trop de gestionnaires"));
            }
            hk.settings_change.push(Persistent::save(&ctx, f));
            Ok(())
        })?,
    )?;
    horizon.set("settings", settings)?;

    let h = hooks.clone();
    horizon.set(
        "instances",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
            let ids: Vec<Json> = h.borrow().instances.iter().map(|i| Json::String(i.id.clone())).collect();
            to_js(&ctx, &Json::Array(ids))
        })?,
    )?;
    let h = hooks.clone();
    horizon.set(
        "on",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String, f: Function<'js>| -> rquickjs::Result<()> {
            check_name(&ctx, &name)?;
            let mut hk = h.borrow_mut();
            if hk.on.values().map(Vec::len).sum::<usize>() >= MAX_ALL_HANDLERS {
                return Err(rquickjs::Exception::throw_range(&ctx, "on : trop de gestionnaires"));
            }
            let list = hk.on.entry(name).or_default();
            if list.len() >= MAX_HANDLERS {
                return Err(rquickjs::Exception::throw_range(&ctx, "on : trop de gestionnaires"));
            }
            list.push(Persistent::save(&ctx, f));
            Ok(())
        })?,
    )?;

    let timer = Object::new(ctx.clone())?;
    let h = hooks.clone();
    timer.set(
        "every",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, ms: f64, f: Function<'js>| -> rquickjs::Result<u32> {
            let mut hk = h.borrow_mut();
            if hk.timers.len() >= MAX_TIMERS {
                return Err(rquickjs::Exception::throw_range(&ctx, "timer.every : trop de minuteurs"));
            }
            let every = timer_interval(ms);
            hk.next_timer += 1;
            let id = hk.next_timer;
            hk.timers.push(Timer { id, every, next: Instant::now() + every, f: Persistent::save(&ctx, f) });
            Ok(id)
        })?,
    )?;
    let h = hooks.clone();
    timer.set(
        "clear",
        Function::new(ctx.clone(), move |id: u32| {
            h.borrow_mut().timers.retain(|t| t.id != id);
        })?,
    )?;
    horizon.set("timer", timer)?;

    let log = Object::new(ctx.clone())?;
    for (name, level) in [("info", log::Level::Info), ("warn", log::Level::Warn), ("error", log::Level::Error)] {
        let (h, pid) = (hooks.clone(), id.to_owned());
        log.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| -> rquickjs::Result<()> {
                let mut text = log_text(&ctx, args);
                if text.len() > MAX_LOG {
                    let cut = (0..=MAX_LOG).rev().find(|i| text.is_char_boundary(*i)).unwrap_or(0);
                    text.truncate(cut);
                    text.push('…');
                }
                push(&ctx, &h, FromJs::Log { id: pid.clone(), level, text })
            })?,
        )?;
    }
    horizon.set("log", log)?;

    ctx.globals().set("horizon", horizon)?;
    Ok(())
}

/// `horizon.connect`, `horizon.http`, `horizon.files` (sous permissions).
fn install_io<'js>(ctx: &Ctx<'js>, horizon: &Object<'js>, id: &str, hooks: &Rc<RefCell<Hooks>>) -> rquickjs::Result<()> {
    let (h, pid) = (hooks.clone(), id.to_owned());
    horizon.set(
        "connect",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, url: String| -> rquickjs::Result<Object<'js>> {
            let (handle, outbox) = {
                let mut hk = h.borrow_mut();
                let Some(url) = hk.perms.allows_connect(&url) else { return Err(missing(&ctx, &format!("connect {url}"))) };
                if hk.links.len() >= MAX_CONNS {
                    return Err(rquickjs::Exception::throw_range(&ctx, "connect : trop de connexions"));
                }
                hk.next_handle += 1;
                let handle = hk.next_handle;
                let back = hk.back.clone().expect("retour des entrées/sorties");
                let conn = io::connect(&pid, handle, url, back);
                let outbox = conn.outbox.clone();
                hk.links.insert(handle, Link { _conn: conn, on_message: Vec::new(), on_state: Vec::new() });
                (handle, outbox)
            };
            let c = Object::new(ctx.clone())?;
            c.set(
                "send",
                Function::new(ctx.clone(), move |ctx: Ctx<'js>, v: Value<'js>| -> rquickjs::Result<()> {
                    let text = text_of(&ctx, v);
                    if text.len() > io::MAX_MESSAGE {
                        return Err(rquickjs::Exception::throw_range(&ctx, "send : message trop grand (1 Mo au plus)"));
                    }
                    let _ = outbox.send(text);
                    Ok(())
                })?,
            )?;
            for (name, state) in [("onMessage", false), ("onState", true)] {
                let h = h.clone();
                c.set(
                    name,
                    Function::new(ctx.clone(), move |ctx: Ctx<'js>, f: Function<'js>| -> rquickjs::Result<()> {
                        let mut hk = h.borrow_mut();
                        let Some(link) = hk.links.get_mut(&handle) else { return Ok(()) };
                        let list = if state { &mut link.on_state } else { &mut link.on_message };
                        if list.len() >= MAX_HANDLERS {
                            return Err(rquickjs::Exception::throw_range(&ctx, "trop de gestionnaires"));
                        }
                        list.push(Persistent::save(&ctx, f));
                        Ok(())
                    })?,
                )?;
            }
            let h = h.clone();
            c.set(
                "close",
                Function::new(ctx.clone(), move || {
                    h.borrow_mut().links.remove(&handle);
                })?,
            )?;
            Ok(c)
        })?,
    )?;

    let http = Object::new(ctx.clone())?;
    for (name, post) in [("get", false), ("post", true)] {
        let (h, pid) = (hooks.clone(), id.to_owned());
        http.set(
            name,
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, url: String, rest: Rest<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                let mut args = rest.0.into_iter();
                let body = if post { Some(args.next().map(|v| text_of(&ctx, v)).unwrap_or_default()) } else { None };
                let headers: Vec<(String, String)> = match args.next().and_then(|o| o.into_object()) {
                    Some(o) => match o.get::<_, Option<Object>>("headers")? {
                        Some(hd) => hd.props::<String, String>().filter_map(Result::ok).take(32).collect(),
                        None => Vec::new(),
                    },
                    None => Vec::new(),
                };
                let mut hk = h.borrow_mut();
                let Some(url) = hk.perms.allows_http(&url) else { return Err(missing(&ctx, &format!("http {url}"))) };
                if hk.pending.len() >= MAX_PENDING {
                    return Err(rquickjs::Exception::throw_range(&ctx, "http : trop de requêtes en cours"));
                }
                let (promise, resolve, reject) = ctx.promise()?;
                hk.next_handle += 1;
                let handle = hk.next_handle;
                hk.pending.insert(handle, (Persistent::save(&ctx, resolve), Persistent::save(&ctx, reject)));
                let back = hk.back.clone().expect("retour des entrées/sorties");
                io::request(&pid, handle, url, body, headers, back);
                Ok(promise.into_value())
            })?,
        )?;
    }
    horizon.set("http", http)?;

    let files = Object::new(ctx.clone())?;
    let h = hooks.clone();
    files.set(
        "read",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, path: String| -> rquickjs::Result<Option<String>> {
            let p = PathBuf::from(&path);
            if !h.borrow().perms.allows_file(&p, &|v| std::env::var(v).ok()) {
                return Err(missing(&ctx, &format!("files {path}")));
            }
            Ok(read_small(&p))
        })?,
    )?;
    let h = hooks.clone();
    files.set(
        "watch",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, path: String, f: Function<'js>| -> rquickjs::Result<()> {
            let p = PathBuf::from(&path);
            let mut hk = h.borrow_mut();
            if !hk.perms.allows_file(&p, &|v| std::env::var(v).ok()) {
                return Err(missing(&ctx, &format!("files {path}")));
            }
            if hk.watches.len() >= MAX_WATCHES {
                return Err(rquickjs::Exception::throw_range(&ctx, "files.watch : trop de fichiers"));
            }
            hk.watches.push(Watch { path: p, mtime: None, read_once: false, f: Persistent::save(&ctx, f) });
            // Première lecture au prochain tour.
            hk.next_watch = Some(Instant::now());
            Ok(())
        })?,
    )?;
    horizon.set("files", files)?;
    Ok(())
}

/// `horizon.island`, `horizon.alert`, `horizon.palette`, `horizon.open`.
fn install_shell<'js>(ctx: &Ctx<'js>, horizon: &Object<'js>, id: &str, hooks: &Rc<RefCell<Hooks>>) -> rquickjs::Result<()> {
    let island = Object::new(ctx.clone())?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    island.set(
        "show",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, spec: Object<'js>| -> rquickjs::Result<()> {
            let key: String = spec.get::<_, Option<String>>("id")?.unwrap_or_default();
            check_name(&ctx, &key)?;
            let spec = to_json(&ctx, spec.into_value());
            push(&ctx, &h, FromJs::Island { id: pid.clone(), key, spec: Some(spec) })
        })?,
    )?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    island.set(
        "hide",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| -> rquickjs::Result<()> {
            check_name(&ctx, &key)?;
            push(&ctx, &h, FromJs::Island { id: pid.clone(), key, spec: None })
        })?,
    )?;
    horizon.set("island", island)?;

    let (h, pid) = (hooks.clone(), id.to_owned());
    horizon.set(
        "alert",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, spec: Object<'js>| -> rquickjs::Result<()> {
            let spec = to_json(&ctx, spec.into_value());
            push(&ctx, &h, FromJs::Alert { id: pid.clone(), spec })
        })?,
    )?;

    let palette = Object::new(ctx.clone())?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    palette.set(
        "set",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, list: Value<'js>| -> rquickjs::Result<()> {
            let entries = to_json(&ctx, list);
            if entries.as_array().is_none_or(|a| a.len() > MAX_ENTRIES) {
                return Err(rquickjs::Exception::throw_range(&ctx, "palette.set : tableau de 50 entrées au plus"));
            }
            push(&ctx, &h, FromJs::Palette { id: pid.clone(), entries })
        })?,
    )?;
    let h = hooks.clone();
    palette.set(
        "on",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, f: Function<'js>| -> rquickjs::Result<()> {
            let mut hk = h.borrow_mut();
            let list = hk.on.entry("palette".to_owned()).or_default();
            if list.len() >= MAX_HANDLERS {
                return Err(rquickjs::Exception::throw_range(&ctx, "palette.on : trop de gestionnaires"));
            }
            list.push(Persistent::save(&ctx, f));
            Ok(())
        })?,
    )?;
    horizon.set("palette", palette)?;

    let h = hooks.clone();
    horizon.set(
        "open",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, url: String| -> rquickjs::Result<()> {
            let ok = url::Url::parse(&url).ok().filter(|u| u.scheme() == "https" && u.host_str().is_some() && u.username().is_empty() && u.password().is_none());
            let Some(u) = ok else {
                return Err(rquickjs::Exception::throw_type(&ctx, "open : seuls les liens https:// sont permis"));
            };
            push(&ctx, &h, FromJs::Open { url: u.to_string() })
        })?,
    )?;
    Ok(())
}
