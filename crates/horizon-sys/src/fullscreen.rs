//! Fenêtres de premier niveau dans l'ordre Z (dessus d'abord), pour détecter le plein écran.

use crate::win32::{class_name, frame_bounds, is_cloaked};
use horizon_core::geometry::WinInfo;
use windows::Win32::Foundation::{HWND, LPARAM, POINT};
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, EnumWindows, GWL_EXSTYLE, GetCursorInfo, GetCursorPos, GetWindowLongPtrW, GetWindowTextW,
    IsIconic, IsWindowVisible, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};
use windows::core::BOOL;

/// Classes du bureau et de la barre des tâches : jamais du plein écran.
const SHELL_CLASSES: [&str; 4] = ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

struct Ctx<'a> {
    own: &'a [isize],
    out: Vec<(WinInfo, HWND)>,
}

/// Une fenêtre de l'ordre Z avec de quoi la reconnaître (sonde `examples/fullscreen_probe`).
pub struct Described {
    pub info: WinInfo,
    pub class: String,
    pub title: String,
}

unsafe extern "system" fn callback(h: HWND, data: LPARAM) -> BOOL {
    // SAFETY : `data` pointe vers le Ctx de `windows_z_order`, vivant pendant l'énumération.
    let ctx = unsafe { &mut *(data.0 as *mut Ctx) };
    // SAFETY : simples lectures d'état sur un HWND fourni par EnumWindows.
    let (visible, iconic, ex) = unsafe {
        (
            IsWindowVisible(h).as_bool(),
            IsIconic(h).as_bool(),
            GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
        )
    };
    if !visible || iconic || is_cloaked(h) {
        return true.into();
    }
    let Some(rect) = frame_bounds(h) else { return true.into() };
    if rect.w <= 0 || rect.h <= 0 {
        return true.into();
    }
    let overlay = ex & (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) != 0;
    let ignorable = overlay
        || ctx.own.contains(&(h.0 as isize))
        || SHELL_CLASSES.contains(&class_name(h).as_str());
    ctx.out.push((WinInfo { rect, ignorable }, h));
    true.into()
}

fn enumerate(own: &[isize]) -> Vec<(WinInfo, HWND)> {
    let mut ctx = Ctx { own, out: Vec::new() };
    // SAFETY : énumération synchrone, le pointeur sur `ctx` reste valide pendant l'appel.
    let _ = unsafe { EnumWindows(Some(callback), LPARAM(&mut ctx as *mut _ as isize)) };
    ctx.out
}

/// Fenêtres visibles, dessus d'abord ; `own` = fenêtres d'Horizon (ignorées).
pub fn windows_z_order(own: &[isize]) -> Vec<WinInfo> {
    enumerate(own).into_iter().map(|(info, _)| info).collect()
}

/// Les mêmes fenêtres, avec leur classe et leur titre.
pub fn describe_z_order(own: &[isize]) -> Vec<Described> {
    enumerate(own)
        .into_iter()
        .map(|(info, h)| {
            let mut buf = [0u16; 160];
            // SAFETY : tampon de la taille annoncée.
            let n = unsafe { GetWindowTextW(h, &mut buf) }.max(0) as usize;
            Described { info, class: class_name(h), title: String::from_utf16_lossy(&buf[..n]) }
        })
        .collect()
}

/// Position du curseur en pixels physiques.
pub fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY : sortie POINT valide.
    unsafe { GetCursorPos(&mut p) }.ok().map(|_| (p.x, p.y))
}

/// Position du curseur s'il est affiché ; `None` quand l'appli du dessus l'a caché (jeu qui
/// capture la souris : sa position ne dit plus où l'utilisateur veut aller).
pub fn shown_cursor() -> Option<(i32, i32)> {
    let mut info = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
    // SAFETY : sortie CURSORINFO valide, taille renseignée.
    unsafe { GetCursorInfo(&mut info) }.ok()?;
    let shown = info.flags.0 & CURSOR_SHOWING.0 != 0 && !info.hCursor.is_invalid();
    shown.then_some((info.ptScreenPos.x, info.ptScreenPos.y))
}
