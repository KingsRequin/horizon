//! Décodage d'une pochette (PNG ou JPEG) en pixels RGBA, réduite pour l'analyse de couleur.

/// Pixels RGBA (largeur, hauteur, octets), image ramenée à 64 px au plus de côté.
pub fn decode_rgba(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(64, 64).to_rgba8();
    Some((small.width(), small.height(), small.into_raw()))
}
