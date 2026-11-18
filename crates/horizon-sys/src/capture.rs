//! Captures d'écran vers une session Claude : dossier des captures de Windows, miniature,
//! chemin mis dans le presse-papiers et collé par Ctrl+V. Jamais Entrée : la personne relit
//! et envoie elle-même.

use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{GlobalFree, HANDLE};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_DEFAULT, SHGetKnownFolderPath};

/// Dossier des captures d'écran (Images\Captures d'écran, ou celui choisi par l'utilisateur).
pub fn screenshots_dir() -> Option<PathBuf> {
    // SAFETY : chaîne allouée par le système, copiée puis libérée.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Miniature PNG (`max` px au plus de côté) d'une image ; `None` si elle est illisible (encore
/// en cours d'écriture, par exemple).
pub fn thumbnail_png(path: &Path, max: u32) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?;
    let small = img.thumbnail(max, max);
    let mut out = std::io::Cursor::new(Vec::new());
    small.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Met un texte dans le presse-papiers.
pub fn set_clipboard_text(text: &str) -> anyhow::Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY : mémoire globale allouée, remplie sous verrou, confiée au presse-papiers (qui la
    // libère) ; libérée par nous seulement si le presse-papiers ne l'a pas prise.
    unsafe {
        OpenClipboard(None)?;
        let result = (|| -> anyhow::Result<()> {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let dst = GlobalLock(mem) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(mem));
                anyhow::bail!("mémoire du presse-papiers inaccessible");
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), dst, wide.len());
            let _ = GlobalUnlock(mem);
            if let Err(e) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0))) {
                let _ = GlobalFree(Some(mem));
                return Err(e.into());
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() },
        },
    }
}

/// Ctrl+V dans la fenêtre au premier plan. Seules ces deux touches partent (jamais Entrée).
pub fn paste() -> bool {
    let v = VIRTUAL_KEY(u16::from(b'V'));
    let inputs = [key(VK_CONTROL, false), key(v, false), key(v, true), key(VK_CONTROL, true)];
    // SAFETY : tableau d'entrées initialisées, taille de la structure annoncée.
    unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) as usize == inputs.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshots_folder_is_known() {
        // Dossier connu de Windows (pas forcément créé tant qu'aucune capture n'a été faite).
        // Les machines de CI (compte de service sans profil complet) n'en ont pas : la fonction
        // y renvoie `None`, ce que l'appelant gère déjà (pas de proposition de capture).
        let Some(d) = screenshots_dir() else {
            assert!(std::env::var_os("CI").is_some(), "dossier des captures inconnu sur un poste de travail");
            return;
        };
        assert!(d.is_absolute(), "{}", d.display());
    }

    #[test]
    fn thumbnail_of_a_png() {
        let dir = std::env::temp_dir().join(format!("horizon-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 255])).save(&p).unwrap();
        let png = thumbnail_png(&p, 80).unwrap();
        let back = image::load_from_memory(&png).unwrap();
        assert_eq!((back.width(), back.height()), (80, 40));
        std::fs::write(&p, b"pas une image").unwrap();
        assert!(thumbnail_png(&p, 80).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
