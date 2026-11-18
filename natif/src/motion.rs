//! Mouvement : réglage Windows « Effets d'animation » (ce que WebView2 lisait comme
//! `prefers-reduced-motion`), et la courbe `ease-out` d'app.css pour les fondus menés par Rust.

use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
};
use windows::core::BOOL;

/// L'utilisateur a coupé les effets d'animation de Windows (`prefers-reduced-motion: reduce`).
pub fn reduced() -> bool {
    let mut on = BOOL(1);
    // SAFETY : lecture d'un BOOL dans une variable locale.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_ok() && !on.as_bool()
}

/// `cubic-bezier(x1, y1, x2, y2)` au temps `t` (0–1) : progression (0–1).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t == 0.0 || t == 1.0 {
        return t;
    }
    let curve = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    // Paramètre de la courbe dont l'abscisse vaut `t` (dichotomie : la courbe est croissante en x).
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if curve(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(y1, y2, (lo + hi) / 2.0)
}

/// `--ease-out` d'app.css : cubic-bezier(0.2, 0.8, 0.2, 1).
pub fn ease_out(t: f32) -> f32 {
    cubic_bezier(0.2, 0.8, 0.2, 1.0, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_starts_fast_and_ends_at_one() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert_eq!(ease_out(2.0), 1.0);
        assert!(ease_out(0.25) > 0.6, "{}", ease_out(0.25));
        let mut last = 0.0;
        for i in 1..=20 {
            let v = ease_out(i as f32 / 20.0);
            assert!(v >= last, "croissante");
            last = v;
        }
        // Courbe linéaire : identité.
        assert!((cubic_bezier(0.0, 0.0, 1.0, 1.0, 0.3) - 0.3).abs() < 1e-3);
    }
}
