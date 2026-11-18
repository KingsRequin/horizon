//! Petits utilitaires Win32 partagés par les modules de horizon-sys.

use horizon_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowRect};

pub(crate) fn to_rect(r: RECT) -> Rect {
    Rect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

/// Rectangle d'une fenêtre, bords invisibles compris (pixels physiques pour un fil conscient
/// de l'échelle de chaque écran).
pub(crate) fn window_rect(h: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY : `r` valide en écriture.
    unsafe { GetWindowRect(h, &mut r) }.ok().map(|_| to_rect(r))
}

/// Cadre visible d'une fenêtre (sans les bords invisibles de redimensionnement), à défaut son
/// rectangle.
pub(crate) fn frame_bounds(h: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY : tampon RECT de la taille annoncée.
    let dwm = unsafe {
        DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size_of::<RECT>() as u32)
    };
    if dwm.is_err() {
        return window_rect(h);
    }
    Some(to_rect(r))
}

/// Texte UTF-16 d'un tampon, jusqu'au premier zéro (ou tout le tampon).
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Texte en UTF-16 terminé par un zéro, pour une API Win32.
pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Classe d'une fenêtre.
pub(crate) fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY : tampon de la taille annoncée.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// Fenêtre masquée par le système (autre bureau virtuel, appli suspendue).
pub(crate) fn is_cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    // SAFETY : tampon u32 de la taille annoncée.
    let ok = unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut _ as *mut _, size_of::<u32>() as u32) };
    ok.is_ok() && v != 0
}

/// Millisecondes écoulées depuis la dernière saisie (clavier, souris) de la session.
pub(crate) fn input_idle_ms() -> u64 {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY : structure de la taille annoncée.
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return 0;
    }
    // Compteurs 32 bits qui reviennent à zéro tous les 49 jours : différence modulaire.
    // SAFETY : simple lecture du compteur.
    u64::from(unsafe { GetTickCount() }.wrapping_sub(info.dwTime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_round_trip_and_stop_at_nul() {
        let w = to_wide("Écran 2");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w), "Écran 2");
        let mut buf = [0u16; 8];
        buf[..2].copy_from_slice(&[u16::from(b'a'), u16::from(b'b')]);
        assert_eq!(from_wide(&buf), "ab");
        assert_eq!(from_wide(&[u16::from(b'x'); 3]), "xxx", "sans zéro final : tout le tampon");
    }
}
