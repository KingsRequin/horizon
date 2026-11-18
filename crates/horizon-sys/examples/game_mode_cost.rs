//! Mesure de ce que le mode jeu économise, poste par poste, dans ce seul processus (sans
//! Horizon ni ses fenêtres) : capture du son et analyse (spectre), mesures système chaque
//! seconde ou toutes les 6 s. Temps CPU du processus sur chaque période.
//!
//! `cargo run --release -p horizon-sys --example game_mode_cost [secondes]`
//! (le son doit jouer pour mesurer l'analyse sur un vrai signal).

use horizon_sys::audio::{AudioFeatures, AudioOptions};
use horizon_sys::system::Sampler;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

/// Temps CPU (noyau + utilisateur) du processus.
fn cpu() -> Duration {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY : pseudo-handle du processus courant, sorties valides.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.expect("GetProcessTimes");
    let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Duration::from_nanos((t(k) + t(u)) * 100)
}

/// Fait tourner `f` pendant `secs` et donne le CPU moyen en % d'un cœur.
fn measure(label: &str, secs: u64, mut f: impl FnMut()) -> f64 {
    let (c0, t0) = (cpu(), Instant::now());
    while t0.elapsed() < Duration::from_secs(secs) {
        f();
    }
    let pct = (cpu() - c0).as_secs_f64() * 100.0 / t0.elapsed().as_secs_f64();
    println!("{label:<42} {pct:>6.2} % d'un cœur");
    pct
}

fn main() -> anyhow::Result<()> {
    let secs: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(20);
    let idle = measure("repos", secs, || std::thread::sleep(Duration::from_millis(50)));

    // Son : capture + analyse à 20 images/s (kick d'Essaim compris), relue par le fil comme `audio-hub`.
    let sink = Arc::new(Mutex::new(AudioFeatures::default()));
    let audio = horizon_sys::audio::start(AudioOptions { bands: 24, rate: 20.0, kick: true }, sink.clone())?;
    let with_audio = measure("son en cours (capture + spectre, 20 Hz)", secs, || {
        std::thread::sleep(Duration::from_millis(50));
        let _ = sink.lock().map(|f| f.clone());
    });
    let playing = !sink.lock().map(|f| f.silent).unwrap_or(true);
    drop(audio);

    let mut sampler = Sampler::new();
    let every_s = measure("mesures système chaque seconde", secs, || {
        std::thread::sleep(Duration::from_secs(1));
        let _ = sampler.sample(false);
    });
    let mut n = 0u64;
    let every_6 = measure("mesures système toutes les 6 s", secs, || {
        std::thread::sleep(Duration::from_secs(1));
        n += 1;
        if n.is_multiple_of(6) {
            let _ = sampler.sample(false);
        }
    });

    println!();
    println!("son {} pendant la mesure", if playing { "présent" } else { "ABSENT (silence : analyse peu sollicitée)" });
    let saved = (with_audio - idle).max(0.0) + (every_s - every_6).max(0.0);
    println!("économie du mode jeu (côté Rust, hors rendu des pages) : ≈ {saved:.2} % d'un cœur");
    Ok(())
}
