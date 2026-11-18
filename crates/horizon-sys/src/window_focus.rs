//! Fenêtres principales et mise au premier plan robuste (Windows refuse souvent
//! `SetForegroundWindow` à un processus qui n'est pas au premier plan).

use crate::win32::is_cloaked;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongPtrW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
    WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

/// Fenêtre principale : visible, ni fenêtre d'outil, ni fenêtre possédée, ni masquée par le
/// système (bureaux virtuels, applis suspendues).
pub(crate) fn is_main_window(h: HWND) -> bool {
    // SAFETY : lectures d'attributs d'une fenêtre (une fenêtre disparue renvoie des zéros).
    unsafe {
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        IsWindowVisible(h).as_bool()
            && ex & WS_EX_TOOLWINDOW.0 == 0
            && GetWindow(h, GW_OWNER).map_or(true, |o| o.0.is_null())
            && !is_cloaked(h)
    }
}

pub(crate) fn pid_of(h: HWND) -> u32 {
    let mut pid = 0;
    // SAFETY : sortie u32 valide.
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    pid
}

/// Fenêtres principales ouvertes (fenêtre, PID propriétaire), de la plus haute à la plus basse
/// dans l'ordre d'empilement.
pub fn main_windows() -> Vec<(isize, u32)> {
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        // SAFETY : `data` pointe vers le Vec de l'appelant, vivant pendant l'énumération.
        let out = unsafe { &mut *(data.0 as *mut Vec<(isize, u32)>) };
        if is_main_window(h) {
            out.push((h.0 as isize, pid_of(h)));
        }
        true.into()
    }
    let mut out: Vec<(isize, u32)> = Vec::new();
    // SAFETY : énumération synchrone, le pointeur reste valide pendant l'appel.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize)) };
    out
}

/// Nom de l'exécutable d'un processus (« WindowsTerminal.exe »).
pub fn exe_name(pid: u32) -> Option<String> {
    let path = crate::foreground::exe_of_pid(pid)?;
    path.rsplit(['\\', '/']).next().map(str::to_owned)
}

fn send(inputs: &[INPUT]) {
    // SAFETY : tableau d'entrées initialisées, taille de la structure annoncée.
    unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
}

fn key(vk: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY(vk),
                dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() },
                ..Default::default()
            },
        },
    }
}

/// Fenêtre au premier plan (0 s'il n'y en a pas).
pub fn foreground() -> isize {
    // SAFETY : appel sans argument.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// Restaure (si réduite) et met la fenêtre au premier plan. Windows n'y autorise que le
/// processus qui a reçu la dernière entrée : après un clic sur Horizon c'est le cas ; sinon
/// une entrée souris vide, puis l'appui d'Alt, débloquent la mise au premier plan.
pub fn bring_to_front(hwnd: isize) -> bool {
    let h = HWND(hwnd as *mut _);
    // SAFETY : fenêtre vérifiée par IsWindow ; appels sans effet si elle disparaît entre-temps.
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return false;
        }
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let done = || GetForegroundWindow() == h;
        if SetForegroundWindow(h).as_bool() && done() {
            return true;
        }
        // Entrée souris vide : Horizon devient « dernier processus à avoir reçu une entrée ».
        send(&[INPUT { r#type: INPUT_MOUSE, ..Default::default() }]);
        if SetForegroundWindow(h).as_bool() && done() {
            return true;
        }
        // Dernier recours : Alt enfoncé pendant l'appel (un appui d'Alt lève le verrou de
        // premier plan). Son relâchement arrive à la fenêtre ramenée ; un terminal l'ignore.
        send(&[key(VK_MENU.0, false)]);
        let _ = SetForegroundWindow(h);
        send(&[key(VK_MENU.0, true)]);
        let _ = BringWindowToTop(h);
        done()
    }
}
