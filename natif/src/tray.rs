//! Icône de la zone de notification : réglages, mode édition, palette, « Ne pas déranger »,
//! rechargement, démarrage avec Windows, journal, quitter. Un point orange sur l'icône (et une
//! info-bulle) signale une intégration en échec (page « État » des réglages).
//!
//! Équivalent de `app/src-tauri/src/tray.rs`, en Win32 (`Shell_NotifyIconW`, menu
//! `TrackPopupMenu`) au lieu du `TrayIconBuilder` de Tauri ; le menu est construit à chaque
//! ouverture (cases et raccourci de la palette toujours à jour).

use crate::host::AppHandle;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
    DestroyWindow, GetCursorPos, GetSystemMetrics, HICON, HWND_MESSAGE, ICONINFO, MF_CHECKED, MF_SEPARATOR, MF_STRING,
    PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SetForegroundWindow, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP,
    WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{HSTRING, PCWSTR, w};

const TRAY_ID: u32 = 1;
/// Message des clics sur l'icône.
const WM_TRAY: u32 = WM_APP + 1;
const CMD_RELOAD: usize = 1;
const CMD_LOG: usize = 2;
const CMD_QUIT: usize = 3;
const CMD_EDIT: usize = 4;
const CMD_OPTIONS: usize = 5;
const CMD_PALETTE: usize = 6;
const CMD_DND: usize = 7;
const CMD_AUTOSTART: usize = 8;

/// Horizon démarre avec Windows (registre).
pub fn autostart_enabled() -> bool {
    horizon_sys::autostart::is_enabled()
}

/// Active ou coupe le démarrage avec Windows. Le registre fait foi : l'état affiché est relu,
/// même si l'écriture a échoué.
pub fn set_autostart(on: bool) -> Result<(), String> {
    horizon_sys::autostart::set(on).map_err(|e| e.to_string())
}

/// Icône d'Horizon (RGBA) aux tailles d'une petite icône, de 100 à 300 % d'échelle : côté en
/// pixels et PNG. Chaque taille est dessinée calée sur le pixel (`scripts/icones.ps1`), ce
/// qu'une réduction par Windows d'une seule image ne donnerait pas.
const ICONS: [(u32, &[u8]); 6] = [
    (16, include_bytes!("../icons/horizon-16.png")),
    (20, include_bytes!("../icons/horizon-20.png")),
    (24, include_bytes!("../icons/horizon-24.png")),
    (32, include_bytes!("../icons/horizon-32.png")),
    (40, include_bytes!("../icons/horizon-40.png")),
    (48, include_bytes!("../icons/horizon-48.png")),
];

/// PNG de la plus petite icône qui couvre `side` pixels ; la plus grande au-delà.
fn icon_png(side: u32) -> &'static [u8] {
    ICONS.iter().find(|(size, _)| *size >= side).unwrap_or(&ICONS[ICONS.len() - 1]).1
}

struct Tray {
    app: AppHandle,
    hwnd: HWND,
    /// Icône d'origine, sans témoin : largeur, hauteur, RGBA.
    icon: (u32, u32, Vec<u8>),
    hicon: HICON,
    tooltip: String,
}

thread_local! {
    /// Icône du fil de l'interface (sa fenêtre de messages y vit).
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

/// Le raccourci de la palette a changé (le menu, refait à chaque ouverture, le montre déjà).
pub fn sync_palette(app: &AppHandle) {
    log::debug!("menu de l'icône : palette {}", crate::palette::hotkey_status(app).0);
}

/// Libellé du menu qui ouvre la palette, avec son raccourci (aligné à droite par Windows).
pub fn palette_label(hotkey: &str) -> String {
    if hotkey.is_empty() { "Palette de commandes".into() } else { format!("Palette de commandes\t{hotkey}") }
}

/// Message envoyé par l'Explorateur quand il redémarre : l'icône est à remettre.
fn taskbar_created() -> u32 {
    // SAFETY : chaîne constante terminée par zéro.
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

/// Crée l'icône (fil de l'interface).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    // SAFETY : lecture d'une mesure du système.
    let side = unsafe { GetSystemMetrics(SM_CXSMICON) };
    let icon = decode_png(icon_png(side.max(0) as u32))?;
    let hicon = hicon_from_rgba(icon.0, icon.1, &icon.2)?;
    // SAFETY : classe et fenêtre « message seulement » de ce fil.
    let hwnd = unsafe {
        let class = w!("HorizonNatifTray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(WINDOW_EX_STYLE(0), class, PCWSTR::null(), WS_OVERLAPPED, 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)?
    };
    let tray = Tray { app: app.clone(), hwnd, icon, hicon, tooltip: "Horizon".into() };
    notify(&tray, NIM_ADD)?;
    TRAY.with_borrow_mut(|t| *t = Some(tray));
    Ok(())
}

/// Retire l'icône (fin d'Horizon).
pub fn stop() {
    if let Some(t) = TRAY.with_borrow_mut(Option::take) {
        let _ = notify(&t, NIM_DELETE);
        // SAFETY : objets créés par `start`.
        unsafe {
            let _ = DestroyIcon(t.hicon);
            let _ = DestroyWindow(t.hwnd);
        }
    }
}

fn notify(t: &Tray, action: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE) -> anyhow::Result<()> {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: t.hwnd,
        uID: TRAY_ID,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: t.hicon,
        ..Default::default()
    };
    let tip: Vec<u16> = t.tooltip.encode_utf16().take(nid.szTip.len() - 1).collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    // SAFETY : structure initialisée avec sa taille.
    unsafe {
        if !Shell_NotifyIconW(action, &nid).as_bool() {
            anyhow::bail!("Shell_NotifyIconW refusé");
        }
        if action == NIM_ADD {
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }
    Ok(())
}

/// Témoin : point orange en bas à droite de l'icône et info-bulle, tant que `failing` n'est
/// pas vide. Appelable de n'importe quel fil.
pub fn set_health(app: &AppHandle, failing: &[String]) {
    let tooltip = match failing.len() {
        0 => "Horizon".to_owned(),
        1 => format!("Horizon — en échec : {}", failing[0]),
        n => format!("Horizon — {n} intégrations en échec"),
    };
    let dot = !failing.is_empty();
    let _ = app.run_on_main_thread(move || {
        TRAY.with_borrow_mut(|t| {
            let Some(t) = t.as_mut() else { return };
            let (w, h, rgba) = &t.icon;
            let pixels = if dot { with_dot(*w, *h, rgba) } else { rgba.clone() };
            if let Ok(icon) = hicon_from_rgba(*w, *h, &pixels) {
                // SAFETY : ancienne icône créée par nous, remplacée juste après.
                unsafe {
                    let _ = DestroyIcon(t.hicon);
                }
                t.hicon = icon;
            }
            t.tooltip = tooltip;
            let _ = notify(t, NIM_MODIFY);
        })
    });
}

/// Copie de l'icône avec un disque orange (bord sombre) dans le coin inférieur droit.
fn with_dot(w: u32, h: u32, icon: &[u8]) -> Vec<u8> {
    let mut rgba = icon.to_vec();
    let r = (w.min(h) as f32 * 0.22).max(2.0);
    let (cx, cy) = (w as f32 - r - 0.5, h as f32 - r - 0.5);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let px = [0xfa, 0xb3, 0x87, 0xff];
            let color = if d <= r - 1.0 {
                Some(px)
            } else if d <= r + 0.6 {
                Some([0x11, 0x11, 0x1b, 0xff])
            } else {
                None
            };
            if let Some(c) = color {
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&c);
            }
        }
    }
    rgba
}

fn decode_png(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| anyhow::anyhow!("PNG trop grand"))?];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    anyhow::ensure!(info.color_type == png::ColorType::Rgba, "icône : RGBA attendu");
    Ok((info.width, info.height, buf))
}

/// Icône Windows depuis des pixels RGBA (non prémultipliés).
fn hicon_from_rgba(w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<HICON> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY : bitmaps créés, remplis (w × h pixels de 4 octets), confiés à CreateIconIndirect
    // (qui les copie) puis détruits.
    unsafe {
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let px = std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize);
        for (d, s) in px.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
        }
        let mask = CreateBitmap(w as i32, h as i32, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() });
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        Ok(icon?)
    }
}

/// Menu de l'icône ; renvoie la commande choisie.
fn menu(hwnd: HWND) -> usize {
    let Some(app) = TRAY.with_borrow(|t| t.as_ref().map(|t| t.app.clone())) else { return 0 };
    let entries: [(usize, String, Option<bool>); 10] = [
        (CMD_OPTIONS, "Réglages d’Horizon…".into(), None),
        (CMD_EDIT, "Modifier la barre (écran principal)".into(), None),
        (CMD_PALETTE, palette_label(&crate::palette::hotkey_status(&app).0), None),
        (CMD_DND, "Ne pas déranger".into(), Some(crate::dnd::is_active(&app))),
        (0, String::new(), None),
        (CMD_RELOAD, "Recharger les barres".into(), None),
        (CMD_AUTOSTART, "Démarrer avec Windows".into(), Some(autostart_enabled())),
        (CMD_LOG, "Ouvrir le journal".into(), None),
        (0, String::new(), None),
        (CMD_QUIT, "Quitter Horizon".into(), None),
    ];
    // SAFETY : menu créé, montré puis détruit ici ; fenêtre de ce fil.
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        for (id, text, checked) in &entries {
            if *id == 0 {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                continue;
            }
            let flags = if *checked == Some(true) { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(m, flags, *id, &HSTRING::from(text.as_str()));
        }
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // Sans premier plan, le menu ne se ferme pas d'un clic ailleurs.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, p.x, p.y, None, hwnd, None);
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        cmd.0 as usize
    }
}

fn run(cmd: usize) {
    let Some(app) = TRAY.with_borrow(|t| t.as_ref().map(|t| t.app.clone())) else { return };
    match cmd {
        CMD_OPTIONS => crate::settings_window::open(&app, crate::settings_window::SettingsPage::Options("barres".into())),
        CMD_EDIT => {
            if let Err(e) = crate::edit::edit_main_bar(&app) {
                log::warn!("modifier la barre : {e}");
            }
        }
        CMD_PALETTE => crate::palette::open(&app),
        CMD_DND => crate::dnd::toggle(&app),
        CMD_RELOAD => crate::bars::rebuild(&app),
        CMD_AUTOSTART => {
            if let Err(e) = set_autostart(!autostart_enabled()) {
                log::warn!("démarrage avec Windows : {e}");
            }
            let _ = crate::host::Emitter::emit(&app, crate::events::EV_OPTIONS, ());
        }
        CMD_LOG => crate::status::open_log(),
        CMD_QUIT => app.exit(0),
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        // Version 4 : l'événement est dans le mot bas de lParam.
        let event = (lparam.0 & 0xffff) as u32;
        if event == WM_CONTEXTMENU || event == WM_RBUTTONUP || event == WM_LBUTTONUP {
            run(menu(hwnd));
        }
        return LRESULT(0);
    }
    if msg == taskbar_created() {
        TRAY.with_borrow(|t| {
            if let Some(t) = t {
                let _ = notify(t, NIM_ADD);
            }
        });
        return LRESULT(0);
    }
    // SAFETY : traitement par défaut.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dot_is_drawn_in_the_corner() {
        let icon = vec![0; 32 * 32 * 4];
        let dotted = with_dot(32, 32, &icon);
        let at = |x: u32, y: u32| dotted[((y * 32 + x) * 4) as usize..][..4].to_vec();
        assert_eq!(at(0, 0), vec![0, 0, 0, 0], "le reste de l'icône est intact");
        assert_eq!(at(25, 25), vec![0xfa, 0xb3, 0x87, 0xff]);
        assert_eq!(palette_label(""), "Palette de commandes");
        assert_eq!(palette_label("Ctrl+Alt+P"), "Palette de commandes\tCtrl+Alt+P");
    }

    #[test]
    fn embedded_icons_decode_at_their_size() {
        for (size, png) in ICONS {
            let (w, h, rgba) = decode_png(png).unwrap();
            assert_eq!((w, h, rgba.len() as u32), (size, size, size * size * 4));
        }
    }

    #[test]
    fn icon_follows_the_small_icon_size() {
        let side = |want: u32| decode_png(icon_png(want)).unwrap().0;
        assert_eq!(side(16), 16, "100 %");
        assert_eq!(side(20), 20, "125 %");
        assert_eq!(side(18), 20, "entre deux tailles : la suivante, que Windows réduit");
        assert_eq!(side(64), 48, "au-delà : la plus grande");
        assert_eq!(side(0), 16);
    }
}
