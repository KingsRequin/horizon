//! Le vrai binaire, sur une demande d'autorisation : rien sur la sortie standard, code 0
//! (aucune décision : la demande suit son cours, un autre hook peut décider).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

const REQUEST: &str = r#"{"session_id":"t","transcript_path":"C:/t.jsonl","cwd":"C:/p","permission_mode":"default","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"npm test"}}"#;

fn run(addr: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_horizon-hook"))
        .env("HORIZON_HOOK_ADDR", addr)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(REQUEST.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn permission_request_is_observed_silently() {
    // Un récepteur qui répond, comme Horizon.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = server.local_addr().unwrap().to_string();
    let got = std::thread::spawn(move || {
        let (mut c, _) = server.accept().unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        while !buf.ends_with(b"}") {
            match c.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        let _ = c.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
        String::from_utf8_lossy(&buf).into_owned()
    });
    let out = run(&addr);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty(), "sortie standard : {:?}", String::from_utf8_lossy(&out.stdout));
    assert!(out.stderr.is_empty());
    assert!(got.join().unwrap().contains(r#""hook_event_name":"PermissionRequest""#));
}

#[test]
fn absent_horizon_changes_nothing() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().to_string();
    let out = run(&port);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}
