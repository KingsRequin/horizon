//! Scripts des plugins (`main.js`) : QuickJS sur un fil à part (`plugins-js`), un moteur
//! (runtime + contexte) par plugin pour que chacun ait sa limite de mémoire. Le script n'a que
//! l'objet `horizon` (`api.rs`) : ni `require`, ni `fetch`, ni `process`.
//!
//! Échanges avec le fil de l'interface : [`ToJs`] entre, [`FromJs`] sort. Chaque rappel est
//! limité à [`CALL_LIMIT`] (interruption de QuickJS) et mesuré ([`FromJs::Cost`]) ; une
//! exception non rattrapée, une interruption ou un manque de mémoire donnent
//! [`FromJs::Crash`] (le budget les compte).

mod api;
mod io;

use crossbeam_channel::{Sender, unbounded};
use horizon_core::plugin::Permissions;
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
    /// Charge (ou recharge) le script `main` du plugin `id`, avec ses permissions.
    Load { id: String, main: String, instances: Vec<Instance>, permissions: Permissions },
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
    /// Résultat d'une entrée/sortie du script (fils de `io.rs`).
    Io { id: String, handle: u32, io: Io },
}

/// Résultat d'une entrée/sortie.
#[derive(Debug, Clone)]
pub enum Io {
    WsOpen,
    WsClosed,
    WsText(String),
    /// Réponse HTTP (statut, corps) ou erreur.
    Http(Result<(u16, String), String>),
    /// Sonde d'une machine : latence (ms) ou erreur.
    Probe(Result<u32, String>),
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
    /// `import` : au chargement du script (l'ancienne version, s'il y en avait une, reste).
    Crash { id: String, message: String, import: bool },
    /// Le script s'est chargé sans erreur.
    Loaded { id: String },
    /// Temps passé dans un rappel (µs).
    Cost { id: String, us: u64 },
    /// `horizon.island.show` (`Some`) ou `hide` (`None`) : activité durable `key` de l'îlot.
    Island { id: String, key: String, spec: Option<Json> },
    /// `horizon.alert` : alerte passagère de l'îlot.
    Alert { id: String, spec: Json },
    /// `horizon.palette.set` : entrées du plugin dans la palette.
    Palette { id: String, entries: Json },
    /// `horizon.open` : lien vérifié (déclaré, ou quelconque avec l'action `open`).
    Open { id: String, url: String },
    /// `horizon.ssh.open` : terminal SSH vers une cible vérifiée.
    Ssh { id: String, target: String },
    /// `horizon.settings.open` : page de réglages d'un exemplaire (`None` : le premier).
    SettingsOpen { id: String, instance: Option<String> },
    /// Le fil s'est arrêté (après `ToJs::Quit`).
    Quit,
}

/// Erreur de syntaxe de `source` (« main.js:<ligne> : … »), sans l'exécuter.
pub fn syntax_error(source: &str) -> Option<String> {
    let rt = rquickjs::Runtime::new().ok()?;
    rt.set_memory_limit(MEMORY_LIMIT);
    let ctx = rquickjs::Context::full(&rt).ok()?;
    ctx.with(|ctx| match rquickjs::Module::declare(ctx.clone(), "main.js", source) {
        Ok(_) => None,
        Err(e) => Some(api::describe_error(&ctx, e)),
    })
}

/// Lance le fil des scripts ; `out` reçoit ce qu'ils produisent (depuis ce fil).
pub fn spawn(out: impl Fn(FromJs) + Send + 'static) -> anyhow::Result<Sender<ToJs>> {
    let (tx, rx) = unbounded();
    let me = tx.clone();
    crate::util::start_thread("plugins-js", move || api::run(&rx, &me, &out))?;
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
        load_with(s, id, main, instances, Permissions::default());
    }

    fn load_with(s: &Sender<ToJs>, id: &str, main: &str, instances: Vec<Instance>, permissions: Permissions) {
        s.send(ToJs::Load { id: id.into(), main: main.into(), instances, permissions }).unwrap();
    }

    /// Prochain message qui n'est ni une mesure ni un journal.
    fn next(rx: &Receiver<FromJs>) -> FromJs {
        let end = Instant::now() + Duration::from_secs(3);
        loop {
            match rx.recv_deadline(end).expect("message attendu") {
                FromJs::Cost { .. } | FromJs::Log { .. } | FromJs::Loaded { .. } => {}
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
    fn broken_reload_keeps_the_previous_script() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.ui.on("ping", () => horizon.ui.set("pong", 1));"#, vec![]);
        next(&rx);
        load(&s, "a", "horizon.ui.on(\"ping\", () => horizon.ui.set(\"pong\", 2));\nfaute(;\n", vec![]);
        assert!(matches!(next(&rx), FromJs::Crash { import: true, .. }));
        s.send(ToJs::Callback { id: "a".into(), instance: "w".into(), name: "ping".into(), args: vec![] }).unwrap();
        assert_eq!(next(&rx), set("a", "pong", json!(1)));
    }

    #[test]
    fn broken_reload_with_new_permissions_drops_the_old_script() {
        let (s, rx) = engine();
        let wide = Permissions { http: vec!["https://example.com/".into()], ..Permissions::default() };
        load_with(&s, "a", r#"horizon.ui.on("ping", () => horizon.ui.set("pong", 1));"#, vec![], wide);
        next(&rx);
        load_with(&s, "a", "faute(;\n", vec![], Permissions::default());
        assert!(matches!(next(&rx), FromJs::Crash { import: true, .. }));
        s.send(ToJs::Callback { id: "a".into(), instance: "w".into(), name: "ping".into(), args: vec![] }).unwrap();
        // L'ancien script (aux permissions plus larges) ne répond plus.
        assert!(rx.recv_timeout(Duration::from_millis(300)).into_iter().all(|m| !matches!(m, FromJs::UiSet { .. })));
    }

    #[test]
    fn throw_at_import_reports_line() {
        let (s, rx) = engine();
        load(&s, "a", "let a = 1;\nlet b = 2;\nthrow new Error('boum');\n", vec![]);
        let FromJs::Crash { message, import, .. } = next(&rx) else { panic!("plantage attendu") };
        assert!(import);
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
               try { for (let i = 0; i < 1000; i++) horizon.on("e" + i, () => {}); } catch (e) { refused++; }
               try { for (let i = 0; i < 5; i++) horizon.ui.set("v" + i, "x".repeat(900 * 1024)); } catch (e) { refused++; }
               horizon.ui.set("refused", refused);"#,
            vec![],
        );
        // Les deux premières valeurs de 900 Ko passent avant le plafond de 2 Mo.
        assert!(matches!(next(&rx), FromJs::UiSet { ref prop, .. } if prop == "v0"));
        assert!(matches!(next(&rx), FromJs::UiSet { ref prop, .. } if prop == "v1"));
        assert_eq!(next(&rx), set("a", "refused", json!(4)));
    }

    #[test]
    fn io_refused_without_permission() {
        let (s, rx) = engine();
        load(
            &s,
            "a",
            r#"const errs = [];
               try { horizon.connect("ws://127.0.0.1:28650"); } catch (e) { errs.push(e.message); }
               try { horizon.http.get("https://example.com/"); } catch (e) { errs.push(e.message); }
               try { horizon.files.read("C:/Windows/win.ini"); } catch (e) { errs.push(e.message); }
               try { horizon.open("file:///C:/Windows/win.ini"); } catch (e) { errs.push(e.message); }
               horizon.ui.set("errs", errs);"#,
            vec![],
        );
        let FromJs::UiSet { value, .. } = next(&rx) else { panic!() };
        let errs: Vec<String> = serde_json::from_value(value).unwrap();
        assert_eq!(errs.len(), 4, "{errs:?}");
        assert!(errs[..3].iter().all(|e| e.contains("permission manquante")), "{errs:?}");
    }

    #[test]
    fn files_read_declared_file() {
        let dir = std::env::temp_dir().join(format!("horizon-js-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("etat.json");
        std::fs::write(&f, r#"{"batterie":42}"#).unwrap();
        let path = f.to_string_lossy().replace('\\', "/");
        let (s, rx) = engine();
        let perms = Permissions { files: vec![path.clone()], ..Permissions::default() };
        let main = format!(r#"horizon.ui.set("b", JSON.parse(horizon.files.read("{path}")).batterie);"#);
        load_with(&s, "a", &main, vec![], perms);
        assert_eq!(next(&rx), set("a", "b", json!(42)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connect_talks_to_a_local_app() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let (got_tx, got_rx) = unbounded::<String>();
        std::thread::spawn(move || {
            let (stream, _) = server.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            ws.send(tungstenite::Message::text("salut")).unwrap();
            if let Ok(tungstenite::Message::Text(t)) = ws.read() {
                let _ = got_tx.send(t.to_string());
            }
        });
        let (s, rx) = engine();
        let url = format!("ws://127.0.0.1:{port}");
        let perms = Permissions { connect: vec![url.clone()], ..Permissions::default() };
        let main = format!(
            r#"const c = horizon.connect("{url}");
               c.onMessage((m) => {{ horizon.ui.set("got", m); c.send({{ ok: true }}); }});"#
        );
        load_with(&s, "a", &main, vec![], perms);
        assert_eq!(next(&rx), set("a", "got", json!("salut")));
        assert_eq!(got_rx.recv_timeout(Duration::from_secs(3)).unwrap(), r#"{"ok":true}"#);
    }

    #[test]
    fn connect_does_not_follow_redirects() {
        use std::io::{Read, Write};
        // Service déclaré qui redirige vers un autre port (non déclaré).
        let other = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let other_port = other.local_addr().unwrap().port();
        let (reached_tx, reached_rx) = unbounded::<()>();
        std::thread::spawn(move || {
            if let Ok((stream, _)) = other.accept() {
                let _ = reached_tx.send(());
                if let Ok(mut ws) = tungstenite::accept(stream) {
                    let _ = ws.send(tungstenite::Message::text("détourné"));
                }
            }
        });
        let declared = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = declared.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in declared.incoming().take(3).flatten() {
                let mut s = stream;
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let reply = format!("HTTP/1.1 302 Found\r\nLocation: ws://127.0.0.1:{other_port}/\r\nContent-Length: 0\r\n\r\n");
                let _ = s.write_all(reply.as_bytes());
            }
        });
        let (s, rx) = engine();
        let url = format!("ws://127.0.0.1:{port}");
        let perms = Permissions { connect: vec![url.clone()], ..Permissions::default() };
        let main = format!(r#"horizon.connect("{url}").onMessage((m) => horizon.ui.set("got", m));"#);
        load_with(&s, "a", &main, vec![], perms);
        assert!(reached_rx.recv_timeout(Duration::from_millis(1500)).is_err(), "redirection suivie");
        assert!(!matches!(rx.try_recv(), Ok(FromJs::UiSet { .. })));
    }

    #[test]
    fn island_alert_palette_open() {
        let (s, rx) = engine();
        let perms = Permissions { http: vec!["https://example.com/".into()], ..Permissions::default() };
        load_with(
            &s,
            "a",
            r#"horizon.island.show({ id: "appel", icon: "phoneCall", title: "Léo" });
               horizon.island.hide("appel");
               horizon.alert({ title: "Batterie faible", body: "Montre : 15 %" });
               horizon.palette.set([{ id: "off", label: "Éteindre" }]);
               horizon.open("https://example.com/aide");
               let refused = 0;
               try { horizon.open("https://autre.example.org/"); } catch (e) { refused++; }
               try { horizon.http.get("https://example.com/", { headers: { Host: "autre.example.org" } }); } catch (e) { refused++; }
               horizon.ui.set("refused", refused);"#,
            vec![],
            perms,
        );
        assert!(matches!(next(&rx), FromJs::Island { ref key, spec: Some(_), .. } if key == "appel"));
        assert!(matches!(next(&rx), FromJs::Island { ref key, spec: None, .. } if key == "appel"));
        assert!(matches!(next(&rx), FromJs::Alert { .. }));
        assert!(matches!(next(&rx), FromJs::Palette { .. }));
        assert_eq!(next(&rx), FromJs::Open { id: "a".into(), url: "https://example.com/aide".into() });
        assert_eq!(next(&rx), set("a", "refused", json!(2)));
    }

    #[test]
    fn probe_tells_whether_a_machine_answers() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        // Port fermé : celui d'un écouteur qu'on vient de lâcher.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (s, rx) = engine();
        let perms = Permissions { system: vec!["probe".into()], ..Permissions::default() };
        let main = format!(
            r#"horizon.net.probe("127.0.0.1:{port}").then((r) => horizon.ui.set("ouvert", [r.ok, typeof r.ms, r.error]));
               horizon.net.probe("127.0.0.1:{closed}").then((r) => horizon.ui.set("ferme", [r.ok, r.ms, typeof r.error]));
               let refused = 0;
               try {{ horizon.net.probe("pas une cible"); }} catch (e) {{ refused++; }}
               try {{ horizon.net.probe("ftp://127.0.0.1/"); }} catch (e) {{ refused++; }}
               horizon.ui.set("refused", refused);"#
        );
        load_with(&s, "a", &main, vec![], perms);
        assert_eq!(next(&rx), set("a", "refused", json!(2)));
        let mut seen = std::collections::HashMap::new();
        for _ in 0..2 {
            let FromJs::UiSet { prop, value, .. } = next(&rx) else { panic!("résultat de sonde attendu") };
            seen.insert(prop, value);
        }
        assert_eq!(seen["ouvert"], json!([true, "number", null]));
        assert_eq!(seen["ferme"], json!([false, null, "string"]));
        drop(server);
    }

    #[test]
    fn system_actions_need_their_permission() {
        let (s, rx) = engine();
        load(
            &s,
            "a",
            r#"const errs = [];
               try { horizon.net.probe("127.0.0.1:22"); } catch (e) { errs.push(e.message); }
               try { horizon.ssh.open("nas"); } catch (e) { errs.push(e.message); }
               try { horizon.open("http://192.168.1.20:8080/"); } catch (e) { errs.push(e.message); }
               horizon.ui.set("errs", errs);"#,
            vec![],
        );
        let FromJs::UiSet { value, .. } = next(&rx) else { panic!() };
        let errs: Vec<String> = serde_json::from_value(value).unwrap();
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert!(errs.iter().all(|e| e.contains("permission manquante")), "{errs:?}");
    }

    #[test]
    fn ssh_open_and_settings_open() {
        let (s, rx) = engine();
        let perms = Permissions { system: vec!["ssh".into(), "open".into()], ..Permissions::default() };
        load_with(
            &s,
            "a",
            r#"horizon.ssh.open("moi@nas");
               horizon.open("http://192.168.1.20:8080");
               horizon.settings.open("w1");
               horizon.settings.open();
               let refused = 0;
               try { horizon.ssh.open("-oProxyCommand=calc"); } catch (e) { refused++; }
               try { horizon.ssh.open("nas; calc"); } catch (e) { refused++; }
               try { horizon.open("file:///C:/Windows/win.ini"); } catch (e) { refused++; }
               horizon.ui.set("refused", refused);"#,
            vec![],
            perms,
        );
        assert_eq!(next(&rx), FromJs::Ssh { id: "a".into(), target: "moi@nas".into() });
        assert_eq!(next(&rx), FromJs::Open { id: "a".into(), url: "http://192.168.1.20:8080/".into() });
        assert_eq!(next(&rx), FromJs::SettingsOpen { id: "a".into(), instance: Some("w1".into()) });
        assert_eq!(next(&rx), FromJs::SettingsOpen { id: "a".into(), instance: None });
        assert_eq!(next(&rx), set("a", "refused", json!(3)));
    }

    #[test]
    fn headers_allowlist() {
        assert!(io::header_allowed("Authorization") && io::header_allowed("X-Api-Key") && io::header_allowed("accept"));
        assert!(io::header_allowed("anthropic-beta") && io::header_allowed("Anthropic-Version"));
        for h in ["Host", "Connection", "Transfer-Encoding", "Content-Length", "Proxy-Authorization", "Upgrade", "TE", "X-", "X-a b"] {
            assert!(!io::header_allowed(h), "{h}");
        }
    }

    #[test]
    fn palette_choice_reaches_handler() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.palette.on((id) => horizon.ui.set("choisi", id));"#, vec![]);
        s.send(ToJs::Event { id: "a".into(), name: "palette".into(), payload: json!("off") }).unwrap();
        assert_eq!(next(&rx), set("a", "choisi", json!("off")));
    }

    #[test]
    fn events_reach_handlers() {
        let (s, rx) = engine();
        load(&s, "a", r#"horizon.on("dnd", (p) => horizon.ui.set("dnd", p.active));"#, vec![]);
        s.send(ToJs::Event { id: "a".into(), name: "dnd".into(), payload: json!({ "active": true }) }).unwrap();
        assert_eq!(next(&rx), set("a", "dnd", json!(true)));
    }
}
