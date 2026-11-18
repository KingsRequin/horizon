//! Presse-papiers Windows (texte), pour les champs de saisie de Slint (Ctrl+C, Ctrl+V).

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Presse-papiers ouvert, refermé à la fin de la portée.
struct Open;

impl Open {
    fn new() -> Option<Open> {
        // Un autre programme peut le tenir un instant : quelques essais.
        for _ in 0..5 {
            // SAFETY : sans fenêtre propriétaire.
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Some(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY : ouvert par `new`.
        let _ = unsafe { CloseClipboard() };
    }
}

pub fn text() -> Option<String> {
    let _open = Open::new()?;
    // SAFETY : presse-papiers ouvert ; mémoire verrouillée le temps de la copie.
    unsafe {
        let data = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
        let mem = HGLOBAL(data.0);
        let ptr = GlobalLock(mem) as *const u16;
        if ptr.is_null() {
            return None;
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(mem);
        Some(s)
    }
}

pub fn set_text(text: &str) {
    let Some(_open) = Open::new() else { return };
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY : presse-papiers ouvert ; bloc alloué, rempli, puis confié à Windows (ou libéré).
    unsafe {
        let _ = EmptyClipboard();
        let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return };
        let ptr = GlobalLock(mem) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(mem));
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(mem);
        if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))).is_err() {
            let _ = GlobalFree(Some(mem));
        }
    }
}
