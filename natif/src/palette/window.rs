//! Raccourci global de la palette, ouverture sur l'écran de la souris, fermeture. Repris de
//! `app/src-tauri/src/palette/window.rs` ; la fenêtre Slint est dans `ui::palette` (fil de
//! l'interface : `toggle`, `hide` et `open` y sont appelées).

use crate::events::EV_PALETTE_ENTRIES;
use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{lock, spawn_named};
use horizon_core::hotkey::Hotkey;
use horizon_core::palette::window_rect;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::catalogue::catalogue;
use super::{LABEL, Palette};

/// Taille logique de la fenêtre (la boîte visible s'ajuste au contenu, le reste est transparent).
const SIZE: (u32, u32) = (620, 470);

/// Cache la palette ; `restore` rend le premier plan à la fenêtre d'avant (Échap, action) —
/// pas après une perte de focus, où l'utilisateur a déjà choisi une autre fenêtre. Fil de
/// l'interface.
pub fn hide(app: &AppHandle, restore: bool) {
    let previous = lock(&app.state::<Palette>().previous).take();
    if crate::ui::palette::hide_window() && restore && let Some(h) = previous {
        horizon_sys::window_focus::bring_to_front(h);
    }
}

/// Ouvre la palette ; la referme si elle est déjà ouverte. Fil de l'interface.
pub fn toggle(app: &AppHandle) {
    if crate::ui::palette::is_visible() {
        return hide(app, true);
    }
    let started = Instant::now();
    let Some(hwnd) = app.get_window(LABEL).map(|w| w.hwnd()) else { return };
    let before = horizon_sys::window_focus::foreground();
    let p = app.state::<Palette>();
    *lock(&p.previous) = (before != 0 && before != hwnd).then_some(before);
    // Le catalogue interroge l'audio, Prisme, les suggestions… : construit hors du fil de
    // l'interface, il remplace celui de l'ouverture précédente, et la fenêtre relance sa
    // recherche.
    let a = app.clone();
    let generation = p.catalogue_gen.fetch_add(1, Ordering::SeqCst) + 1;
    spawn_named("palette-catalogue", move || {
        let t = Instant::now();
        let entries = catalogue(&a);
        log::debug!("palette : catalogue de {} entrées en {} ms", entries.len(), t.elapsed().as_millis());
        let p = a.state::<Palette>();
        {
            let mut current = lock(&p.entries);
            if p.catalogue_gen.load(Ordering::SeqCst) != generation {
                return;
            }
            *current = entries;
        }
        let _ = a.emit_to(LABEL, EV_PALETTE_ENTRIES, ());
    });
    let monitors = horizon_sys::monitors::list();
    let cursor = horizon_sys::fullscreen::cursor().unwrap_or((0, 0));
    let Some(r) = window_rect(&monitors, cursor, SIZE.0, SIZE.1) else { return };
    *lock(&p.origin) = Some(r);
    *lock(&p.shown_at) = Some(Instant::now());
    crate::ui::palette::show_window((r.x, r.y, r.w.max(1) as u32, r.h.max(1) as u32));
    log::debug!("palette : affichée en {} ms", started.elapsed().as_millis());
}

/// Perte du premier plan : la palette se ferme, sauf juste après son affichage (Windows peut
/// annoncer une perte de focus avant de le donner).
pub(crate) fn deactivated(app: &AppHandle) {
    let recent = lock(&app.state::<Palette>().shown_at).is_some_and(|t| t.elapsed().as_millis() < 150);
    if !recent {
        hide(app, false);
    }
}

/// Crée la fenêtre (cachée) et enregistre le raccourci global (fil de l'interface).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    crate::ui::palette::create(app)?;
    app.state::<Palette>().start_apps(app);
    register_hotkey(app);
    Ok(())
}

fn register_hotkey(app: &AppHandle) {
    let text = crate::state::settings().palette.hotkey;
    if text.trim().is_empty() {
        return log::info!("palette : pas de raccourci (settings.toml)");
    }
    match set_hotkey(app, &text) {
        Ok(()) => log::info!("palette : raccourci {text}"),
        Err(e) => log::error!("palette : {e}"),
    }
}

/// Met en service le raccourci `text` (vide = aucun), à chaud. Le nouveau est pris avant de
/// lâcher l'ancien : en cas d'échec (raccourci illisible ou déjà pris par une autre
/// application), l'ancien reste en service et l'erreur est renvoyée.
pub fn set_hotkey(app: &AppHandle, text: &str) -> Result<(), String> {
    let text = text.trim();
    let p = app.state::<Palette>();
    let mut slot = lock(&p.hotkey);
    if text.is_empty() {
        if let Some(h) = slot.handle.take() {
            h.stop();
        }
        (slot.text, slot.error) = (String::new(), None);
        return Ok(());
    }
    let result = match Hotkey::parse(text) {
        None => Err(format!("raccourci « {text} » illisible : il faut Ctrl, Alt ou Win et une touche (exemple : Ctrl+Alt+P)")),
        Some(key) if slot.handle.is_some() && Hotkey::parse(&slot.text) == Some(key) => {
            slot.text = text.to_owned();
            Ok(None)
        }
        Some(key) => {
            let a = app.clone();
            horizon_sys::hotkey::watch(key, move || {
                let b = a.clone();
                let _ = a.run_on_main_thread(move || toggle(&b));
            })
            .map(Some)
            .map_err(|e| format!("raccourci {text} indisponible : {e:#}"))
        }
    };
    match result {
        Ok(handle) => {
            if let Some(h) = handle
                && let Some(old) = slot.handle.replace(h)
            {
                old.stop();
            }
            (slot.text, slot.error) = (text.to_owned(), None);
            Ok(())
        }
        Err(e) => {
            slot.error = Some(e.clone());
            Err(e)
        }
    }
}

/// Raccourci en service (vide = aucun) et dernière erreur.
pub fn hotkey_status(app: &AppHandle) -> (String, Option<String>) {
    let Some(p) = app.try_state::<Palette>() else { return (String::new(), None) };
    let slot = lock(&p.inner().hotkey);
    let text = if slot.handle.is_some() { slot.text.clone() } else { String::new() };
    (text, slot.error.clone())
}

/// Ouvre la palette (menus, icône de notification), de n'importe quel fil.
pub fn open(app: &AppHandle) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if !crate::ui::palette::is_visible() {
            toggle(&a);
        }
    });
}

/// Essai (`--palette-essai`) : la palette s'ouvre, et ses actions sont seulement journalisées
/// jusqu'à l'arrêt d'Horizon.
pub fn open_essai(app: &AppHandle) {
    app.state::<Palette>().essai.store(true, Ordering::SeqCst);
    log::info!("palette : essai, les actions ne s'exécutent pas");
    open(app);
}
