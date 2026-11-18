//! Capture d'écran → session Claude : une nouvelle image dans le dossier des captures est
//! proposée dans l'îlot (miniature, « Envoyer à deskbar ») pour la session qui attend ; le
//! clic ramène son terminal et y colle le chemin (presse-papiers + Ctrl+V). Jamais Entrée.
//! Réglage `[features] capture_to_claude`.
//!
//! Repris de `app/src-tauri/src/widgets/claude_sessions/capture.rs`. Différence : la miniature
//! est préparée ici en pixels, aux deux tailles de l'îlot, coins arrondis (le rendu logiciel de
//! Slint ne découpe pas les coins).

use crate::alerts::{self, Action, Pixels, Severity, Thumb};
use crate::host::AppHandle;
use crate::util::start_thread;
use horizon_core::capture::{NewCaptures, pick_target};
use horizon_core::dnd::AlertKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Relecture du dossier (seulement sa date tant qu'il ne change pas).
const PERIOD: Duration = Duration::from_millis(1500);
/// Durée de la proposition dans l'îlot.
const HOLD_MS: u64 = 15_000;
/// Laisse au terminal le temps de prendre le focus avant Ctrl+V.
const FOCUS_SETTLE: Duration = Duration::from_millis(120);
/// Miniatures dessinées au double de leur taille logique (nettes jusqu'à 200 %) : îlot 40 × 24
/// (coins `--inner-radius`, 8 px), ligne de notification 32 × 20 (coins `--r-sm`, 4 px).
const ISLAND: (u32, u32, f32) = (80, 48, 16.0);
const LINE: (u32, u32, f32) = (64, 40, 8.0);

fn mtime_ms(p: &Path) -> Option<u64> {
    let t = std::fs::metadata(p).and_then(|m| m.modified()).ok()?;
    Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Fichiers du dossier (nom, date de modification).
fn list(dir: &Path) -> Vec<(String, u64)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.filter_map(|e| {
        let e = e.ok()?;
        let t = e.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64;
        Some((e.file_name().to_string_lossy().into_owned(), t))
    })
    .collect()
}

/// Image recadrée pour remplir `w` × `h` (`object-fit: cover`), coins arrondis.
fn cover(img: &image::DynamicImage, (w, h, radius): (u32, u32, f32)) -> Pixels {
    let mut rgba = img.resize_to_fill(w, h, image::imageops::FilterType::Triangle).to_rgba8().into_raw();
    round_corners(&mut rgba, w, h, radius);
    Pixels { width: w, height: h, rgba: Arc::new(rgba) }
}

/// Arrondit les coins d'une image RGBA `w` × `h` (rayon `radius` pixels), bord adouci sur un
/// pixel.
fn round_corners(px: &mut [u8], w: u32, h: u32, radius: f32) {
    let (fw, fh) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            // Distance au bord le plus proche sur chaque axe.
            let (cx, cy) = ((x as f32 + 0.5).min(fw - x as f32 - 0.5), (y as f32 + 0.5).min(fh - y as f32 - 0.5));
            if cx >= radius || cy >= radius {
                continue;
            }
            let d = ((radius - cx).powi(2) + (radius - cy).powi(2)).sqrt();
            let keep = (radius - d + 0.5).clamp(0.0, 1.0);
            let a = &mut px[((y * w + x) * 4 + 3) as usize];
            *a = (*a as f32 * keep).round() as u8;
        }
    }
}

/// Miniatures d'une capture qui vient d'apparaître (l'outil de capture peut encore l'écrire).
fn thumbnail(path: &Path) -> Option<Thumb> {
    for _ in 0..5 {
        if let Ok(img) = image::open(path) {
            return Some(Thumb { island: cover(&img, ISLAND), line: cover(&img, LINE) });
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

fn offer(app: &AppHandle, path: PathBuf) {
    let sessions = super::sessions(app);
    let Some(target) = pick_target(&sessions) else { return };
    let Some(image) = thumbnail(&path) else {
        return log::info!("capture {} illisible : pas proposée", path.display());
    };
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let alert = alerts::Alert::icon("camera", Severity::Info, format!("Envoyer à {}", target.name), name)
        .with_action(Action::Capture { path: path.display().to_string(), session: target.id.clone() })
        .with_image(image, HOLD_MS);
    alerts::show(app, AlertKind::Claude, alert);
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let Some(dir) = horizon_sys::capture::screenshots_dir() else {
        anyhow::bail!("dossier des captures d'écran introuvable");
    };
    let app = app.clone();
    start_thread("captures", move || {
        let mut captures = NewCaptures::default();
        let mut seen_dir: Option<u64> = None;
        loop {
            horizon_sys::presence::wait_while_away();
            // Le dossier change (date) quand un fichier y est créé : liste seulement alors.
            let stamp = mtime_ms(&dir);
            if stamp.is_some() && stamp != seen_dir {
                seen_dir = stamp;
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
                let fresh = captures.scan(&list(&dir), now);
                if crate::state::settings().features.capture_to_claude {
                    for name in fresh {
                        offer(&app, dir.join(name));
                    }
                }
            }
            std::thread::sleep(PERIOD);
        }
    })
}

/// Clic sur la proposition : terminal de la session au premier plan, chemin collé (Ctrl+V),
/// jamais Entrée. Le chemin reste dans le presse-papiers si le terminal ne revient pas. Hors du
/// fil de l'interface (attente du focus) ; un échec est dit dans l'îlot.
pub fn capture_send(app: &AppHandle, path: String, session: String) {
    let app = app.clone();
    let _ = start_thread("capture-envoi", move || {
        if let Err(e) = send(&app, path, session) {
            log::warn!("capture non collée : {e}");
            let alert = alerts::Alert::icon("warning", Severity::Attention, "Capture non collée".into(), e);
            alerts::show(&app, AlertKind::Feedback, alert);
        }
    });
}

/// Texte collé : le chemin, entre guillemets s'il contient une espace.
fn pasted(path: String) -> String {
    if path.contains(' ') { format!("\"{path}\"") } else { path }
}

fn send(app: &AppHandle, path: String, session: String) -> Result<(), String> {
    horizon_sys::capture::set_clipboard_text(&pasted(path)).map_err(|e| format!("presse-papiers indisponible : {e:#}"))?;
    super::claude_focus(app.clone(), session).map_err(|e| format!("{e} : chemin copié, à coller avec Ctrl+V"))?;
    std::thread::sleep(FOCUS_SETTLE);
    if !horizon_sys::capture::paste() {
        return Err("collage refusé par Windows : chemin copié, à coller avec Ctrl+V".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_quoted_only_with_a_space() {
        assert_eq!(pasted(r"C:\Captures\a.png".into()), r"C:\Captures\a.png");
        assert_eq!(pasted(r"C:\Mes captures\a.png".into()), "\"C:\\Mes captures\\a.png\"");
    }

    #[test]
    fn thumbnails_fill_their_box_with_round_corners() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(300, 100, image::Rgba([10, 20, 30, 255])));
        let p = cover(&img, ISLAND);
        assert_eq!((p.width, p.height, p.rgba.len()), (80, 48, 80 * 48 * 4));
        let alpha = |x: u32, y: u32| p.rgba[((y * p.width + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "coin découpé");
        assert_eq!(alpha(40, 24), 255, "centre intact");
        assert_eq!(alpha(40, 0), 255, "bord droit hors des coins");
    }
}
