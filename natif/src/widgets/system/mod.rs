//! Mesures système, prises chaque seconde avec l'historique (60 s) des courbes, et envoyées
//! seulement aux fenêtres qui les affichent :
//!  * barres en style îlots avec un widget Système (courbes) : toutes les 2 s, historique compris ;
//!  * barres en style graphique (jauges) : toutes les 2 s, seulement si une valeur entière
//!    affichée a changé, sans historique ;
//!  * panneau Système ouvert, ou montré en tuile par une barre haute : chaque seconde, avec
//!    l'historique et les processus gourmands (la barre à tuile reçoit tout à ce rythme).
//!    Les processus sont relus chaque seconde pour le panneau surgissant, toutes les
//!    `TILE_TOP_EVERY` s pour une tuile seule (relire tous les processus coûte cher).
//!
//! Rien n'est mesuré pendant une absence (session verrouillée, écran éteint) ; en mode jeu, une
//! mesure toutes les 6 s.
//!
//! Repris de `app/src-tauri/src/widgets/system.rs`. Différences : la charge est une valeur
//! (pas d'emprunt, elle traverse vers le fil de l'interface) ; liaison à la barre (global
//! `SystemData`) ici, au panneau dans `panel`.

mod panel;

use crate::bars::Bars;
use crate::host::{self, AppHandle, Emitter, Manager};
use crate::popup::{LABEL as POPUP, PopupState};
use crate::state::AppState;
use crate::ui::{BarWindow, Spark, SystemData};
use crate::util::{lock, start_thread};
use crate::widgets::Backend;
use horizon_core::layout::{BarLayout, BarStyle, WidgetKind};
use horizon_core::series::Series;
use horizon_sys::system::{Sampler, SystemSample};
use serde::Serialize;
use slint::ComponentHandle;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Mesures système.
pub const EV_SYSTEM: &str = "horizon://system";

pub const BACKEND: Backend = Backend {
    id: "system",
    name: "mesures système",
    manage: |app| {
        app.manage(SystemFeed::default());
    },
    start: Some(start),
    bind: Some(bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

const HISTORY: usize = 60;
/// Les barres reçoivent une mesure sur `BAR_EVERY`.
const BAR_EVERY: u64 = 2;
/// Mode jeu : une mesure toutes les 6 s (multiple de `BAR_EVERY` : les barres la reçoivent ;
/// la mesure couvre tout l'intervalle).
const GAME_EVERY: u64 = 6;
/// Tuile seule (sans panneau surgissant) : processus gourmands relus une mesure sur
/// `TILE_TOP_EVERY` (leur usage est alors une moyenne sur l'intervalle).
const TILE_TOP_EVERY: u64 = 3;
/// Taille d'une courbe (Sparkline.svelte), en px logiques.
const SPARK_W: f32 = 38.0;
const SPARK_H: f32 = 16.0;

/// Historique des courbes (vide pour les jauges).
#[derive(Clone, Default, Serialize)]
pub struct History {
    pub cpu: Vec<f32>,
    pub gpu: Vec<f32>,
    pub ram: Vec<f32>,
}

/// Une mesure, telle qu'envoyée aux barres et au panneau.
#[derive(Clone, Serialize)]
pub struct SystemPayload {
    #[serde(flatten)]
    pub sample: SystemSample,
    pub ram_pct: f32,
    pub history: History,
}

/// Dernière mesure complète (historique compris), pour une barre ou un panneau qui s'ouvre.
#[derive(Default)]
pub struct SystemFeed {
    last: std::sync::Mutex<Option<SystemPayload>>,
}

/// Barres visibles qui affichent le widget Système, par rythme.
#[derive(Default)]
struct Audience {
    /// Style îlots (courbes), toutes les 2 s.
    spark: Vec<String>,
    /// Style graphique (jauges), toutes les 2 s si une valeur a changé.
    gauge: Vec<String>,
    /// Barres hautes qui montrent son panneau en tuile : tout, chaque seconde.
    tiles: Vec<String>,
}

fn visible(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    // SAFETY : simple lecture de l'état d'une fenêtre ; un HWND périmé répond « non visible ».
    unsafe { IsWindowVisible(HWND(hwnd as *mut _)).as_bool() }
}

fn bar_audience(app: &AppHandle) -> Audience {
    // Liste des barres copiée avant de lire la disposition (ordre des verrous, voir `state`).
    let bars: Vec<(String, String, isize)> = {
        let b = app.state::<Bars>();
        let l = lock(&b.list);
        l.iter().map(|b| (b.label.clone(), b.monitor.id.clone(), b.hwnd)).collect()
    };
    // (écran, style, panneau en tuile) des barres qui ont le widget.
    let users: Vec<(String, BarStyle, bool)> = app.state::<AppState>().read(|l| {
        l.bars
            .iter()
            .filter(|b| b.widgets().any(|w| w.kind == WidgetKind::SYSTEM))
            .map(|b| {
                let shown = b.tiles_shown(crate::ui::bar::drawer_open_on(&b.monitor_id));
                (b.monitor_id.clone(), b.style, shown.iter().any(|w| w.kind == WidgetKind::SYSTEM))
            })
            .collect()
    });
    let mut out = Audience::default();
    for (label, monitor, hwnd) in bars {
        let Some((_, style, tile)) = users.iter().find(|(m, _, _)| *m == monitor) else { continue };
        if !visible(hwnd) {
            continue;
        }
        match (tile, style) {
            (true, _) => out.tiles.push(label),
            (false, BarStyle::Islands) => out.spark.push(label),
            (false, BarStyle::Graphic) => out.gauge.push(label),
        }
    }
    out
}

fn send(app: &AppHandle, labels: Vec<String>, payload: SystemPayload) {
    if labels.is_empty() {
        return;
    }
    let _ = app.emit_filter(EV_SYSTEM, payload, move |l| labels.iter().any(|x| x == l));
}

/// Valeurs entières montrées par les jauges.
fn shown(s: &SystemSample, ram_pct: f32) -> (i32, Option<i32>, i32) {
    (s.cpu.round() as i32, s.gpu.map(|g| g.round() as i32), ram_pct.round() as i32)
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("system", move || {
        let mut sampler = Sampler::new();
        let (mut cpu, mut gpu, mut ram) = (Series::new(HISTORY), Series::new(HISTORY), Series::new(HISTORY));
        let mut tick: u64 = 0;
        // Dernières valeurs envoyées aux jauges, et à quelles barres (une barre recréée ou
        // ajoutée les reçoit même si elles n'ont pas changé).
        let mut last_gauges = None;
        let screen_changes = || app.state::<Bars>().screen_changes.load(Ordering::Relaxed);
        let mut seen_screen_changes = screen_changes();
        // Processus gourmands de la dernière relecture, repris entre deux relectures.
        let mut last_top = Vec::new();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            // Session verrouillée ou écran éteint : rien à afficher. La première mesure au
            // retour couvre toute l'absence (moyenne), sans fausse valeur.
            horizon_sys::presence::wait_while_away();
            tick += 1;
            // Écrans modifiés : le pilote graphique a pu redémarrer, l'accès NVML ouvert avant
            // ne doit plus servir (plantage dans nvml.dll sinon).
            let changes = screen_changes();
            if changes != seen_screen_changes {
                seen_screen_changes = changes;
                sampler.reset_gpu("écrans modifiés");
            }
            let popup = app.state::<PopupState>().is_open("system");
            let audience = bar_audience(&app);
            let panel = popup || !audience.tiles.is_empty();
            // Mode jeu : une mesure toutes les 6 s (chaque seconde si le panneau est affiché).
            if crate::game::active() && !panel && !tick.is_multiple_of(GAME_EVERY) {
                continue;
            }
            let top_due = popup
                || (panel && (last_top.is_empty() || tick.is_multiple_of(TILE_TOP_EVERY)));
            let mut s = sampler.sample(top_due);
            if top_due {
                last_top.clone_from(&s.top);
            } else if panel {
                s.top.clone_from(&last_top);
            } else {
                last_top.clear();
            }
            let ram_pct = if s.ram_total > 0 { s.ram_used as f32 * 100.0 / s.ram_total as f32 } else { 0.0 };
            cpu.push(s.cpu);
            gpu.push(s.gpu.unwrap_or(0.0));
            ram.push(ram_pct);
            let full = SystemPayload {
                sample: s.clone(),
                ram_pct,
                history: History { cpu: cpu.values(), gpu: gpu.values(), ram: ram.values() },
            };
            // Gardée pour le panneau ou la barre qui s'ouvre.
            *lock(&app.state::<SystemFeed>().last) = Some(full.clone());
            // Le panneau (surgissant ou en tuile) écoute la fenêtre surgissante et écrit dans
            // chaque fenêtre qui le montre (`ui::panel_hosts`).
            if popup || !audience.tiles.is_empty() {
                send(&app, vec![POPUP.to_owned()], full.clone());
            }
            send(&app, audience.tiles, full.clone());
            if !tick.is_multiple_of(BAR_EVERY) {
                continue;
            }
            send(&app, audience.spark, full);
            let now = (shown(&s, ram_pct), audience.gauge);
            if last_gauges.as_ref() != Some(&now) {
                let light = SystemPayload { sample: s, ram_pct, history: History::default() };
                send(&app, now.1.clone(), light);
                last_gauges = Some(now);
            }
        }
    })
}

/// Tracé SVG d'une courbe (valeurs 0–100, la plus récente à droite, `cap` points sur la
/// largeur) : ligne et zone remplie jusqu'en bas (spark.ts).
pub fn spark_paths(values: &[f32], w: f32, h: f32, cap: usize) -> (String, String) {
    if values.is_empty() {
        return (String::new(), String::new());
    }
    let fmt = |v: f32| {
        let r = (v * 10.0).round() / 10.0;
        if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
    };
    let step = w / (cap.max(2) - 1) as f32;
    let y = |v: f32| h - v.clamp(0.0, 100.0) / 100.0 * h;
    let n = values.len();
    let mut points: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, &v)| (w - (n - 1 - i) as f32 * step, y(v))).collect();
    // Une seule mesure : petit segment plat pour qu'elle reste visible.
    if n == 1 {
        points.insert(0, (w - step, points[0].1));
    }
    let line = points
        .iter()
        .enumerate()
        .map(|(i, (x, py))| format!("{}{},{}", if i > 0 { 'L' } else { 'M' }, fmt(*x), fmt(*py)))
        .collect::<Vec<_>>()
        .join(" ");
    let area = format!("{line} L{},{} L{},{} Z", fmt(w), fmt(h), fmt(points[0].0), fmt(h));
    (line, area)
}

fn spark(values: &[f32]) -> Spark {
    spark_sized(values, SPARK_W, SPARK_H)
}

/// Courbe de `w` × `h` px logiques (barre : 38 × 16 ; panneau : 64 × 18).
fn spark_sized(values: &[f32], w: f32, h: f32) -> Spark {
    let (line, area) = spark_paths(values, w, h, HISTORY);
    Spark { line: line.into(), area: area.into() }
}

fn show(window: &BarWindow, p: &SystemPayload) {
    let g = window.global::<SystemData>();
    g.set_cpu(p.sample.cpu);
    g.set_gpu(p.sample.gpu.unwrap_or(-1.0));
    g.set_ram(p.ram_pct);
    // Jauges (style graphique) : sans historique, les courbes restent telles quelles.
    if !p.history.cpu.is_empty() {
        g.set_cpu_spark(spark(&p.history.cpu));
        g.set_gpu_spark(spark(&p.history.gpu));
        g.set_ram_spark(spark(&p.history.ram));
    }
}

fn bind(app: &AppHandle, window: &BarWindow, label: &str, _bar: &BarLayout) {
    if let Some(p) = lock(&app.state::<SystemFeed>().last).clone() {
        show(window, &p);
    }
    let weak = window.as_weak();
    host::listen(EV_SYSTEM, Some(label), move |e| {
        if let (Some(p), Some(w)) = (e.payload::<SystemPayload>(), weak.upgrade()) {
            show(&w, p);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_value_at_the_right_edge_and_100_at_the_top() {
        assert_eq!(spark_paths(&[0.0, 50.0, 100.0], 40.0, 20.0, 3).0, "M0,20 L20,10 L40,0");
    }

    #[test]
    fn short_history_is_aligned_right() {
        assert_eq!(spark_paths(&[100.0, 0.0], 40.0, 20.0, 3).0, "M20,0 L40,20");
    }

    #[test]
    fn area_is_closed_down_to_the_bottom() {
        assert_eq!(spark_paths(&[0.0, 100.0], 10.0, 10.0, 2).1, "M0,10 L10,0 L10,10 L0,10 Z");
    }

    #[test]
    fn empty_and_single_values() {
        assert_eq!(spark_paths(&[], 40.0, 20.0, 60), (String::new(), String::new()));
        assert_eq!(spark_paths(&[50.0], 40.0, 20.0, 60).0, "M39.3,10 L40,10");
    }
}
