//! Sonde en lecture seule du widget Serveurs : lit `~/.ssh/config` (sans les clés), puis
//! essaie une connexion TCP vers chaque hôte, en parallèle, comme Horizon. N'affiche ni
//! utilisateur ni chemin de clé.
//!
//! `cargo run -p horizon-core --example servers_probe`

use horizon_core::homelab::{Probe, ServersSettings, parse_ssh_config, targets};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

fn main() {
    let path = std::path::PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE")).join(".ssh").join("config");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => return println!("~/.ssh/config illisible : {e}"),
    };
    let hosts = parse_ssh_config(&text);
    println!("{} hôte(s) dans ~/.ssh/config", hosts.len());
    let list = targets(&hosts, &ServersSettings::default());
    let results: Vec<(String, String, Option<u128>)> = std::thread::scope(|s| {
        let handles: Vec<_> = list
            .iter()
            .map(|t| {
                s.spawn(move || {
                    let Probe::Tcp { host, port } = &t.probe else { unreachable!("test TCP par défaut") };
                    let start = Instant::now();
                    let ok = (host.as_str(), *port).to_socket_addrs().ok().and_then(|mut a| {
                        a.find(|a| TcpStream::connect_timeout(a, Duration::from_millis(1500)).is_ok())
                    });
                    (t.id.clone(), t.probe.label(), ok.map(|_| start.elapsed().as_millis()))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("sonde")).collect()
    });
    for (id, check, r) in results {
        match r {
            Some(ms) => println!("  {id:<20} {check:<8} en ligne ({ms} ms)"),
            None => println!("  {id:<20} {check:<8} injoignable"),
        }
    }
}
