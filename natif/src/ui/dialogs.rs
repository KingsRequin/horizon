//! Boîtes de dialogue de Windows pour les fenêtres de réglages : « Ouvrir » (`<input type=file>`
//! et `dialog().file()` de la version Tauri) et sélecteur de couleurs (`<input type=color>`).
//! Modales : appelées sur le fil de l'interface, hors d'un rappel Slint en cours (minuteur nul),
//! elles font tourner leur propre boucle de messages.

use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW, GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR,
    OFN_NODEREFERENCELINKS, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows::core::PCWSTR;

/// Extensions d'images acceptées pour l'icône d'un raccourci (comme `shortcuts_pick_file`).
pub const IMAGE_EXTENSIONS: [&str; 6] = ["png", "svg", "ico", "jpg", "jpeg", "webp"];

/// Filtre de la boîte « Ouvrir » (paires libellé / motifs, terminées par deux zéros).
fn filter(images: bool) -> Vec<u16> {
    let text = if images {
        let patterns: Vec<String> = IMAGE_EXTENSIONS.iter().map(|e| format!("*.{e}")).collect();
        format!("Images\0{}\0", patterns.join(";"))
    } else {
        "Tous les fichiers\0*.*\0".to_owned()
    };
    text.encode_utf16().chain([0]).collect()
}

/// Boîte « Ouvrir » : une image (`images`) ou n'importe quel fichier ; un raccourci `.lnk` est
/// rendu tel quel (pas sa cible). `None` : annulée.
pub fn pick_file(owner: isize, images: bool) -> Option<String> {
    let filter = filter(images);
    let mut buf = vec![0u16; 4096];
    let mut ofn = OPENFILENAMEW {
        lStructSize: size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as *mut _),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_NODEREFERENCELINKS,
        ..Default::default()
    };
    // SAFETY : structure initialisée ; le filtre et le tampon vivent pendant le dialogue.
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len])).filter(|p| !p.is_empty())
}

/// Sélecteur de couleurs de Windows, partant de `rgb` ; `None` : annulé.
pub fn choose_color(owner: isize, [r, g, b]: [u8; 3]) -> Option<[u8; 3]> {
    thread_local! {
        /// Couleurs personnalisées du dialogue, gardées d'une ouverture à l'autre.
        static CUSTOM: RefCell<[COLORREF; 16]> = const { RefCell::new([COLORREF(0xffffff); 16]) };
    }
    CUSTOM.with_borrow_mut(|custom| {
        let mut cc = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: HWND(owner as *mut _),
            rgbResult: COLORREF(r as u32 | (g as u32) << 8 | (b as u32) << 16),
            lpCustColors: custom.as_mut_ptr(),
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        // SAFETY : structure initialisée ; tableau de 16 couleurs vivant pendant le dialogue.
        let ok = unsafe { ChooseColorW(&mut cc) }.as_bool();
        let v = cc.rgbResult.0;
        ok.then_some([(v & 0xff) as u8, (v >> 8 & 0xff) as u8, (v >> 16 & 0xff) as u8])
    })
}

/// Couleur `#rrggbb` (valeur d'un `<input type=color>`).
pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_end_with_two_zeros() {
        let f = String::from_utf16(&filter(true)).unwrap();
        assert_eq!(f, "Images\0*.png;*.svg;*.ico;*.jpg;*.jpeg;*.webp\0\0");
        assert!(String::from_utf16(&filter(false)).unwrap().ends_with("*.*\0\0"));
        assert_eq!(hex([255, 0, 16]), "#ff0010");
    }
}
