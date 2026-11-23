//! Sonde du plein écran (lecture seule) : pour chaque écran, les fenêtres qu'Horizon voit
//! au-dessus (ordre Z, celles qu'il ignore comprises) et s'il tient l'écran pour « en plein
//! écran » (`horizon_core::geometry::fullscreen_monitors`, la règle du masquage des barres). Une
//! ligne par changement : sert à voir quelle fenêtre fait revenir une barre à tort.
//!
//! `cargo run -p horizon-sys --example fullscreen_probe -- [secondes] [fenêtres de plus]`
//! (pour chaque écran : tout jusqu'à la première fenêtre non ignorée, celle qui décide, puis
//! quelques-unes de plus)

use horizon_core::geometry::{Rect, fullscreen_monitors};
use std::time::{Duration, Instant};

fn stamp() -> String {
    let ms = horizon_core::time::now_ms() % 86_400_000;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn overlaps(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let depth: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let monitors = horizon_sys::monitors::list();
    let names = horizon_core::geometry::screen_names(&monitors);
    let name = |id: &str| names.iter().find(|(i, _)| i == id).map_or(id, |(_, n)| n.as_str()).to_owned();
    for m in &monitors {
        println!("{} : {:?}", name(&m.id), m.rect);
    }
    let end = Instant::now() + Duration::from_secs(seconds);
    let mut last = Vec::new();
    while Instant::now() < end {
        let windows = horizon_sys::fullscreen::describe_z_order(&[]);
        let infos: Vec<_> = windows.iter().map(|w| w.info).collect();
        let full = fullscreen_monitors(&infos, &monitors);
        let state: Vec<String> = monitors
            .iter()
            .map(|m| {
                // Jusqu'à la fenêtre qui décide (la première non ignorée), puis `depth` de plus.
                let over: Vec<_> = windows.iter().filter(|w| overlaps(w.info.rect, m.rect)).collect();
                let decisive = over.iter().position(|w| !w.info.ignorable).unwrap_or(0);
                let top: Vec<String> = over
                    .iter()
                    .take(decisive + 1 + depth)
                    .map(|w| {
                        let r = w.info.rect;
                        let skip = if w.info.ignorable { " (ignorée)" } else { "" };
                        format!("    [{}] « {} » {},{} {}×{}{skip}", w.class, w.title, r.x, r.y, r.w, r.h)
                    })
                    .collect();
                let verdict = if full.contains(&m.id.as_str()) { "PLEIN ÉCRAN" } else { "pas en plein écran" };
                format!("  {} : {verdict}\n{}", name(&m.id), top.join("\n"))
            })
            .collect();
        if state != last {
            println!("{}\n{}", stamp(), state.join("\n"));
            last = state;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
