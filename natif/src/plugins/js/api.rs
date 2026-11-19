//! Boucle du fil des scripts et objet `horizon` exposé à chaque plugin.
//!
//! Chaque plugin a son runtime QuickJS (limite de mémoire propre) et son contexte. Les fonctions
//! de l'API ne font que noter ce qu'elles produisent ([`Hooks::out`]) : c'est envoyé après le
//! rappel, avec son coût. Les fonctions JS gardées (`ui.on`, `on`, minuteurs) sont des
//! [`Persistent`], vidées avant la destruction du runtime.

use super::{CALL_LIMIT, FromJs, Instance, MEMORY_LIMIT, TIMER_MIN, ToJs, UNLOAD_LIMIT};
use crossbeam_channel::{Receiver, RecvTimeoutError};
use rquickjs::context::EvalOptions;
use rquickjs::function::{Opt, Rest};
use rquickjs::{Context, Ctx, Function, Object, Persistent, Runtime, Value};
use serde_json::Value as Json;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

type Saved = Persistent<Function<'static>>;

/// Minuteur d'un script.
struct Timer {
    id: u32,
    every: Duration,
    next: Instant,
    f: Saved,
}

/// Ce que le script a enregistré, et ce qu'il produit.
#[derive(Default)]
struct Hooks {
    /// Rappels Slint écoutés (`ui.on`).
    ui: HashMap<String, Saved>,
    /// Événements (`on`) : ceux d'Horizon, `unload`, `instances`.
    on: HashMap<String, Vec<Saved>>,
    settings_change: Vec<Saved>,
    timers: Vec<Timer>,
    next_timer: u32,
    instances: Vec<Instance>,
    /// Messages produits pendant le rappel en cours.
    out: Vec<FromJs>,
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
pub(super) fn run(rx: &Receiver<ToJs>, out: &dyn Fn(FromJs)) {
    let mut plugins: HashMap<String, Plugin> = HashMap::new();
    loop {
        let due = plugins.values().filter_map(|p| p.hooks.borrow().timers.iter().map(|t| t.next).min()).min();
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
            && handle(&mut plugins, msg, out)
        {
            out(FromJs::Quit);
            return;
        }
        fire_timers(&plugins, out);
    }
}

/// Traite un message ; vrai pour `Quit`.
fn handle(plugins: &mut HashMap<String, Plugin>, msg: ToJs, out: &dyn Fn(FromJs)) -> bool {
    match msg {
        ToJs::Load { id, main, instances } => {
            if let Some(old) = plugins.remove(&id) {
                old.unload(out);
            }
            match Plugin::new(&id, instances) {
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
        ToJs::Quit => {
            let end = Instant::now() + UNLOAD_LIMIT;
            for (_, p) in plugins.drain() {
                if Instant::now() < end {
                    p.unload(out);
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
                    t.next = now + t.every;
                    t.f.clone()
                })
                .collect()
        };
        for f in due {
            p.invoke(out, |ctx| f.restore(ctx)?.call::<_, ()>(()));
        }
    }
}

impl Plugin {
    fn new(id: &str, instances: Vec<Instance>) -> rquickjs::Result<Plugin> {
        let rt = Runtime::new()?;
        rt.set_memory_limit(MEMORY_LIMIT);
        rt.set_max_stack_size(512 * 1024);
        let deadline: Rc<Cell<Option<Instant>>> = Rc::default();
        let d = deadline.clone();
        rt.set_interrupt_handler(Some(Box::new(move || d.get().is_some_and(|t| Instant::now() > t))));
        let ctx = Context::full(&rt)?;
        let hooks = Rc::new(RefCell::new(Hooks { instances, ..Hooks::default() }));
        ctx.with(|ctx| install(&ctx, id, &hooks))?;
        Ok(Plugin { id: id.to_owned(), hooks, deadline, ctx, rt })
    }

    /// Exécute `f` dans le contexte du plugin, limité dans le temps ; envoie ce qu'il a produit,
    /// son coût, et son plantage éventuel.
    fn invoke(&self, out: &dyn Fn(FromJs), f: impl FnOnce(&Ctx) -> rquickjs::Result<()>) {
        let start = Instant::now();
        self.deadline.set(Some(start + CALL_LIMIT));
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
        let produced = std::mem::take(&mut self.hooks.borrow_mut().out);
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
        let handlers = self.hooks.borrow().on.get(name).cloned().unwrap_or_default();
        for f in handlers {
            self.invoke(out, |ctx| {
                let args: Vec<Value> = args.iter().map(|a| to_js(ctx, a)).collect::<rquickjs::Result<_>>()?;
                f.restore(ctx)?.call::<_, ()>((Rest(args),))
            });
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
        for i in &changed {
            for f in &handlers {
                self.invoke(out, |ctx| f.clone().restore(ctx)?.call::<_, ()>((i.id.clone(), to_js(ctx, &i.settings)?)));
            }
        }
        if ids_changed {
            self.emit("instances", &[Json::Array(ids)], out);
        }
    }

    /// Gestionnaires `unload`, puis le plugin est détruit (`Drop`).
    fn unload(self, out: &dyn Fn(FromJs)) {
        self.emit("unload", &[], out);
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

/// Installe l'objet `horizon` dans le contexte.
fn install<'js>(ctx: &Ctx<'js>, id: &str, hooks: &Rc<RefCell<Hooks>>) -> rquickjs::Result<()> {
    let horizon = Object::new(ctx.clone())?;

    let ui = Object::new(ctx.clone())?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    ui.set(
        "set",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, prop: String, value: Value<'js>, opts: Opt<Object<'js>>| -> rquickjs::Result<()> {
            let instance = opts.0.and_then(|o| o.get::<_, Option<String>>("instance").ok().flatten());
            let value = to_json(&ctx, value);
            h.borrow_mut().out.push(FromJs::UiSet { id: pid.clone(), instance, prop, value });
            Ok(())
        })?,
    )?;
    let (h, pid) = (hooks.clone(), id.to_owned());
    ui.set(
        "on",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String, f: Function<'js>| -> rquickjs::Result<()> {
            let mut hk = h.borrow_mut();
            hk.ui.insert(name.clone(), Persistent::save(&ctx, f));
            hk.out.push(FromJs::UiOn { id: pid.clone(), name });
            Ok(())
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
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, f: Function<'js>| {
            h.borrow_mut().settings_change.push(Persistent::save(&ctx, f));
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
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String, f: Function<'js>| {
            h.borrow_mut().on.entry(name).or_default().push(Persistent::save(&ctx, f));
        })?,
    )?;

    let timer = Object::new(ctx.clone())?;
    let h = hooks.clone();
    timer.set(
        "every",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, ms: f64, f: Function<'js>| -> u32 {
            let every = Duration::from_secs_f64((ms / 1000.0).max(0.0)).max(TIMER_MIN);
            let mut hk = h.borrow_mut();
            hk.next_timer += 1;
            let id = hk.next_timer;
            hk.timers.push(Timer { id, every, next: Instant::now() + every, f: Persistent::save(&ctx, f) });
            id
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
            Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
                let text = log_text(&ctx, args);
                h.borrow_mut().out.push(FromJs::Log { id: pid.clone(), level, text });
            })?,
        )?;
    }
    horizon.set("log", log)?;

    ctx.globals().set("horizon", horizon)?;
    Ok(())
}
