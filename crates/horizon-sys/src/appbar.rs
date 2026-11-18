//! Barres « appbar » : Windows réserve leur hauteur en haut de l'écran, les fenêtres maximisées
//! commencent dessous. La fenêtre est sous-classée pour recevoir les avis de l'appbar (place,
//! plein écran) et les changements d'écrans, renvoyés sur un canal.

use crossbeam_channel::Sender;
use horizon_core::geometry::Rect;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Shell::{
    ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABM_WINDOWPOSCHANGED, ABN_FULLSCREENAPP, ABN_POSCHANGED,
    APPBARDATA, DefSubclassProc, RemoveWindowSubclass, SHAppBarMessage, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, RegisterWindowMessageW, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SetWindowLongPtrW, SetWindowPos, ShowWindow, WM_ACTIVATE, WM_DISPLAYCHANGE, WM_DPICHANGED,
    WM_SETTINGCHANGE, WM_WINDOWPOSCHANGED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::w;

/// Ce que les fenêtres de barre signalent au reste de l'appli.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppbarEvent {
    /// Une autre appbar a bougé : il faut redemander notre position.
    PosChanged(isize),
    /// Écrans ajoutés, retirés, résolution ou échelle changée : tout reconstruire.
    DisplayChanged,
    /// Une appli passe en plein écran (ou en sort) sur l'écran d'une barre.
    FullscreenApp,
}

const SUBCLASS_ID: usize = 0x484f_525a; // « HORZ »

static EVENTS: OnceLock<Mutex<Option<Sender<AppbarEvent>>>> = OnceLock::new();
/// Fenêtres enregistrées comme appbar, pour libérer leur place même après une panique.
static REGISTERED: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn callback_message() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    // SAFETY : chaîne constante terminée par zéro.
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("HorizonAppbarMessage")) })
}

fn send(ev: AppbarEvent) {
    if let Some(tx) = EVENTS.get().and_then(|m| m.lock().ok()).and_then(|g| g.clone()) {
        let _ = tx.send(ev);
    }
}

/// Définit le canal qui reçoit les événements de toutes les barres.
pub fn set_event_sink(tx: Sender<AppbarEvent>) {
    let slot = EVENTS.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = slot.lock() {
        *g = Some(tx);
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let appbar = callback_message();
    if msg == appbar && wparam.0 as u32 == ABN_POSCHANGED {
        send(AppbarEvent::PosChanged(hwnd.0 as isize));
    } else if msg == appbar && wparam.0 as u32 == ABN_FULLSCREENAPP {
        send(AppbarEvent::FullscreenApp);
    } else if msg == WM_DISPLAYCHANGE || msg == WM_DPICHANGED {
        send(AppbarEvent::DisplayChanged);
    } else if msg == WM_SETTINGCHANGE && wparam.0 == 0x2F {
        // SPI_SETWORKAREA : zone de travail modifiée par une autre barre.
        send(AppbarEvent::PosChanged(hwnd.0 as isize));
    } else if msg == WM_ACTIVATE || msg == WM_WINDOWPOSCHANGED {
        // Exigé par la documentation des appbars pour garder l'ordre Z cohérent.
        let mut abd = data(hwnd);
        // SAFETY : APPBARDATA initialisée avec la bonne taille.
        unsafe { SHAppBarMessage(ABM_WINDOWPOSCHANGED, &mut abd) };
    }
    // SAFETY : chaîne normale des sous-classes.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

fn data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: callback_message(),
        uEdge: ABE_TOP,
        ..Default::default()
    }
}

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// Rend la fenêtre inactivable et absente d'Alt-Tab (outil sans focus).
pub fn make_tool_window(h: isize) {
    let hw = hwnd(h);
    // SAFETY : simple lecture/écriture du style étendu d'une fenêtre qui nous appartient.
    unsafe {
        let ex = GetWindowLongPtrW(hw, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hw,
            GWL_EXSTYLE,
            ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize,
        );
    }
}

/// Affiche ou cache une fenêtre SANS l'activer. `show()` de tao active la fenêtre (SW_SHOW)
/// dès qu'elle a été créée : un panneau survolé ou une barre qui réapparaît volerait alors le
/// premier plan (et le clavier) à l'appli de l'utilisateur.
pub fn set_shown(h: isize, shown: bool) {
    // SAFETY : fenêtre qui nous appartient ; ShowWindow renvoie l'ancien état, pas une erreur.
    unsafe {
        let _ = ShowWindow(hwnd(h), if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

/// Supprime le liseré et les coins arrondis que Windows 11 ajoute aux fenêtres sans bordure.
pub fn remove_frame(h: isize) {
    use windows::Win32::Graphics::Dwm::{
        DWMNCRP_DISABLED, DWMSBT_NONE, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
        DwmSetWindowAttribute,
    };
    const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
    let hw = hwnd(h);
    // SAFETY : attributs DWM documentés, tampons de la taille annoncée.
    unsafe {
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hw,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            size_of_val(&corner) as u32,
        );
        // Sans bordure dessinée par DWM, pas d'ombre (environ 70 px) qui assombrit le bureau
        // sous la barre.
        let policy = DWMNCRP_DISABLED;
        let _ = DwmSetWindowAttribute(
            hw,
            DWMWA_NCRENDERING_POLICY,
            &policy as *const _ as *const _,
            size_of_val(&policy) as u32,
        );
        // Pas de fond système : la fenêtre recevait l'acrylique (DWMSBT_TRANSIENTWINDOW), qui
        // retombe sur un gris uni sous une fenêtre inactive — le « fond gris » des barres.
        let backdrop = DWMSBT_NONE;
        let _ = DwmSetWindowAttribute(
            hw,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            size_of_val(&backdrop) as u32,
        );
        let none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(
            hw,
            DWMWA_BORDER_COLOR,
            &none as *const _ as *const _,
            size_of::<u32>() as u32,
        );
    }
}

/// Windows 11 remet parfois le fond acrylique (`DWMSBT_TRANSIENTWINDOW`, gris ou flou) et la
/// bordure DWM (avec son ombre) sur nos fenêtres, sans message identifié. Vrai si c'était le
/// cas : `remove_frame` vient de les retirer.
pub fn ensure_no_backdrop(h: isize) -> bool {
    use windows::core::BOOL;
    use windows::Win32::Graphics::Dwm::{
        DWM_SYSTEMBACKDROP_TYPE, DWMSBT_NONE, DWMWA_NCRENDERING_ENABLED,
        DWMWA_SYSTEMBACKDROP_TYPE, DwmGetWindowAttribute,
    };
    let mut backdrop = DWM_SYSTEMBACKDROP_TYPE(0);
    let mut frame = BOOL(0);
    // SAFETY : tampons de la taille annoncée.
    let (b, f) = unsafe {
        (
            DwmGetWindowAttribute(
                hwnd(h),
                DWMWA_SYSTEMBACKDROP_TYPE,
                &mut backdrop as *mut _ as *mut _,
                size_of_val(&backdrop) as u32,
            ),
            DwmGetWindowAttribute(
                hwnd(h),
                DWMWA_NCRENDERING_ENABLED,
                &mut frame as *mut _ as *mut _,
                size_of_val(&frame) as u32,
            ),
        )
    };
    let backdrop_back = b.is_ok() && backdrop != DWMSBT_NONE;
    let frame_back = f.is_ok() && frame.as_bool();
    if !backdrop_back && !frame_back {
        return false;
    }
    remove_frame(h);
    true
}

/// Retire ou remet `WS_EX_NOACTIVATE` (le mode édition a besoin du clavier).
pub fn set_activatable(h: isize, activatable: bool) {
    let hw = hwnd(h);
    // SAFETY : idem `make_tool_window`.
    unsafe {
        let ex = GetWindowLongPtrW(hw, GWL_EXSTYLE);
        let flag = WS_EX_NOACTIVATE.0 as isize;
        let new = if activatable { ex & !flag } else { ex | flag };
        SetWindowLongPtrW(hw, GWL_EXSTYLE, new);
    }
}

/// Enregistre la fenêtre comme appbar en haut de l'écran et la place. Renvoie le rectangle
/// accordé par Windows (il peut être décalé vers le bas si une autre barre occupe déjà le haut).
pub fn register(h: isize, wanted: Rect) -> anyhow::Result<Rect> {
    let hw = hwnd(h);
    let mut abd = data(hw);
    // SAFETY : APPBARDATA valide ; la sous-classe vit tant que la fenêtre n'est pas désenregistrée.
    unsafe {
        if SHAppBarMessage(ABM_NEW, &mut abd) == 0 {
            anyhow::bail!("ABM_NEW refusé");
        }
        if !SetWindowSubclass(hw, Some(subclass_proc), SUBCLASS_ID, 0).as_bool() {
            log::warn!("sous-classement de la barre impossible");
        }
    }
    if let Ok(mut r) = REGISTERED.lock() {
        r.push(h);
    }
    update(h, wanted)
}

/// Redemande la position (après un changement d'écran ou d'une autre barre).
pub fn update(h: isize, wanted: Rect) -> anyhow::Result<Rect> {
    let hw = hwnd(h);
    let mut abd = data(hw);
    abd.rc = RECT {
        left: wanted.x,
        top: wanted.y,
        right: wanted.x + wanted.w,
        bottom: wanted.y + wanted.h,
    };
    // SAFETY : APPBARDATA valide.
    unsafe {
        SHAppBarMessage(ABM_QUERYPOS, &mut abd);
        abd.rc.bottom = abd.rc.top + wanted.h;
        SHAppBarMessage(ABM_SETPOS, &mut abd);
        let r = abd.rc;
        SetWindowPos(
            hw,
            Some(HWND_TOPMOST),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE,
        )?;
        Ok(Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
    }
}

/// Libère la place de toutes les barres (crochet de panique : ne bloque jamais).
pub fn release_all() {
    let list = match REGISTERED.try_lock() {
        Ok(mut r) => std::mem::take(&mut *r),
        Err(_) => return,
    };
    for h in list {
        let mut abd = data(hwnd(h));
        // SAFETY : APPBARDATA valide ; sans effet si la fenêtre n'est plus une appbar.
        unsafe { SHAppBarMessage(ABM_REMOVE, &mut abd) };
    }
}

/// Libère la place réservée. À appeler avant de détruire la fenêtre.
pub fn unregister(h: isize) {
    if let Ok(mut r) = REGISTERED.lock() {
        r.retain(|x| *x != h);
    }
    let hw = hwnd(h);
    let mut abd = data(hw);
    // SAFETY : APPBARDATA valide ; retirer une sous-classe absente est sans effet.
    unsafe {
        SHAppBarMessage(ABM_REMOVE, &mut abd);
        let _ = RemoveWindowSubclass(hw, Some(subclass_proc), SUBCLASS_ID);
    }
}
