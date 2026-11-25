//! Décodage d'une pochette (PNG ou JPEG) en pixels RGBA, réduite pour l'analyse de couleur.

/// Pixels RGBA (largeur, hauteur, octets), image ramenée à 64 px au plus de côté.
pub fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(64, 64).to_rgba8();
    Some((small.width(), small.height(), small.into_raw()))
}

/// Pochette pour le téléphone : rognée au centre en carré, ramenée à `side` px au plus (jamais
/// agrandie), en JPEG de `max_bytes` au plus. La qualité baisse de 85 à 45 par pas de 10
/// jusqu'à tenir ; `None` si l'image est illisible ou ne tient pas même à 45.
pub fn jpeg_square(bytes: &[u8], side: u32, max_bytes: usize) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let edge = img.width().min(img.height());
    let square = img.crop_imm((img.width() - edge) / 2, (img.height() - edge) / 2, edge, edge);
    let small = if edge > side { square.resize_exact(side, side, image::imageops::FilterType::Triangle) } else { square };
    let rgb = small.to_rgb8();
    (45..=85).rev().step_by(10).find_map(|quality| {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb).ok()?;
        (out.len() <= max_bytes).then_some(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    /// PNG de bruit coloré (le pire cas pour le JPEG), `w` × `h`.
    fn noisy_png(w: u32, h: u32) -> Vec<u8> {
        let mut seed: u32 = 12345;
        let img = RgbImage::from_fn(w, h, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let [a, b, c, _] = seed.to_le_bytes();
            Rgb([a, b, c])
        });
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn a_big_cover_becomes_a_small_square_jpeg() {
        let jpeg = jpeg_square(&noisy_png(1200, 800), 300, 48 * 1024).expect("pochette encodée");
        assert!(jpeg.len() <= 48 * 1024, "{} octets", jpeg.len());
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "JPEG");
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (300, 300));
    }

    #[test]
    fn a_small_cover_is_not_enlarged() {
        let jpeg = jpeg_square(&noisy_png(64, 80), 300, 48 * 1024).unwrap();
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (64, 64));
    }

    #[test]
    fn garbage_and_impossible_budgets_are_refused() {
        assert_eq!(jpeg_square(b"pas une image", 300, 48 * 1024), None);
        assert_eq!(jpeg_square(&noisy_png(1200, 800), 300, 100), None);
    }
}
