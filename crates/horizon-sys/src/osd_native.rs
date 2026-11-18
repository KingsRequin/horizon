//! Affichage du volume de Windows 11 (îlot XAML de l'Explorateur, en bas au centre) : masqué
//! pendant qu'Horizon montre le sien, remis ensuite. Méthode de HideVolumeOSD / FluentFlyout :
//! la fenêtre est poussée hors de l'écran et rendue transparente aux clics ; Windows continue de
//! la piloter (rien n'est injecté dans l'Explorateur).
//!
//! L'îlot n'existe qu'après le premier affichage depuis le démarrage de l'Explorateur : il est
//! cherché après chaque touche de volume (la première fois, il apparaît donc un instant). On le
//! reconnaît parmi les îlots de l'Explorateur à son style « sans activation » et à sa petite taille.

use std::sync::Mutex;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GWL_EXSTYLE, GetWindowLongPtrW, GetWindowRect, IsWindow, IsWindowVisible,
    SW_HIDE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
};
use windows::core::w;

/// Position hors de tout écran.
const AWAY: i32 = -32000;

/// Îlot masqué : fenêtre, style étendu et position d'origine.
struct Hidden {
    hwnd: isize,
    ex_style: isize,
    origin: (i32, i32),
}

static HIDDEN: Mutex<Option<Hidden>> = Mutex::new(None);

fn hidden() -> std::sync::MutexGuard<'static, Option<Hidden>> {
    HIDDEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Îlot de l'affichage du volume, s'il est à l'écran.
fn find() -> Option<HWND> {
    let mut prev: Option<HWND> = None;
    // SAFETY : énumération de fenêtres par classe ; `prev` avance à chaque tour (sinon la boucle
    // ne finirait pas).
    unsafe {
        loop {
            let island = FindWindowExW(None, prev, w!("XamlExplorerHostIslandWindow"), None).ok()?;
            prev = Some(island);
            if !IsWindowVisible(island).as_bool() {
                continue;
            }
            let ex = GetWindowLongPtrW(island, GWL_EXSTYLE);
            if ex & WS_EX_NOACTIVATE.0 as isize == 0 {
                continue;
            }
            let mut r = RECT::default();
            if GetWindowRect(island, &mut r).is_err() {
                continue;
            }
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            // L'affichage du volume (et des médias) : quelques centaines de pixels au plus.
            if w > 0 && h > 0 && w < 900 && h < 400 && r.left > AWAY / 2 {
                return Some(island);
            }
        }
    }
}

/// Masque l'affichage du volume s'il est trouvé (ou le garde masqué) ; vrai s'il l'est.
pub fn hide() -> bool {
    let mut g = hidden();
    if let Some(h) = g.as_ref() {
        let hwnd = HWND(h.hwnd as *mut _);
        let ours = (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as isize;
        // SAFETY : test de validité, lecture du style, déplacement seulement.
        let still = unsafe {
            IsWindow(Some(hwnd)).as_bool() && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & ours == ours
        };
        if still {
            // L'Explorateur le replace à chaque affichage : on le repousse.
            // SAFETY : fenêtre valide vérifiée ci-dessus.
            let _ = unsafe { SetWindowPos(hwnd, None, AWAY, AWAY, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
            return true;
        }
        // L'Explorateur a recréé l'îlot : on le cherche de nouveau.
        *g = None;
    }
    let Some(island) = find() else { return false };
    // SAFETY : fenêtre valide trouvée ci-dessus ; changements de style et de position seulement.
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(island, &mut r);
        let ex = GetWindowLongPtrW(island, GWL_EXSTYLE);
        SetWindowLongPtrW(island, GWL_EXSTYLE, ex | (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as isize);
        let _ = SetWindowPos(island, None, AWAY, AWAY, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        *g = Some(Hidden { hwnd: island.0 as isize, ex_style: ex, origin: (r.left, r.top) });
    }
    log::info!("affichage du volume de Windows masqué");
    true
}

/// Remet l'affichage du volume de Windows (style et position d'origine, caché jusqu'au
/// prochain changement de volume).
pub fn restore() {
    let Some(h) = hidden().take() else { return };
    let hwnd = HWND(h.hwnd as *mut _);
    // SAFETY : fenêtre testée avant usage ; changements de style et de position seulement.
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, h.ex_style);
        let _ = SetWindowPos(hwnd, None, h.origin.0, h.origin.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
    log::info!("affichage du volume de Windows remis");
}
