//! Une fenêtre de barre par écran, enregistrée comme appbar ; reconstruction quand les écrans
//! changent.
//!
//! Repris de `app/src-tauri/src/bars.rs`. Différences : la fenêtre est une fenêtre Slint créée
//! sur le fil de l'interface (`ui::bar::open`), pas un `WebviewWindowBuilder` ; l'affichage et le
//! masquage passent par ce fil (`run_on_main_thread`). Plus de fond acrylique à retirer à
//! chaque affichage (fenêtre layered), ni de page WebView2 à suspendre.

use crate::host::{AppHandle, Manager, Window};
use crate::util::{lock, start_thread};
use horizon_core::geometry::{MonitorInfo, appbar_rect_for_monitor, same_screens};
use horizon_core::layout::BAND_HEIGHT;
use horizon_sys::appbar::{self, AppbarEvent};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

pub struct BarWindow {
    pub label: String,
    pub monitor: MonitorInfo,
    pub hwnd: isize,
    /// Hauteur réservée en pixels logiques (`BarLayout::height` de sa barre).
    pub height: u32,
}

/// Hauteur voulue pour la barre d'un écran (la bande seule si la disposition n'en a pas).
fn wanted_height(app: &AppHandle, monitor_id: &str) -> u32 {
    app.state::<crate::state::AppState>().bar(monitor_id).map_or(BAND_HEIGHT, |b| b.height)
}

#[derive(Default)]
pub struct Bars {
    /// Fenêtres de barre ouvertes.
    pub list: Mutex<Vec<BarWindow>>,
    /// Écrans pour lesquels les barres ont été construites la dernière fois.
    screens: Mutex<Vec<MonitorInfo>>,
    /// Nombre de changements d'écrans constatés.
    pub screen_changes: AtomicU64,
}

/// Crée la barre d'un écran (fil de l'interface).
fn spawn_one(app: &AppHandle, index: usize, monitor: MonitorInfo) -> anyhow::Result<BarWindow> {
    // Nom unique à chaque reconstruction : la destruction des anciennes fenêtres est différée,
    // leur nom reste pris un moment.
    static GENERATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let generation = if index == 0 {
        GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    } else {
        GENERATION.load(std::sync::atomic::Ordering::Relaxed)
    };
    let label = format!("bar-{generation}-{index}");
    let height = wanted_height(app, &monitor.id);
    let rect = appbar_rect_for_monitor(&monitor, height);
    // La fenêtre naît à sa place, sur son écran : elle prend tout de suite l'échelle de l'écran.
    let hwnd = crate::ui::bar::open(app, &label, &monitor, (rect.x, rect.y, rect.w.max(1) as u32, rect.h.max(1) as u32))?;
    // Pas de bordure DWM (ni de son ombre) sur une fenêtre sans cadre.
    appbar::remove_frame(hwnd);
    let granted = appbar::register(hwnd, rect)?;
    log::info!("barre {label} sur {} ({:?})", monitor.device_name, granted);
    if let Some(w) = app.get_window(&label) {
        show(&w, hwnd)?;
    }
    Ok(BarWindow { label, monitor, hwnd, height })
}

/// Affiche une barre, sans l'activer (fil de l'interface).
pub fn show(window: &Window, hwnd: isize) -> anyhow::Result<()> {
    let w = window.clone();
    window.app_handle().run_on_main_thread(move || {
        crate::ui::bar::set_visible(w.label(), true);
        // Garde : Windows 11 peut poser sa bordure DWM sur une fenêtre sans cadre.
        appbar::remove_frame(hwnd);
    })
}

/// Masque une barre (plein écran) : plus aucune image n'est dessinée tant qu'elle est cachée.
pub fn hide(window: &Window) -> anyhow::Result<()> {
    let label = window.label().to_owned();
    window.app_handle().run_on_main_thread(move || crate::ui::bar::set_visible(&label, false))
}

/// Crée les barres, puis suit les changements d'écran.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    spawn_all(app);
    watch(app.clone())
}

/// Crée une barre sur chaque écran (fil de l'interface).
pub fn spawn_all(app: &AppHandle) {
    let monitors = horizon_sys::monitors::list();
    // Même si une barre échoue, ces écrans-là sont traités : pas de reconstruction en boucle.
    *lock(&app.state::<Bars>().inner().screens) = monitors.clone();
    app.state::<crate::state::AppState>().sync_monitors(&monitors);
    // Fenêtres créées sans tenir le verrou des barres (voir l'ordre des verrous dans `state`).
    let mut created = Vec::new();
    for (i, m) in monitors.into_iter().enumerate() {
        match spawn_one(app, i, m) {
            Ok(b) => created.push(b),
            Err(e) => log::error!("création de la barre {i} : {e:#}"),
        }
    }
    lock(&app.state::<Bars>().inner().list).extend(created);
}

/// Ferme toutes les barres (place libérée) puis les recrée.
pub fn rebuild(app: &AppHandle) {
    close_all(app);
    spawn_all(app);
}

/// Après un événement d'écran : ne recrée les barres que si les écrans ont vraiment changé.
/// Une barre posée sur un écran à une autre échelle reçoit `WM_DPICHANGED` à sa création ;
/// tout recréer à chaque fois faisait réapparaître les barres en boucle. Sinon, chaque barre
/// est seulement remise à sa place.
fn rebuild_if_changed(app: &AppHandle) {
    let now = horizon_sys::monitors::list();
    if same_screens(&lock(&app.state::<Bars>().inner().screens), &now) {
        let hwnds: Vec<isize> = {
            let bars = app.state::<Bars>();
            let list = lock(&bars.list);
            list.iter().map(|b| b.hwnd).collect()
        };
        for h in hwnds {
            reposition(app, h);
        }
        return;
    }
    log::info!("écrans modifiés : reconstruction des barres");
    app.state::<Bars>().screen_changes.fetch_add(1, Ordering::Relaxed);
    rebuild(app);
}

/// Libère la place réservée et ferme les fenêtres (fil de l'interface, où vivent les fenêtres
/// Slint).
pub fn close_all(app: &AppHandle) {
    let bars = app.state::<Bars>();
    let old: Vec<BarWindow> = std::mem::take(&mut *lock(&bars.list));
    let labels: Vec<String> = old.iter().map(|b| b.label.clone()).collect();
    app.state::<crate::widgets::media::audio::AudioHub>().forget(&labels);
    for b in old {
        appbar::unregister(b.hwnd);
        app.unregister_window(&b.label);
        crate::ui::bar::close(&b.label);
    }
}

fn reposition(app: &AppHandle, hwnd: isize) {
    // Verrou relâché avant l'appel à Windows, qui peut envoyer des messages aux barres.
    let found = {
        let bars = app.state::<Bars>();
        let list = lock(&bars.list);
        list.iter().find(|b| b.hwnd == hwnd).map(|b| (b.label.clone(), appbar_rect_for_monitor(&b.monitor, b.height)))
    };
    if let Some((label, rect)) = found
        && let Err(e) = appbar::update(hwnd, rect)
    {
        log::warn!("repositionnement de {label} : {e:#}");
    }
}

/// Après un changement de disposition : chaque barre dont la hauteur a changé (réglage, menu,
/// annulation, profil) est redimensionnée et la place réservée sur son écran suit, à chaud.
pub fn sync_heights(app: &AppHandle) {
    let windows: Vec<(isize, String, u32)> = {
        let bars = app.state::<Bars>();
        let list = lock(&bars.list);
        list.iter().map(|b| (b.hwnd, b.monitor.id.clone(), b.height)).collect()
    };
    // Disposition lue verrou des barres relâché (ordre des verrous : voir `state`).
    let changed: Vec<(isize, u32)> = windows
        .into_iter()
        .filter_map(|(hwnd, monitor, old)| {
            let h = wanted_height(app, &monitor);
            (h != old).then_some((hwnd, h))
        })
        .collect();
    if changed.is_empty() {
        return;
    }
    {
        let bars = app.state::<Bars>();
        let mut list = lock(&bars.list);
        for (hwnd, h) in &changed {
            if let Some(b) = list.iter_mut().find(|b| b.hwnd == *hwnd) {
                b.height = *h;
            }
        }
    }
    // Windows envoie des messages aux barres pendant `ABM_SETPOS` : sur le fil de l'interface.
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        for (hwnd, h) in changed {
            log::info!("barre {hwnd:#x} : hauteur {h} px");
            reposition(&a, hwnd);
        }
    });
}

/// Écoute les événements des appbars. Les changements d'écran arrivent en rafale : on attend
/// le calme (800 ms) avant de tout reconstruire.
fn watch(app: AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    appbar::set_event_sink(tx);
    start_thread("appbar-events", move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                AppbarEvent::PosChanged(h) => {
                    let a = app.clone();
                    let _ = app.run_on_main_thread(move || reposition(&a, h));
                }
                AppbarEvent::FullscreenApp => app.state::<crate::autohide::Autohide>().wake(),
                AppbarEvent::DisplayChanged => {
                    while rx.recv_timeout(Duration::from_millis(800)).is_ok() {}
                    let a = app.clone();
                    let _ = app.run_on_main_thread(move || rebuild_if_changed(&a));
                }
            }
        }
    })
}
