//! Essai réel de « Ramener une fenêtre ici » sur une fenêtre WinForms de test créée par
//! l'exemple lui-même (jamais une fenêtre de l'utilisateur) : normale, agrandie puis réduite,
//! promenée vers chacun des autres écrans. Deux fenêtres : une appli non consciente de l'échelle
//! (Windows PowerShell) et une appli consciente de l'échelle de chaque écran (pwsh, WinForms
//! en PerMonitorV2).
//!
//! `cargo run -p horizon-sys --example bring_window`

use horizon_core::geometry::MonitorInfo;
use horizon_sys::app_windows::{show_on, list, state};
use std::process::{Child, Command};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{SW_MAXIMIZE, SW_MINIMIZE, ShowWindow};

fn spawn(shell: &str, title: &str, per_monitor: bool) -> Option<Child> {
    let dpi = if per_monitor { "[System.Windows.Forms.Application]::SetHighDpiMode('PerMonitorV2') | Out-Null;" } else { "" };
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; {dpi} $f = New-Object System.Windows.Forms.Form; \
         $f.Text = '{title}'; $f.StartPosition = 'Manual'; $f.Left = 200; $f.Top = 200; \
         $f.Width = 640; $f.Height = 420; [void]$f.ShowDialog()"
    );
    Command::new(shell).args(["-NoProfile", "-NonInteractive", "-Command", &script]).spawn().ok()
}

/// Poignée de la fenêtre de test (titre unique, processus lancé par l'exemple).
fn find(title: &str, monitors: &[MonitorInfo]) -> Option<isize> {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(20) {
        if let Some(w) = list(0, monitors).into_iter().find(|w| w.title == title) {
            return Some(w.hwnd as isize);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

fn on(h: isize, monitors: &[MonitorInfo]) -> String {
    let (r, _, _) = state(h).expect("fenêtre lisible");
    horizon_core::app_windows::monitor_of(r, monitors).map(|m| m.id.clone()).unwrap_or_default()
}

/// Taille logique du cadre (pixels physiques / échelle de son écran).
fn logical(h: isize, monitors: &[MonitorInfo]) -> (f64, f64) {
    let (r, _, _) = state(h).expect("fenêtre lisible");
    let m = horizon_core::app_windows::monitor_of(r, monitors).expect("écran");
    (f64::from(r.w) / f64::from(m.scale), f64::from(r.h) / f64::from(m.scale))
}

fn show(h: isize, cmd: windows::Win32::UI::WindowsAndMessaging::SHOW_WINDOW_CMD) {
    // SAFETY : fenêtre de test de ce processus enfant.
    let _ = unsafe { ShowWindow(HWND(h as *mut _), cmd) };
    std::thread::sleep(Duration::from_millis(400));
}

fn run(label: &str, h: isize, monitors: &[MonitorInfo]) -> usize {
    let mut failures = 0;
    let base = logical(h, monitors);
    println!("\n== {label} : cadre logique de départ {:.0} × {:.0}", base.0, base.1);
    for to in monitors {
        let before = on(h, monitors);
        let r = show_on(h, to, monitors);
        std::thread::sleep(Duration::from_millis(300));
        let (frame, max, min) = state(h).expect("état");
        let now = on(h, monitors);
        let size = logical(h, monitors);
        let fits = f64::from(frame.w) <= f64::from(to.work.w) + 2.0 && f64::from(frame.h) <= f64::from(to.work.h) + 2.0;
        let kept = (size.0 - base.0).abs() <= base.0 * 0.03 + 2.0 && (size.1 - base.1).abs() <= base.1 * 0.03 + 2.0;
        let ok = r.is_ok() && now == to.id && !min && (kept || fits);
        println!(
            "{} {before} → {} : {r:?}, arrivée {now}, cadre {frame:?}, logique {:.0} × {:.0}{}{}",
            if ok { "OK " } else { "ÉCHEC" },
            to.id,
            size.0,
            size.1,
            if max { ", agrandie" } else { "" },
            if kept { "" } else { " (réduite pour tenir)" },
        );
        failures += usize::from(!ok);
    }
    failures
}

fn main() {
    // Comme Horizon (manifeste) : pixels physiques partout, écrans compris.
    // SAFETY : réglage du processus, avant toute fenêtre.
    let _ = unsafe {
        windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        )
    };
    let monitors = horizon_sys::monitors::list();
    for m in &monitors {
        println!("écran {} : {:?} zone {:?} échelle {}{}", m.id, m.rect, m.work, m.scale, if m.primary { " (principal)" } else { "" });
    }
    let mut failures = 0;
    for (shell, per_monitor) in [("powershell", false), ("pwsh", true)] {
        let title = format!("Horizon essai fenetre {} {}", std::process::id(), if per_monitor { "pmv2" } else { "unaware" });
        let Some(mut child) = spawn(shell, &title, per_monitor) else {
            println!("{shell} absent : essai sauté");
            continue;
        };
        match find(&title, &monitors) {
            Some(h) => {
                failures += run(&format!("{shell} normale"), h, &monitors);
                show(h, SW_MAXIMIZE);
                let n = run(&format!("{shell} agrandie"), h, &monitors);
                let still = state(h).is_some_and(|(_, max, _)| max);
                println!("toujours agrandie : {still}");
                failures += n + usize::from(!still);
                for to in &monitors {
                    show(h, SW_MINIMIZE);
                    let r = show_on(h, to, &monitors);
                    std::thread::sleep(Duration::from_millis(300));
                    let (_, _, min) = state(h).expect("état");
                    let ok = r.is_ok() && !min && on(h, &monitors) == to.id;
                    println!("{} réduite → {} : {r:?}", if ok { "OK " } else { "ÉCHEC" }, to.id);
                    failures += usize::from(!ok);
                }
            }
            None => {
                println!("fenêtre de test introuvable ({shell})");
                failures += 1;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    println!("\n{failures} échec(s)");
    std::process::exit(i32::from(failures > 0));
}
