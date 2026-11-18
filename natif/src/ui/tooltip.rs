//! Info-bulles : l'attribut `title` des éléments de la version Tauri. Un widget appelle
//! `Tip.show(texte)` au survol et `Tip.hide()` à la sortie ou au clic (`ui/tooltip.slint`) ;
//! l'info-bulle Windows (contrôle commun « tooltips », suivi à la position voulue) apparaît sous
//! la souris après `DELAY`. Une seule pour tout Horizon : la souris n'est que sur une barre à la
//! fois. Fil de l'interface.

use std::cell::RefCell;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTF_ABSOLUTE, TTF_TRACK, TTM_ADDTOOLW, TTM_DELTOOLW, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE,
    TTM_TRACKPOSITION, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyWindow, GetCursorPos, IsWindow, SendMessageW, WINDOW_STYLE, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::PWSTR;

/// Attente avant d'afficher (celle d'un navigateur).
const DELAY: Duration = Duration::from_millis(500);
/// Écart sous le pointeur (pixels physiques).
const BELOW: i32 = 22;

struct Tip {
    hwnd: HWND,
    /// Fenêtre propriétaire de l'outil affiché (barre).
    owner: Option<HWND>,
    /// Texte de l'outil (gardé tant qu'il est affiché).
    text: Vec<u16>,
}

thread_local! {
    static TIP: RefCell<Option<Tip>> = const { RefCell::new(None) };
    static TIMER: slint::Timer = slint::Timer::default();
}

fn tool(owner: HWND, text: &mut [u16]) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_TRACK | TTF_ABSOLUTE,
        hwnd: owner,
        uId: 1,
        lpszText: PWSTR(text.as_mut_ptr()),
        ..Default::default()
    }
}

/// Contrôle d'info-bulle (créé à la première demande, recréé s'il a disparu).
fn control() -> Option<HWND> {
    let alive = TIP.with_borrow(|t| t.as_ref().map(|t| t.hwnd)).filter(|h| unsafe { IsWindow(Some(*h)) }.as_bool());
    if alive.is_some() {
        return alive;
    }
    // SAFETY : création d'une fenêtre de classe système sur ce fil.
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TRANSPARENT,
            TOOLTIPS_CLASSW,
            None,
            WS_POPUP | WINDOW_STYLE(TTS_NOPREFIX | TTS_ALWAYSTIP),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()?
    };
    // SAFETY : contrôle créé ci-dessus.
    unsafe {
        SendMessageW(hwnd, TTM_SETMAXTIPWIDTH, Some(WPARAM(0)), Some(LPARAM(420)));
    }
    TIP.with_borrow_mut(|t| *t = Some(Tip { hwnd, owner: None, text: Vec::new() }));
    Some(hwnd)
}

/// Affiche `text` sous la souris après le délai, pour la fenêtre `owner` (HWND de la barre).
pub fn show(owner: isize, text: &str) {
    hide();
    if text.is_empty() {
        return;
    }
    let text = text.to_owned();
    TIMER.with(|t| {
        t.start(slint::TimerMode::SingleShot, DELAY, move || display(HWND(owner as *mut _), &text));
    });
}

fn display(owner: HWND, text: &str) {
    let Some(hwnd) = control() else { return };
    let mut p = POINT::default();
    // SAFETY : lecture de la position du pointeur.
    if unsafe { GetCursorPos(&mut p) }.is_err() {
        return;
    }
    TIP.with_borrow_mut(|t| {
        let Some(t) = t.as_mut() else { return };
        t.text = text.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ti = tool(owner, &mut t.text);
        let pos = ((p.y + BELOW) as u32 & 0xffff) << 16 | (p.x as u32 & 0xffff);
        // SAFETY : contrôle vivant ; `ti` et son texte valides pendant les appels (le contrôle
        // copie le texte).
        unsafe {
            SendMessageW(hwnd, TTM_ADDTOOLW, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
            SendMessageW(hwnd, TTM_TRACKPOSITION, Some(WPARAM(0)), Some(LPARAM(pos as isize)));
            SendMessageW(hwnd, TTM_TRACKACTIVATE, Some(WPARAM(1)), Some(LPARAM(&mut ti as *mut _ as isize)));
        }
        t.owner = Some(owner);
    });
}

/// Retire l'info-bulle (sortie, clic) et annule celle qui allait venir.
pub fn hide() {
    TIMER.with(|t| t.stop());
    TIP.with_borrow_mut(|t| {
        let Some(t) = t.as_mut() else { return };
        let Some(owner) = t.owner.take() else { return };
        let mut ti = tool(owner, &mut t.text);
        // SAFETY : contrôle et outil créés par nous.
        unsafe {
            SendMessageW(t.hwnd, TTM_TRACKACTIVATE, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
            SendMessageW(t.hwnd, TTM_DELTOOLW, Some(WPARAM(0)), Some(LPARAM(&mut ti as *mut _ as isize)));
        }
    });
}

/// Détruit le contrôle (arrêt).
pub fn close() {
    hide();
    if let Some(t) = TIP.with_borrow_mut(Option::take) {
        // SAFETY : fenêtre créée sur ce fil.
        unsafe {
            let _ = DestroyWindow(t.hwnd);
        }
    }
}
