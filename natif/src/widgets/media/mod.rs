//! Lecture en cours : diffusion de l'état quand il change de façon visible (la position est
//! extrapolée par les barres ; elle n'est renvoyée que si elle dérive), de la pochette quand
//! elle change ; commandes. Liaison aux barres : `ui.rs` ; son en cours : `audio.rs`.
//!
//! Panneau surgissant : `panel.rs`.
//!
//! Repris de `app/src-tauri/src/widgets/media/mod.rs`. Différences : la pochette part en pixels
//! déjà réduits (`Art` : une petite pour les barres, une plus grande aux coins arrondis pour le
//! panneau), pas en URL `data:` ; la lecture par la palette et par l'îlot en phases D et F.

pub mod audio;
mod panel;
mod ui;

use crate::host::{AppHandle, Emitter, Manager};
use crate::util::lock;
use crate::widgets::Backend;
use horizon_core::media::{MediaInfo, collapsed, is_new_track, position_at, worth_sending};
use horizon_sys::media::{MediaCommand, MediaHandle, MediaSnapshot};
use serde::Serialize;
use std::sync::{Arc, Mutex};

/// Lecture en cours.
pub const EV_MEDIA: &str = "horizon://media";
/// Pochette de la lecture en cours (`Option<Art>`).
pub const EV_MEDIA_ART: &str = "horizon://media-art";

/// Pas d'avance ou de recul dans la lecture : le même pour la molette et les boutons.
pub const SEEK_MS: i64 = 10_000;

/// Côté des pochettes envoyées aux barres, en pixels (affichées en 28 px logiques au plus, à
/// 150 % sur l'écran 4K : 42 px ; marge pour les écrans plus denses).
const ART_SIZE: u32 = 64;
/// Côté de la pochette du panneau (84 px logiques, jusqu'à 200 %).
const PANEL_ART_SIZE: u32 = 168;
/// Rayon de ses coins (`--r-2xl` = 12 px sur 84), en pixels de l'image.
const PANEL_ART_RADIUS: f32 = 24.0;

pub const BACKEND: Backend = Backend {
    id: "media",
    name: "lecture en cours",
    manage: |app| {
        app.manage(MediaState::default());
        app.manage(audio::AudioHub::default());
    },
    start: Some(|app| crate::widgets::start_each(app, &[("lecture", start), ("son en cours", audio::start)])),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

#[derive(Clone, Serialize, Default)]
pub struct MediaPayload {
    pub info: Option<MediaInfo>,
    /// Position extrapolée au moment de l'envoi.
    pub position_ms: u64,
    pub collapsed: bool,
    pub sessions: Vec<(String, String)>,
}

/// Pochette réduite en carré de `ART_SIZE` (RGBA, rognée au centre comme `object-fit: cover`),
/// et celle du panneau (`PANEL_ART_SIZE`, coins arrondis : le rendu logiciel de Slint ne découpe
/// pas les coins).
#[derive(Clone, Serialize)]
pub struct Art {
    pub size: u32,
    #[serde(skip)]
    pub rgba: Arc<Vec<u8>>,
    pub panel_size: u32,
    #[serde(skip)]
    pub panel: Arc<Vec<u8>>,
}

#[derive(Default)]
pub struct MediaState {
    handle: Mutex<Option<MediaHandle>>,
    /// Dernier état, pochette comprise (un seul verrou : toujours cohérents).
    current: Mutex<MediaFull>,
    /// Dernier état envoyé aux barres (lecture, sessions) ; pris après `current`.
    sent: Mutex<Option<SentMedia>>,
}

type SentMedia = (Option<MediaInfo>, Vec<(String, String)>);

/// Pochette décodée et réduite, et sa couleur d'accent lisible (aucune pour le logo qu'un
/// navigateur publie à la place d'une pochette : il est montré, sans teinter l'accent) ; `None`
/// si illisible.
fn art_pixels(bytes: &[u8]) -> Option<(Art, Option<[u8; 3]>)> {
    let (w, h, px) = horizon_sys::art::decode_rgba(bytes)?;
    let accent = horizon_core::accent::cover_accent(&px, w, h);
    let mut panel = cover_square(w, h, &px, PANEL_ART_SIZE)?;
    round_corners(&mut panel, PANEL_ART_SIZE, PANEL_ART_RADIUS);
    let art = Art {
        size: ART_SIZE,
        rgba: Arc::new(cover_square(w, h, &px, ART_SIZE)?),
        panel_size: PANEL_ART_SIZE,
        panel: Arc::new(panel),
    };
    Some((art, accent))
}

/// Arrondit les coins d'une image carrée RGBA de côté `size` (rayon `radius` pixels), bord
/// adouci sur un pixel.
fn round_corners(px: &mut [u8], size: u32, radius: f32) {
    let n = size as f32;
    for y in 0..size {
        for x in 0..size {
            // Distance au centre du coin le plus proche, pour un pixel dans un coin.
            let (cx, cy) = ((x as f32 + 0.5).min(n - x as f32 - 0.5), (y as f32 + 0.5).min(n - y as f32 - 0.5));
            if cx >= radius || cy >= radius {
                continue;
            }
            let d = ((radius - cx).powi(2) + (radius - cy).powi(2)).sqrt();
            let keep = (radius - d + 0.5).clamp(0.0, 1.0);
            let a = &mut px[((y * size + x) * 4 + 3) as usize];
            *a = (*a as f32 * keep).round() as u8;
        }
    }
}

/// Rogne l'image `w` × `h` (RGBA) en carré centré et la réduit (moyenne des pixels couverts) en
/// `size` × `size`. `None` pour une image vide ou des pixels manquants.
fn cover_square(w: u32, h: u32, px: &[u8], size: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || size == 0 || px.len() < (w * h * 4) as usize {
        return None;
    }
    let side = w.min(h);
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for oy in 0..size {
        // Lignes et colonnes de la source couvertes par ce pixel (au moins une).
        let sy0 = y0 + oy * side / size;
        let sy1 = (y0 + (oy + 1) * side / size).max(sy0 + 1);
        for ox in 0..size {
            let sx0 = x0 + ox * side / size;
            let sx1 = (x0 + (ox + 1) * side / size).max(sx0 + 1);
            let mut sum = [0u64; 4];
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += px[i + c] as u64;
                    }
                }
            }
            let n = ((sy1 - sy0) * (sx1 - sx0)) as u64;
            let o = ((oy * size + ox) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (sum[c] / n) as u8;
            }
        }
    }
    Some(out)
}

fn on_snapshot(app: &AppHandle, s: MediaSnapshot) {
    let now = horizon_core::time::now_ms();
    let payload = MediaPayload {
        position_ms: s.info.as_ref().map(|i| position_at(i, now)).unwrap_or(0),
        collapsed: collapsed(s.info.as_ref()),
        info: s.info,
        sessions: s.sessions,
    };
    let playing = payload.info.as_ref().is_some_and(|i| i.playing);
    // La pochette est décodée avant de prendre le verrou.
    let decoded = s.art.map(|art| art.as_deref().and_then(art_pixels));
    let cover = decoded.as_ref().map(|d| d.as_ref().and_then(|(_, c)| *c));
    let art = decoded.map(|d| d.map(|(a, _)| a));
    let (send, new_track) = {
        let state = app.state::<MediaState>();
        let mut current = lock(&state.current);
        let mut sent = lock(&state.sent);
        // Les barres extrapolent la position : une relecture qui ne change rien de visible
        // (position cohérente, mêmes sessions) ne leur est pas renvoyée.
        let send = sent.as_ref().is_none_or(|(info, sessions)| {
            worth_sending(info.as_ref(), payload.info.as_ref(), now) || *sessions != payload.sessions
        });
        if send {
            *sent = Some((payload.info.clone(), payload.sessions.clone()));
        }
        let new_track = playing && is_new_track(current.payload.info.as_ref(), payload.info.as_ref());
        current.payload = payload.clone();
        if let Some(a) = &art {
            current.art = a.clone();
        }
        (send, new_track)
    };
    if send {
        let info = payload.info.as_ref();
        let media = serde_json::json!({
            "title": info.map(|i| i.title.clone()),
            "artist": info.map(|i| i.artist.clone()),
            "playing": info.is_some_and(|i| i.playing),
        });
        crate::plugins::events::emit(app, "media", media);
        let _ = app.emit(EV_MEDIA, payload);
    }
    if let Some(a) = art {
        let _ = app.emit(EV_MEDIA_ART, a);
    }
    // Couleur d'accent (fenêtres, Prisme) : la pochette pendant la lecture, sinon le fond d'écran.
    crate::accent::set_media(app, cover, playing);
    if new_track {
        crate::osd::track_changed(app);
    }
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    let h = horizon_sys::media::start(move |s| on_snapshot(&a, s))?;
    *lock(&app.state::<MediaState>().handle) = Some(h);
    Ok(())
}


/// Identifiant SMTC de l'appli de la lecture en cours.
pub fn current_app_id(app: &AppHandle) -> Option<String> {
    lock(&app.state::<MediaState>().current).payload.info.as_ref().map(|i| i.app_id.clone())
}

/// Envoie une commande au fil média ; erreur s'il n'a pas pu démarrer.
fn send(app: &AppHandle, c: MediaCommand) -> Result<(), String> {
    let state = app.state::<MediaState>();
    let handle = lock(&state.handle);
    handle.as_ref().ok_or("lecture en cours indisponible")?.send(c);
    Ok(())
}

#[derive(Clone, Default, Serialize)]
pub struct MediaFull {
    #[serde(flatten)]
    pub payload: MediaPayload,
    pub art: Option<Art>,
}

/// Titre en cours et lecture en marche (palette) ; `None` sans lecture.
pub fn now_playing(app: &AppHandle) -> Option<(String, bool)> {
    let state = app.state::<MediaState>();
    let current = lock(&state.current);
    current.payload.info.as_ref().map(|i| (i.title.clone(), i.playing))
}

/// État courant, pour une barre qui vient de s'ouvrir.
pub fn media_state(app: &AppHandle) -> MediaFull {
    let mut full = lock(&app.state::<MediaState>().current).clone();
    if let Some(i) = &full.payload.info {
        full.payload.position_ms = position_at(i, horizon_core::time::now_ms());
    }
    full
}

pub fn media_play_pause(app: &AppHandle) -> Result<(), String> {
    send(app, MediaCommand::PlayPause)
}

pub fn media_next(app: &AppHandle) -> Result<(), String> {
    send(app, MediaCommand::Next)
}

pub fn media_prev(app: &AppHandle) -> Result<(), String> {
    send(app, MediaCommand::Previous)
}

pub fn media_seek_relative(app: &AppHandle, ms: i64) -> Result<(), String> {
    send(app, MediaCommand::SeekRelative(ms))
}

/// Suivre cette lecture (identifiant d'appli) plutôt que celle choisie par Windows.
pub fn media_select_session(app: &AppHandle, app_id: String) -> Result<(), String> {
    send(app, MediaCommand::Select(app_id))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_crops_the_centre_and_averages() {
        // 4 × 2 : deux colonnes noires, deux blanches au centre… rognée en 2 × 2 au centre
        // (colonnes 1 et 2), puis réduite en 1 × 1 : moitié noire, moitié blanche.
        let mut px = Vec::new();
        for _ in 0..2 {
            for v in [0u8, 0, 255, 255] {
                px.extend([v, v, v, 255]);
            }
        }
        assert_eq!(cover_square(4, 2, &px, 1), Some(vec![127, 127, 127, 255]));
        assert_eq!(cover_square(4, 2, &px, 2), Some(vec![0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255]));
        assert_eq!(cover_square(0, 2, &px, 2), None);
        assert_eq!(cover_square(4, 4, &px, 2), None, "pixels manquants");
    }

    #[test]
    fn corners_are_rounded_and_the_rest_kept() {
        let size = 20;
        let mut px = vec![255u8; (size * size * 4) as usize];
        round_corners(&mut px, size, 6.0);
        let alpha = |x: u32, y: u32| px[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "coin découpé");
        assert_eq!(alpha(19, 19), 0);
        assert_eq!(alpha(10, 10), 255, "centre intact");
        assert_eq!(alpha(10, 0), 255, "bord droit intact");
        assert!(alpha(1, 1) < 128 && alpha(3, 3) > 128, "arc adouci");
    }

    #[test]
    fn cover_enlarges_a_small_picture() {
        let px = [10u8, 20, 30, 255];
        assert_eq!(cover_square(1, 1, &px, 2), Some([px; 4].concat()));
    }
}
