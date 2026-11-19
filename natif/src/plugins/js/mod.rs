//! Scripts des plugins (`main.js`) : QuickJS sur un fil à part (`plugins-js`), un moteur
//! (runtime + contexte) par plugin pour que chacun ait sa limite de mémoire. Le script n'a que
//! l'objet `horizon` (`api.rs`) : ni `require`, ni `fetch`, ni `process`.
//!
//! Échanges avec le fil de l'interface : [`ToJs`] entre, [`FromJs`] sort. Chaque rappel est
//! limité à [`CALL_LIMIT`] (interruption de QuickJS) et mesuré ([`FromJs::Cost`]) ; une
//! exception non rattrapée, une interruption ou un manque de mémoire donnent
//! [`FromJs::Crash`] (le budget les compte).

mod api;

use crossbeam_channel::{Sender, unbounded};
use serde_json::Value as Json;
use std::time::Duration;

/// Temps maximal d'un rappel de script.
pub const CALL_LIMIT: Duration = Duration::from_millis(50);
/// Mémoire maximale d'un plugin.
pub const MEMORY_LIMIT: usize = 32 * 1024 * 1024;
/// Intervalle minimal de `horizon.timer.every`.
pub const TIMER_MIN: Duration = Duration::from_millis(250);
/// Temps laissé aux scripts pour leur `unload` à la fermeture d'Horizon (tous ensemble).
pub const UNLOAD_LIMIT: Duration = Duration::from_millis(500);

/// Un exemplaire du plugin et ses réglages.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: String,
    pub settings: Json,
}

/// Vers le fil des scripts.
#[derive(Debug, Clone)]
pub enum ToJs {
    /// Charge (ou recharge) le script `main` du plugin `id`.
    Load { id: String, main: String, instances: Vec<Instance> },
    /// Appelle ses gestionnaires `unload`, puis le retire.
    Unload(String),
    /// Rappel Slint `name` d'un exemplaire (arguments en JSON).
    Callback { id: String, instance: String, name: String, args: Vec<Json> },
    /// Exemplaires et réglages actuels du plugin.
    Instances { id: String, list: Vec<Instance> },
    /// Événement d'Horizon (le plugin l'a déclaré).
    Event { id: String, name: String, payload: Json },
    /// Fermeture d'Horizon : `unload` de tous, puis fin du fil (réponse : `FromJs::Quit`).
    Quit,
}

/// Depuis le fil des scripts.
#[derive(Debug, Clone, PartialEq)]
pub enum FromJs {
    /// `horizon.ui.set` : propriété d'un exemplaire (`None` : tous).
    UiSet { id: String, instance: Option<String>, prop: String, value: Json },
    /// `horizon.ui.on` : le script écoute ce rappel Slint.
    UiOn { id: String, name: String },
    Log { id: String, level: log::Level, text: String },
    /// Exception non rattrapée, interruption, mémoire épuisée (message avec `main.js:<ligne>`).
    Crash { id: String, message: String },
    /// Temps passé dans un rappel (µs).
    Cost { id: String, us: u64 },
    /// Le fil s'est arrêté (après `ToJs::Quit`).
    Quit,
}

/// Lance le fil des scripts ; `out` reçoit ce qu'ils produisent (depuis ce fil).
pub fn spawn(out: impl Fn(FromJs) + Send + 'static) -> anyhow::Result<Sender<ToJs>> {
    let (tx, rx) = unbounded();
    crate::util::start_thread("plugins-js", move || api::run(&rx, &out))?;
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Receiver;
    use serde_json::json;
    use std::time::Instant;

    fn engine() -> (Sender<ToJs>, Receiver<FromJs>) {
        let (tx, rx) = unbounded();
        let s = spawn(move |m| {
            let _ = tx.send(m);
        })
        .unwrap();
        (s, rx)
    }

    fn load(s: &Sender<ToJs>, id: &str, main: &str, instances: Vec<Instance>) {
        s.send(ToJs::Load { id: id.into(), main: main.into(), instances }).unwrap();
    }

    /// Prochain message qui n'est ni une mesure ni un journal.
    fn next(rx: &Receiver<FromJs>) -> FromJs {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match rx.recv_deadline(end).expect("message attendu") {
                FromJs::Cost { .. } | FromJs::Log { .. } => {}
                m => return m,
            }
        }
    }

    fn set(id: &str, prop: &str, value: Json) -> FromJs {
        FromJs::UiSet { id: id.into(), instance: None, prop: prop.into(), value }
    }

    #[test]
    fn ui_set_reaches_ui_channel() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.set("label", "x"); horizon.ui.set("n", { v: [1, 2] });"#, vec![]);
        assert_eq!(next(&rx), set("a", "label", json!("x")));
        assert_eq!(next(&rx), set("a", "n", json!({ "v": [1, 2] })));
    }

    #[test]
    fn callback_reaches_script_with_instance() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.on("clicked", (inst, n) => horizon.ui.set("got", inst + ":" + n, { instance: inst }));"#, vec![]);
        assert_eq!(next(&rx), FromJs::UiOn { id: "a".into(), name: "clicked".into() });
        s.send(ToJs::Callback { id: "a".into(), instance: "w1".into(), name: "clicked".into(), args: vec![json!(5)] }).unwrap();
        assert_eq!(next(&rx), FromJs::UiSet { id: "a".into(), instance: Some("w1".into()), prop: "got".into(), value: json!("w1:5") });
    }

    #[test]
    fn infinite_loop_is_interrupted() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.on("spin", () => { while (true) {} });"#, vec![]);
        load(&s, "b", r#"horizon.ui.on("ping", () => horizon.ui.set("pong", true));"#, vec![]);
        next(&rx);
        next(&rx);
        let t = Instant::now();
        s.send(ToJs::Callback { id: "a".into(), instance: "w".into(), name: "spin".into(), args: vec![] }).unwrap();
        assert!(matches!(next(&rx), FromJs::Crash { ref id, .. } if id == "a"));
        assert!(t.elapsed() < Duration::from_millis(200), "{:?}", t.elapsed());
        s.send(ToJs::Callback { id: "b".into(), instance: "w".into(), name: "ping".into(), args: vec![] }).unwrap();
        assert_eq!(next(&rx), set("b", "pong", json!(true)));
    }

    #[test]
    fn memory_limit() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.on("eat", () => { const a = []; while (true) a.push(new Array(100000).fill(1)); });"#, vec![]);
        next(&rx);
        s.send(ToJs::Callback { id: "a".into(), instance: "w".into(), name: "eat".into(), args: vec![] }).unwrap();
        assert!(matches!(next(&rx), FromJs::Crash { .. }));
    }

    #[test]
    fn throw_at_import_reports_line() {
        let (s, rx) = engine();
        load(&s, "a", "let a = 1;\nlet b = 2;\nthrow new Error('boum');\n", vec![]);
        let FromJs::Crash { message, .. } = next(&rx) else { panic!("plantage attendu") };
        assert!(message.contains("boum") && message.contains("main.js:3"), "{message}");
    }

    #[test]
    fn timer_minimum_250ms() {
        let (s, rx) = engine();
        load(&s, "a", r#"let n = 0; horizon.timer.every(10, () => horizon.ui.set("n", ++n));"#, vec![]);
        let t0 = Instant::now();
        assert_eq!(next(&rx), set("a", "n", json!(1)));
        let t1 = t0.elapsed();
        assert_eq!(next(&rx), set("a", "n", json!(2)));
        let t2 = t0.elapsed();
        assert!(t2 - t1 >= Duration::from_millis(240), "{t1:?} {t2:?}");
    }

    #[test]
    fn no_globals_from_node_or_browser() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.set("t", [typeof require, typeof fetch, typeof process, typeof window].join(","));"#, vec![]);
        assert_eq!(next(&rx), set("a", "t", json!("undefined,undefined,undefined,undefined")));
    }

    #[test]
    fn settings_per_instance() {
        let (s, rx) = engine();
        let inst = |pas: i64| vec![Instance { id: "w1".into(), settings: json!({ "pas": pas }) }];
        load(
            &s,
            "a",
            r#"horizon.ui.set("p", horizon.settings.get("pas", "w1"));
               horizon.ui.set("list", horizon.instances());
               horizon.settings.onChange((inst, values) => horizon.ui.set("c", inst + values.pas));"#,
            inst(3),
        );
        assert_eq!(next(&rx), set("a", "p", json!(3)));
        assert_eq!(next(&rx), set("a", "list", json!(["w1"])));
        s.send(ToJs::Instances { id: "a".into(), list: inst(5) }).unwrap();
        assert_eq!(next(&rx), set("a", "c", json!("w15")));
    }

    #[test]
    fn unload_runs_handlers_then_quit() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.on("unload", () => horizon.log.info("au revoir"));"#, vec![]);
        s.send(ToJs::Quit).unwrap();
        let end = Instant::now() + Duration::from_secs(3);
        let mut said = false;
        loop {
            match rx.recv_deadline(end).expect("fin attendue") {
                FromJs::Log { text, .. } if text == "au revoir" => said = true,
                FromJs::Quit => break,
                _ => {}
            }
        }
        assert!(said);
    }

    #[test]
    fn hostile_values_are_bounded() {
        let (s, rx) = engine();
        load(
            &s,
            "a",
            r#"horizon.timer.every(Infinity, () => {});
               horizon.timer.every(NaN, () => {});
               horizon.timer.every(1e300, () => {});
               let refused = 0;
               try { horizon.ui.set("big", "x".repeat(2 * 1024 * 1024)); } catch (e) { refused++; }
               try { for (let i = 0; i < 100; i++) horizon.on("dnd", () => {}); } catch (e) { refused++; }
               horizon.ui.set("refused", refused);"#,
            vec![],
        );
        assert_eq!(next(&rx), set("a", "refused", json!(2)));
    }

    #[test]
    fn events_reach_handlers() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.on("dnd", (p) => horizon.ui.set("dnd", p.active));"#, vec![]);
        s.send(ToJs::Event { id: "a".into(), name: "dnd".into(), payload: json!({ "active": true }) }).unwrap();
        assert_eq!(next(&rx), set("a", "dnd", json!(true)));
    }
}
