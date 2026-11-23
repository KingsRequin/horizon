//! `faux-telephone "<lien du QR>" <pourcentage> [--charge] [--boucle <secondes>]`
//!
//! Appaire un faux téléphone avec Horizon (ou rouvre sa session s'il l'est déjà), envoie une
//! batterie, puis, avec `--boucle`, la renvoie en la faisant baisser d'un point à chaque tour
//! et affiche ce que le PC envoie. Son jeton est gardé dans `%TEMP%\faux-telephone.json`.

use anyhow::{Context, bail};
use faux_telephone::{Client, parse_link};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

fn state_path() -> PathBuf {
    std::env::temp_dir().join("faux-telephone.json")
}

/// Session rouverte avec le jeton gardé, si celui-ci vaut pour ce PC (même empreinte).
fn resume(fingerprint: &str) -> Option<Client> {
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(state_path()).ok()?).ok()?;
    if saved["fingerprint"].as_str()? != fingerprint {
        return None;
    }
    let addr = saved["addr"].as_str()?.parse().ok()?;
    match Client::hello(addr, fingerprint, saved["deviceId"].as_str()?, saved["deviceToken"].as_str()?) {
        Ok(client) => Some(client),
        Err(e) => {
            eprintln!("session gardée refusée ({e}) : nouvel appairage");
            None
        }
    }
}

fn open(link: &str) -> anyhow::Result<Client> {
    let fingerprint = parse_link(link)?.fingerprint;
    if let Some(client) = resume(&fingerprint) {
        println!("session rouverte avec {}", client.addr);
        return Ok(client);
    }
    let client = Client::pair(link, "Faux téléphone")?;
    let saved = json!({ "fingerprint": fingerprint, "addr": client.addr.to_string(), "deviceId": client.device_id, "deviceToken": client.device_token });
    std::fs::write(state_path(), saved.to_string()).context("jeton non enregistré")?;
    println!("appairé avec {}", client.addr);
    Ok(client)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(link), Some(percent)) = (args.first(), args.get(1).and_then(|p| p.parse::<u8>().ok())) else {
        bail!("usage : faux-telephone \"<lien du QR>\" <pourcentage> [--charge] [--boucle <secondes>]");
    };
    let charging = args.iter().any(|a| a == "--charge");
    let every = args.iter().position(|a| a == "--boucle").and_then(|i| args.get(i + 1)).and_then(|s| s.parse::<u64>().ok());
    let mut client = open(link)?;
    client.send_battery(percent, charging)?;
    println!("batterie envoyée : {percent} %{}", if charging { ", en charge" } else { "" });
    let Some(every) = every else { return Ok(()) };
    let mut level = percent;
    loop {
        // `receive` rend la main au bout de 5 s sans message : on en profite pour compter le temps.
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(every) {
            match client.receive() {
                Ok(text) => println!("reçu : {text}"),
                Err(e) if e.to_string().contains("fermée") => {
                    println!("coupure : {e} ; nouvel essai dans 2 s");
                    std::thread::sleep(Duration::from_secs(2));
                    match open(link) {
                        Ok(c) => client = c,
                        Err(e) => println!("toujours coupé : {e}"),
                    }
                }
                Err(_) => {}
            }
        }
        level = level.saturating_sub(1);
        match client.send_battery(level, charging) {
            Ok(()) => println!("batterie envoyée : {level} %"),
            Err(e) => println!("envoi impossible : {e}"),
        }
    }
}
