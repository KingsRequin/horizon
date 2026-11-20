//! Appuis de souris n'importe où sur l'écran, le temps qu'un volet est ouvert : il se ferme au
//! clic ailleurs sans que la barre prenne le focus (elle ne le prend jamais hors mode édition).
//!
//! Crochet souris bas niveau (`WH_MOUSE_LL`) sur un fil dédié (sa boucle de messages le sert),
//! posé par [`start`] et retiré par [`stop`] : rien ne tourne volet fermé.

use crossbeam_channel::Sender;
use std::sync::Mutex;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GA_ROOT, GetAncestor, GetMessageW, GetWindowThreadProcessId, HC_ACTION, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT, WM_RBUTTONDOWN, WindowFromPoint,
};

/// Fil du crochet en cours (identifiant Win32) et destinataire des appuis.
struct Watch {
    thread: u32,
    tx: Sender<(i32, i32)>,
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && matches!(wparam.0 as u32, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN) {
        // SAFETY : pour WH_MOUSE_LL, `lparam` pointe sur un MSLLHOOKSTRUCT valide pendant l'appel.
        let pt = unsafe { (*(lparam.0 as *const MSLLHOOKSTRUCT)).pt };
        // Jamais bloquant : le crochet doit rendre la main tout de suite.
        if let Ok(w) = WATCH.try_lock()
            && let Some(w) = w.as_ref()
        {
            let _ = w.tx.try_send((pt.x, pt.y));
        }
    }
    // SAFETY : chaînage normal du crochet.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Pose le crochet : chaque appui de bouton (gauche, droit, milieu) envoie sa position écran
/// (pixels physiques). Vrai si le crochet est posé (déjà posé compris).
pub fn start(tx: Sender<(i32, i32)>) -> bool {
    match WATCH.lock() {
        Ok(w) if w.is_some() => return true,
        Ok(_) => {}
        Err(_) => return false,
    }
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<u32>(1);
    let spawned = std::thread::Builder::new().name("clic-ailleurs".into()).spawn(move || {
        // SAFETY : rappel `extern "system"` valide pour toute la vie du fil, qui sert le crochet
        // par sa boucle de messages.
        unsafe {
            let hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), None, 0) {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("crochet souris impossible : {e}");
                    let _ = ready_tx.send(0);
                    return;
                }
            };
            let _ = ready_tx.send(GetCurrentThreadId());
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnhookWindowsHookEx(hook);
        }
    });
    if let Err(e) = spawned {
        log::warn!("fil du crochet souris : {e}");
        return false;
    }
    match (ready_rx.recv(), WATCH.lock()) {
        (Ok(thread), Ok(mut w)) if thread != 0 => {
            *w = Some(Watch { thread, tx });
            true
        }
        _ => false,
    }
}

/// Retire le crochet (son fil s'arrête). Sans effet s'il n'est pas posé.
pub fn stop() {
    let Some(w) = WATCH.lock().ok().and_then(|mut w| w.take()) else { return };
    // SAFETY : message posté au fil du crochet, qui sort de sa boucle et retire le crochet.
    let _ = unsafe { PostThreadMessageW(w.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
}

/// Fenêtre racine sous ce point écran (pixels physiques), si elle appartient à ce processus
/// (barre, panneau, menu, info-bulle d'Horizon).
pub fn own_window_at(x: i32, y: i32) -> Option<isize> {
    // SAFETY : appels sans pointeur à garder ; une fenêtre disparue entre-temps rend 0.
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.is_invalid() {
            return None;
        }
        let root = GetAncestor(h, GA_ROOT);
        let root = if root.is_invalid() { h } else { root };
        let mut pid = 0u32;
        GetWindowThreadProcessId(root, Some(&mut pid));
        (pid == GetCurrentProcessId()).then_some(root.0 as isize)
    }
}
