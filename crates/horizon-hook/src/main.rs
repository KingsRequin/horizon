//! Hook Claude Code d'Horizon : lit l'événement JSON sur l'entrée standard et le transmet à
//! Horizon (`POST http://127.0.0.1:28660/hook`), complété de la chaîne de ses processus
//! parents (`horizon_procs`) pour qu'Horizon retrouve la fenêtre du terminal.
//!
//! Il ne doit jamais gêner Claude Code : aucune sortie, code de retour toujours 0, délai court,
//! Horizon fermé = rien ne se passe. Pour `PermissionRequest`, c'est essentiel : il observe
//! seulement ; ne rien écrire sur la sortie standard et sortir avec 0 = aucune décision, la
//! demande suit son cours (un autre hook, celui de SharkApp, peut décider). Il est volontairement minuscule (démarrage en quelques
//! millisecondes, il tourne à chaque outil utilisé par Claude). Un ancien Horizon ignore le
//! champ ajouté ; un nouvel Horizon accepte un événement sans ce champ.

use horizon_hook::ADDR;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Sous Windows, une connexion vers un port local fermé est réessayée (~0,6 s) : on abandonne
/// vite pour ne pas ralentir Claude Code quand Horizon est fermé.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const TIMEOUT: Duration = Duration::from_millis(500);
const MAX_INPUT: u64 = 1024 * 1024;
/// Processus gardés au plus (ancêtres et leurs consoles).
const MAX_CHAIN: usize = 16;
/// Ancêtres où la remontée s'arrête : au-delà, plus de terminal mais le bureau ou le système
/// (l'Explorateur a des fenêtres qui ne sont pas le terminal).
const STOP_AT: [&str; 7] =
    ["explorer.exe", "svchost.exe", "services.exe", "wininit.exe", "winlogon.exe", "sihost.exe", "userinit.exe"];
/// Fenêtres de console classiques : appartiennent à un enfant du shell, pas au shell.
const CONSOLE_HOSTS: [&str; 2] = ["conhost.exe", "openconsole.exe"];

/// Processus vu dans l'instantané du système.
#[derive(Debug, Clone, PartialEq)]
struct Proc {
    pid: u32,
    parent: u32,
    name: String,
}

/// Ancêtres de `start` (lui compris), du plus proche au plus lointain, chacun suivi de ses
/// consoles (`conhost.exe`) ; s'arrête au bureau, sur une boucle ou au plafond.
fn host_chain(procs: &[Proc], start: u32) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = Vec::new();
    let mut pid = start;
    while out.len() < MAX_CHAIN {
        let Some(p) = procs.iter().find(|p| p.pid == pid) else { break };
        let lower = p.name.to_ascii_lowercase();
        if pid == 0 || STOP_AT.contains(&lower.as_str()) || out.iter().any(|(q, _)| *q == pid) {
            break;
        }
        out.push((p.pid, p.name.clone()));
        for c in procs.iter().filter(|c| c.parent == pid) {
            if CONSOLE_HOSTS.contains(&c.name.to_ascii_lowercase().as_str()) && out.len() < MAX_CHAIN {
                out.push((c.pid, c.name.clone()));
            }
        }
        pid = p.parent;
    }
    out
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Ajoute `"horizon_procs":[…]` (et `"horizon_window"`) à l'objet JSON reçu, avant son `}`
/// final. Un corps qui n'est pas un objet complet est transmis tel quel.
fn with_procs(body: &[u8], chain: &[(u32, String)], window: Option<isize>) -> Vec<u8> {
    let end = body.iter().rposition(|b| !b.is_ascii_whitespace());
    let Some(end) = end.filter(|&e| body[e] == b'}') else { return body.to_vec() };
    if chain.is_empty() {
        return body.to_vec();
    }
    let empty = body[..end].iter().rposition(|b| !b.is_ascii_whitespace()).is_some_and(|i| body[i] == b'{');
    let items: Vec<String> =
        chain.iter().map(|(pid, name)| format!("{{\"pid\":{pid},\"name\":{}}}", json_string(name))).collect();
    let mut out = body[..end].to_vec();
    if !empty {
        out.push(b',');
    }
    out.extend_from_slice(format!("\"horizon_procs\":[{}]", items.join(",")).as_bytes());
    if let Some(w) = window {
        out.extend_from_slice(format!(",\"horizon_window\":{w}").as_bytes());
    }
    out.push(b'}');
    out
}

/// Les événements d'outils, très fréquents, n'ont pas besoin de la chaîne (Horizon garde la
/// dernière reçue pour la session).
fn wants_chain(body: &[u8]) -> bool {
    let tool = |name: &str| {
        let pat = format!("\"hook_event_name\":\"{name}\"");
        body.windows(pat.len()).any(|w| w == pat.as_bytes())
    };
    !tool("PreToolUse") && !tool("PostToolUse")
}

#[cfg(windows)]
fn snapshot() -> Vec<Proc> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let mut out = Vec::new();
    // SAFETY : instantané fermé avant de sortir ; structure initialisée avec sa taille.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(Proc {
                pid: e.th32ProcessID,
                parent: e.th32ParentProcessID,
                name: String::from_utf16_lossy(&e.szExeFile[..len]),
            });
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(not(windows))]
fn snapshot() -> Vec<Proc> {
    Vec::new()
}

/// Fenêtre visible qui affiche la console du hook : la fenêtre de console classique, ou la
/// fenêtre de Windows Terminal qui possède la pseudo-console de l'onglet.
#[cfg(windows)]
fn console_window() -> Option<isize> {
    use windows::Win32::System::Console::GetConsoleWindow;
    use windows::Win32::UI::WindowsAndMessaging::{GW_OWNER, GetWindow, IsWindowVisible};
    // SAFETY : lectures sans effet de bord ; fenêtre nulle vérifiée.
    unsafe {
        let console = GetConsoleWindow();
        if console.0.is_null() {
            return None;
        }
        let w = GetWindow(console, GW_OWNER).ok().filter(|o| !o.0.is_null()).unwrap_or(console);
        IsWindowVisible(w).as_bool().then_some(w.0 as isize)
    }
}

#[cfg(not(windows))]
fn console_window() -> Option<isize> {
    None
}

fn request(body: &[u8]) -> Vec<u8> {
    let mut r = format!(
        "POST /hook HTTP/1.1\r\nHost: {ADDR}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    r.extend_from_slice(body);
    r
}

/// Adresse d'Horizon ; `HORIZON_HOOK_ADDR` la remplace (tests : ne pas joindre l'Horizon qui
/// tourne).
fn send(body: &[u8]) -> std::io::Result<()> {
    let addr = std::env::var("HORIZON_HOOK_ADDR").unwrap_or_else(|_| ADDR.to_owned());
    send_to(addr.parse().map_err(std::io::Error::other)?, body)
}

fn send_to(addr: SocketAddr, body: &[u8]) -> std::io::Result<()> {
    let mut s = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    s.set_write_timeout(Some(TIMEOUT))?;
    s.set_read_timeout(Some(TIMEOUT))?;
    s.write_all(&request(body))?;
    let mut sink = [0u8; 256];
    let _ = s.read(&mut sink);
    Ok(())
}

fn main() {
    let mut body = Vec::new();
    if std::io::stdin().take(MAX_INPUT).read_to_end(&mut body).is_ok() && !body.is_empty() {
        if wants_chain(&body) {
            let procs = snapshot();
            let me = std::process::id();
            let parent = procs.iter().find(|p| p.pid == me).map(|p| p.parent).unwrap_or(0);
            body = with_procs(&body, &host_chain(&procs, parent), console_window());
        }
        let _ = send(&body);
    }
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, parent: u32, name: &str) -> Proc {
        Proc { pid, parent, name: name.into() }
    }

    #[test]
    fn request_is_well_formed() {
        let r = String::from_utf8(request(b"{\"a\":1}")).unwrap();
        assert!(r.starts_with("POST /hook HTTP/1.1\r\n"));
        assert!(r.contains("Content-Length: 7\r\n"));
        assert!(r.ends_with("\r\n\r\n{\"a\":1}"));
    }

    /// Adresse locale libre : un port pris puis aussitôt rendu (personne n'y écoute).
    fn closed_port() -> SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
    }

    #[test]
    fn absent_server_is_an_error_not_a_hang() {
        // Port fermé : échec (que main ignore), borné par le délai de connexion. La marge est
        // large : une machine chargée ne doit pas faire échouer le test.
        let start = std::time::Instant::now();
        assert!(send_to(closed_port(), b"{}").is_err());
        assert!(start.elapsed() < CONNECT_TIMEOUT + Duration::from_secs(2));
    }

    #[test]
    fn event_reaches_a_listening_server() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let received = std::thread::spawn(move || {
            let (mut c, _) = server.accept().unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 512];
            while !buf.ends_with(b"{\"a\":1}") {
                let n = c.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let _ = c.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
            buf
        });
        send_to(addr, b"{\"a\":1}").unwrap();
        assert_eq!(received.join().unwrap(), request(b"{\"a\":1}"));
    }

    #[test]
    fn chain_goes_up_to_the_terminal_with_consoles() {
        let procs = [
            p(4, 0, "System"),
            p(100, 4, "explorer.exe"),
            p(200, 100, "WindowsTerminal.exe"),
            p(210, 200, "OpenConsole.exe"),
            p(300, 200, "pwsh.exe"),
            p(400, 300, "claude.exe"),
            p(500, 400, "bash.exe"),
            p(600, 500, "horizon-hook.exe"),
        ];
        let names: Vec<String> = host_chain(&procs, 500).into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["bash.exe", "claude.exe", "pwsh.exe", "WindowsTerminal.exe", "OpenConsole.exe"]);
    }

    #[test]
    fn classic_console_window_belongs_to_conhost_child() {
        let procs = [p(100, 1, "explorer.exe"), p(300, 100, "cmd.exe"), p(301, 300, "conhost.exe"), p(400, 300, "claude.exe")];
        assert_eq!(host_chain(&procs, 400), [(400, "claude.exe".into()), (300, "cmd.exe".into()), (301, "conhost.exe".into())]);
    }

    #[test]
    fn chain_survives_cycles_dead_parents_and_is_capped() {
        // Parent mort : la remontée s'arrête.
        assert_eq!(host_chain(&[p(5, 77, "a.exe")], 5).len(), 1);
        // Boucle (PID réutilisé) : pas de boucle infinie.
        assert_eq!(host_chain(&[p(1, 2, "a.exe"), p(2, 1, "b.exe")], 1).len(), 2);
        // Chaîne très longue : plafonnée.
        let long: Vec<Proc> = (1..100).map(|i| p(i, i + 1, "x.exe")).collect();
        assert_eq!(host_chain(&long, 1).len(), MAX_CHAIN);
        assert!(host_chain(&[], 1).is_empty());
    }

    #[test]
    fn procs_are_appended_to_the_json_object() {
        let chain = [(12, "claude.exe".to_string()), (3, "Win\"Term\\.exe".to_string())];
        let out = with_procs(b"{\"session_id\":\"x\"}\n", &chain, None);
        let v = String::from_utf8(out).unwrap();
        assert_eq!(
            v,
            r#"{"session_id":"x","horizon_procs":[{"pid":12,"name":"claude.exe"},{"pid":3,"name":"Win\"Term\\.exe"}]}"#
        );
        assert_eq!(
            with_procs(b"{ }", &chain[..1], Some(4242)),
            br#"{ "horizon_procs":[{"pid":12,"name":"claude.exe"}],"horizon_window":4242}"#
        );
        // Corps tronqué ou vide : transmis tel quel.
        assert_eq!(with_procs(b"{\"a\":", &chain, None), b"{\"a\":");
        assert_eq!(with_procs(b"{\"a\":1}", &[], None), b"{\"a\":1}");
    }

    #[cfg(windows)]
    #[test]
    fn real_snapshot_lists_us_and_starts_the_chain_at_our_parent() {
        let procs = snapshot();
        let me = procs.iter().find(|p| p.pid == std::process::id()).expect("processus de test listé");
        let exe = std::env::current_exe().unwrap();
        assert!(me.name.eq_ignore_ascii_case(&exe.file_name().unwrap().to_string_lossy()), "{me:?}");
        // Quel que soit le lanceur du test (cargo, nextest, éditeur), la chaîne part du parent
        // (vide si le parent est un processus du bureau ou a disparu).
        let chain = host_chain(&procs, me.parent);
        assert!(chain.first().is_none_or(|(pid, _)| *pid == me.parent), "{chain:?}");
        let _ = console_window();
    }

    #[test]
    fn tool_events_skip_the_chain() {
        assert!(!wants_chain(br#"{"hook_event_name":"PostToolUse","tool_name":"Bash"}"#));
        assert!(!wants_chain(br#"{"hook_event_name":"PreToolUse"}"#));
        assert!(wants_chain(br#"{"hook_event_name":"Notification","message":"PostToolUse"}"#));
        // Autorisation : la chaîne sert à retrouver le terminal (clic sur l'îlot).
        assert!(wants_chain(br#"{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#));
    }
}
