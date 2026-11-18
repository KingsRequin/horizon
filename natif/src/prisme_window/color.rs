//! Couleurs : OKLCH → sRGB (teintes des effets, couleur de chaque appareil sur le plan), et
//! `#rrggbb`. Formules de Björn Ottosson (OKLab), comme le navigateur pour `oklch(…)`.

/// sRGB (0–255) d'une couleur `oklch(l c h)` (`h` en degrés), ramenée dans le gamut.
pub fn oklch(l: f64, c: f64, h_deg: f64) -> [u8; 3] {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s_ = l - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let (l3, m3, s3) = (l_.powi(3), m_.powi(3), s_.powi(3));
    let lin = [
        4.076_741_662_1 * l3 - 3.307_711_591_3 * m3 + 0.230_969_929_2 * s3,
        -1.268_438_004_6 * l3 + 2.609_757_401_1 * m3 - 0.341_319_396_5 * s3,
        -0.004_196_086_3 * l3 - 0.703_418_614_7 * m3 + 1.707_614_701 * s3,
    ];
    lin.map(|v| (gamma(v.clamp(0.0, 1.0)) * 255.0).round() as u8)
}

fn gamma(v: f64) -> f64 {
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Couleur d'une teinte exprimée en tours, à la clarté et la saturation des effets
/// (`hueCss` d'effects.ts : `oklch(0.72 0.17 h)`).
pub fn hue(turns: f64) -> [u8; 3] {
    oklch(0.72, 0.17, turns * 360.0)
}

/// Couleur d'identification d'un appareil sur le plan (`deviceColor` de plan.ts).
pub fn device(i: usize) -> [u8; 3] {
    const HUES: [f64; 8] = [250.0, 160.0, 30.0, 320.0, 95.0, 200.0, 0.0, 280.0];
    oklch(0.78, 0.13, HUES[i % HUES.len()])
}

/// `#rrggbb` → sRGB (`None` si illisible).
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_owned(),
        _ => return None,
    };
    let n = u32::from_str_radix(&full, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklch_matches_known_colours() {
        // oklch(0.628 0.2577 29.23) = rouge pur ; oklch(1 0 0) = blanc.
        let red = oklch(0.627_955, 0.257_683, 29.234);
        assert!(red[0] >= 254 && red[1] <= 2 && red[2] <= 2, "{red:?}");
        assert_eq!(oklch(1.0, 0.0, 0.0), [255, 255, 255]);
        assert_eq!(oklch(0.0, 0.0, 0.0), [0, 0, 0]);
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(parse_hex("#ffb36b"), Some([0xff, 0xb3, 0x6b]));
        assert_eq!(parse_hex("#fff"), Some([255, 255, 255]));
        assert_eq!(parse_hex("rouge"), None);
        assert_eq!(hex([0xff, 0xb3, 0x6b]), "#ffb36b");
    }
}
