//! Sonde du panneau Audio (lecture seule : ne change ni défaut ni volume) : sorties, micros,
//! défauts, volume général, sessions par appli, et appariement de la lecture en cours.
//!
//! `cargo run -p horizon-sys --example audio_probe`

use horizon_core::mixer::{Device, media_app};
use std::time::Duration;

fn list(title: &str, devices: &[Device], default: Option<&str>) {
    println!("{title} :");
    for d in devices {
        let mark = if Some(d.id.as_str()) == default { "*" } else { " " };
        println!("  {mark} {}  [{}]", d.name, d.id);
    }
}

fn main() -> anyhow::Result<()> {
    let s = horizon_sys::mixer::snapshot_once()?;
    list("Sorties (* = défaut)", &s.outputs, s.default_output.as_deref());
    list("Micros (* = défaut)", &s.inputs, s.default_input.as_deref());
    match s.master {
        Some(m) => println!("Volume général : {:.0} %{}", m.volume * 100.0, if m.muted { " (coupé)" } else { "" }),
        None => println!("Volume général : aucune sortie"),
    }
    println!("Applis ({}) :", s.apps.len());
    for a in &s.apps {
        println!(
            "  {} {:<22} {:>3.0} %{}  {}  aumids={:?}",
            if a.active { "▶" } else { " " },
            a.name,
            a.level.volume * 100.0,
            if a.level.muted { " coupé" } else { "" },
            a.key,
            a.aumids
        );
    }

    // Lecture en cours (SMTC) : premières sessions publiées par le fil média.
    let (tx, rx) = crossbeam_channel::unbounded();
    let _media = horizon_sys::media::start(move |snap| {
        let _ = tx.send(snap);
    })?;
    match rx.recv_timeout(Duration::from_secs(3)) {
        Ok(m) => {
            println!("Lecture en cours (SMTC) :");
            if m.sessions.is_empty() {
                println!("  aucune session");
            }
            for (id, title) in &m.sessions {
                let hit = media_app(id, &s.apps).map_or("— (curseur masqué)".to_owned(), |a| format!("→ {}", a.key));
                println!("  {id}  « {title} »  {hit}");
            }
        }
        Err(_) => println!("Lecture en cours : pas de réponse en 3 s"),
    }
    Ok(())
}
