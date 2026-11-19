//! Une seule instance : un mutex nommé dit si Horizon tourne déjà ; un second lancement
//! transmet ses arguments à la première instance (`WM_COPYDATA` à sa fenêtre de messages) et
//! s'arrête. `horizon.exe --quit` ferme ainsi l'instance en cours (comme
//! `tauri-plugin-single-instance` pour la version Tauri).

use crate::host::AppHandle;
use std::cell::RefCell;
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, FindWindowExW, HWND_MESSAGE, RegisterClassW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WINDOW_EX_STYLE, WM_COPYDATA, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

const MUTEX: PCWSTR = w!("Local\\HorizonNatif.Instance");
const CLASS: PCWSTR = w!("HorizonNatifInstance");
/// Marque des messages d'arguments (« HN »).
const TAG: usize = 0x484E;

/// Mutex de l'instance, tenu jusqu'à la fin.
pub struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        // SAFETY : poignée créée par `acquire`.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Première instance : le mutex. Sinon `None` : les arguments ont été transmis à l'instance en
/// cours, ce lancement doit s'arrêter.
pub fn acquire(args: &[String]) -> anyhow::Result<Option<Guard>> {
    // SAFETY : nom constant ; erreur lue juste après l'appel.
    let (handle, existed) = unsafe {
        let h = CreateMutexW(None, false, MUTEX)?;
        (h, GetLastError() == ERROR_ALREADY_EXISTS)
    };
    let guard = Guard(handle);
    if !existed {
        return Ok(Some(guard));
    }
    forward(args);
    Ok(None)
}

fn forward(args: &[String]) {
    let data = encode(args);
    // La première instance peut être en train de démarrer : quelques essais.
    for _ in 0..20 {
        // SAFETY : recherche parmi les fenêtres « message seulement ».
        if let Ok(target) = unsafe { FindWindowExW(Some(HWND_MESSAGE), None, CLASS, PCWSTR::null()) } {
            let cds = COPYDATASTRUCT { dwData: TAG, cbData: (data.len() * 2) as u32, lpData: data.as_ptr() as *mut _ };
            // SAFETY : `cds` et `data` vivent pendant l'envoi synchrone.
            unsafe {
                SendMessageTimeoutW(target, WM_COPYDATA, WPARAM(0), LPARAM(&cds as *const _ as isize), SMTO_ABORTIFHUNG, 5000, None);
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    log::warn!("instance en cours introuvable : arguments {args:?} perdus");
}

/// Arguments séparés par un caractère nul, en UTF-16.
fn encode(args: &[String]) -> Vec<u16> {
    args.join("\0").encode_utf16().collect()
}

fn decode(units: &[u16]) -> Vec<String> {
    String::from_utf16_lossy(units).split('\0').filter(|a| !a.is_empty()).map(str::to_owned).collect()
}

thread_local! {
    static APP: RefCell<Option<AppHandle>> = const { RefCell::new(None) };
}

/// Fenêtre qui reçoit les arguments des lancements suivants (fil de l'interface).
pub fn listen(app: &AppHandle) -> anyhow::Result<()> {
    APP.with_borrow_mut(|a| *a = Some(app.clone()));
    // SAFETY : classe et fenêtre « message seulement » de ce fil.
    unsafe {
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: GetModuleHandleW(None)?.into(), lpszClassName: CLASS, ..Default::default() };
        RegisterClassW(&wc);
        CreateWindowExW(WINDOW_EX_STYLE(0), CLASS, PCWSTR::null(), WS_OVERLAPPED, 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None)?;
    }
    Ok(())
}

/// Arguments d'un second lancement.
fn received(args: &[String]) {
    log::info!("second lancement : {args:?}");
    let Some(app) = APP.with_borrow(Clone::clone) else { return };
    if args.iter().any(|a| a == "--quit") {
        app.exit(0);
    }
    // Alerte d'essai de l'îlot dynamique (`--alerte-essai <genre>`).
    if let Some(i) = args.iter().position(|a| a == "--alerte-essai") {
        crate::alerts::essai(&app, args.get(i + 1).map_or("", String::as_str));
    }
    // Fenêtre des réglages (`--reglages [page]`) et palette d'essai, sans action
    // (`--palette-essai`).
    if let Some(i) = args.iter().position(|a| a == "--reglages") {
        let section = args.get(i + 1).filter(|s| !s.starts_with("--")).map_or("barres", String::as_str);
        // `widget:<id>`, `profiles` ou une section des « Réglages d'Horizon ».
        let page = crate::settings_window::SettingsPage::parse(section)
            .unwrap_or_else(|| crate::settings_window::SettingsPage::Options(section.to_owned()));
        crate::settings_window::open(&app, page);
    }
    if args.iter().any(|a| a == "--palette-essai") {
        crate::palette::open_essai(&app);
    }
    // Essai du mode édition et des menus de la barre principale (`--edition-essai <quoi>`).
    if let Some(i) = args.iter().position(|a| a == "--edition-essai") {
        crate::ui::edit::essai(&app, args.get(i + 1).map_or("", String::as_str));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        // SAFETY : Windows passe une COPYDATASTRUCT valide pendant le message.
        let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if cds.dwData == TAG && !cds.lpData.is_null() {
            // SAFETY : `cbData` octets à `lpData`, des unités UTF-16 (voir `forward`).
            let units = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
            let args = decode(units);
            // Hors du message : l'expéditeur attend sa fin, la sortie peut prendre un moment.
            let _ = slint::invoke_from_event_loop(move || received(&args));
            return LRESULT(1);
        }
    }
    // SAFETY : traitement par défaut.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_round_trip() {
        let args = vec!["--quit".to_owned(), "C:\\été\\x".to_owned()];
        assert_eq!(decode(&encode(&args)), args);
        assert!(decode(&encode(&[])).is_empty());
    }
}
