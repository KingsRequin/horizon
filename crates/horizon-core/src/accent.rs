//! Couleur d'accent tirée de la pochette : teinte dominante parmi les pixels colorés.

/// Couleur dominante d'une image RGBA (`w` × `h`). `None` si l'image est grise, noire ou blanche.
pub fn dominant_color(rgba: &[u8], w: u32, h: u32) -> Option<[u8; 3]> {
    const BINS: usize = 36;
    const GRID: u32 = 32;
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return None;
    }
    // Par case de teinte : poids total et somme pondérée des couleurs.
    let mut weight = [0f32; BINS];
    let mut sum = [[0f32; 3]; BINS];
    let (sx, sy) = (w.div_ceil(GRID as usize), h.div_ceil(GRID as usize));
    for y in (0..h).step_by(sy) {
        for x in (0..w).step_by(sx) {
            let i = (y * w + x) * 4;
            let px = [rgba[i], rgba[i + 1], rgba[i + 2]];
            let (hue, s, v) = to_hsv(px);
            // Pixels presque gris, noirs ou blancs : ignorés.
            if v < 0.15 || s < 0.2 {
                continue;
            }
            let bin = ((hue / 360.0 * BINS as f32) as usize).min(BINS - 1);
            let wgt = s * v;
            weight[bin] += wgt;
            for c in 0..3 {
                sum[bin][c] += px[c] as f32 * wgt;
            }
        }
    }
    let (best, &total) = weight.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    (total > 0.0).then(|| sum[best].map(|c| (c / total).round().clamp(0.0, 255.0) as u8))
}

/// Part de pixels transparents au-delà de laquelle une image est une icône détourée et non une
/// pochette. Une pochette est opaque ; le logo que Chrome, Vivaldi ou Sigma publient quand la
/// page n'en fournit pas a un fond transparent (6 % pour le plus plein des trois, Sigma).
const ICON_TRANSPARENCY: f32 = 0.03;

/// L'image RGBA (`w` × `h`) est une icône détourée (fond transparent), pas une pochette.
pub fn is_cutout(rgba: &[u8], w: u32, h: u32) -> bool {
    let n = w as usize * h as usize;
    if n == 0 || rgba.len() < n * 4 {
        return false;
    }
    let clear = rgba[..n * 4].chunks_exact(4).filter(|px| px[3] < 32).count();
    clear as f32 > n as f32 * ICON_TRANSPARENCY
}

/// Couleur d'accent lisible d'une pochette RGBA (`w` × `h`). `None` si elle est grise, noire ou
/// blanche, ou si ce n'est pas une pochette mais le logo d'une appli (`is_cutout`) : sa couleur
/// ne dit rien du morceau.
pub fn cover_accent(rgba: &[u8], w: u32, h: u32) -> Option<[u8; 3]> {
    if is_cutout(rgba, w, h) {
        return None;
    }
    dominant_color(rgba, w, h).map(readable_accent)
}

fn to_hsv([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (hue, s, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|u| ((u + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// Couleur assez lumineuse et saturée pour se voir sur une barre sombre.
pub fn readable_accent(rgb: [u8; 3]) -> [u8; 3] {
    const MIN_V: f32 = 0.75;
    const MIN_S: f32 = 0.45;
    let (h, s, v) = to_hsv(rgb);
    if v >= MIN_V && s >= MIN_S {
        return rgb;
    }
    from_hsv(h, s.max(MIN_S), v.max(MIN_V))
}

/// Sources de la couleur d'accent : pochette de la lecture en cours et fond d'écran.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AccentSources {
    /// Couleur de la pochette en cours (lisible), s'il y en a une.
    pub cover: Option<[u8; 3]>,
    /// La lecture est en marche.
    pub playing: bool,
    /// Couleur du fond d'écran (lisible), si la fonction est active et la capture réussie.
    pub wallpaper: Option<[u8; 3]>,
}

impl AccentSources {
    /// Accent des barres : la pochette pendant la lecture, sinon le fond d'écran, à défaut la
    /// dernière pochette (comportement d'avant le fond d'écran) ; `None` = orange par défaut.
    pub fn bars(&self) -> Option<[u8; 3]> {
        match (self.playing, self.cover) {
            (true, Some(c)) => Some(c),
            _ => self.wallpaper.or(self.cover),
        }
    }

    /// Accent de Prisme : la pochette pendant la lecture (scène musique), sinon le fond d'écran
    /// (scène d'ambiance) ; `None` = palette d'origine de Prisme.
    pub fn prisme(&self) -> Option<[u8; 3]> {
        if self.playing { self.cover } else { self.wallpaper }
    }
}

/// Deux couleurs assez proches pour ne pas valoir un changement (teinte à moins de 14°,
/// saturation et luminosité à moins de 0,15) : le fond d'écran animé ne fait pas clignoter
/// l'accent d'une capture à l'autre.
pub fn same_accent(a: [u8; 3], b: [u8; 3]) -> bool {
    let ((ha, sa, va), (hb, sb, vb)) = (to_hsv(a), to_hsv(b));
    let dh = (ha - hb).abs();
    dh.min(360.0 - dh) < 14.0 && (sa - sb).abs() < 0.15 && (va - vb).abs() < 0.15
}

/// Palette d'accent de Windows (`AccentPalette`, 8 × RGBA) tirée d'une couleur : du plus clair
/// au plus sombre (clair 3, 2, 1, accent, sombre 1, 2, 3), puis une case inutilisée. Les
/// luminosités (TSL) sont celles de la palette bleue d'origine de Windows.
pub fn windows_palette(rgb: [u8; 3]) -> [[u8; 4]; 8] {
    const LIGHTNESS: [f32; 7] = [0.80, 0.65, 0.49, 0.42, 0.38, 0.29, 0.20];
    let (h, s, _) = to_hsl(rgb);
    // Saturation bornée : ni gris délavé, ni couleur criarde dans les menus de Windows.
    let s = s.clamp(0.55, 0.95);
    let mut out = [[0, 0, 0, 0xFF]; 8];
    for (slot, l) in out.iter_mut().zip(LIGHTNESS) {
        let [r, g, b] = from_hsl(h, s, l);
        *slot = [r, g, b, 0xFF];
    }
    out[7] = [0, 0, 0, 0];
    out
}

fn to_hsl(rgb: [u8; 3]) -> (f32, f32, f32) {
    let (h, _, _) = to_hsv(rgb);
    let [r, g, b] = rgb.map(|c| c as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    let s = if d == 0.0 { 0.0 } else { d / (1.0 - (2.0 * l - 1.0).abs()) };
    (h, s.clamp(0.0, 1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    // TSL → TSV : même teinte, valeur et saturation recalculées.
    let v = l + c / 2.0;
    let sv = if v == 0.0 { 0.0 } else { c / v };
    from_hsv(h, sv, v)
}

/// N'envoie une couleur (à Prisme) que si elle a changé : `null` n'est envoyé qu'une fois.
#[derive(Debug, Default)]
pub struct AccentSender {
    last: Option<Option<[u8; 3]>>,
}

impl AccentSender {
    /// `Some(valeur)` s'il faut envoyer `valeur`, `None` si c'est la même que la dernière.
    pub fn next(&mut self, c: Option<[u8; 3]>) -> Option<Option<[u8; 3]>> {
        if self.last == Some(c) {
            return None;
        }
        self.last = Some(c);
        Some(c)
    }

    /// Après une reconnexion : la prochaine valeur est renvoyée même si elle n'a pas changé.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = f(x, y);
                v.extend_from_slice(&[r, g, b, 255]);
            }
        }
        v
    }

    fn close(a: [u8; 3], b: [u8; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= 12)
    }

    #[test]
    fn dominant_color_of_solid_orange() {
        let img = image(64, 64, |_, _| [249, 115, 22]);
        assert!(close(dominant_color(&img, 64, 64).unwrap(), [249, 115, 22]));
    }

    #[test]
    fn dominant_color_ignores_black_borders() {
        // Cadre noir épais autour d'un carré bleu.
        let img = image(100, 100, |x, y| {
            if (20..80).contains(&x) && (20..80).contains(&y) { [40, 90, 230] } else { [0, 0, 0] }
        });
        assert!(close(dominant_color(&img, 100, 100).unwrap(), [40, 90, 230]));
    }

    #[test]
    fn dominant_color_prefers_largest_colored_area() {
        let img = image(100, 100, |x, _| if x < 70 { [220, 30, 60] } else { [30, 200, 80] });
        assert!(close(dominant_color(&img, 100, 100).unwrap(), [220, 30, 60]));
    }

    #[test]
    fn dominant_color_grey_image_is_none() {
        let img = image(50, 50, |x, _| { let v = (x * 5) as u8; [v, v, v] });
        assert_eq!(dominant_color(&img, 50, 50), None);
        assert_eq!(dominant_color(&[], 0, 0), None);
        // Taille incohérente avec le tampon : pas de panique.
        assert_eq!(dominant_color(&[1, 2, 3], 10, 10), None);
    }

    #[test]
    fn app_logo_gives_no_cover_accent() {
        // Logo rond sur fond transparent (ce que Chrome publie sans pochette) : pas une pochette.
        let mut logo = image(64, 64, |_, _| [252, 201, 63]);
        for (i, px) in logo.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % 64) as f32 - 31.5, (i / 64) as f32 - 31.5);
            if x * x + y * y > 30.0 * 30.0 {
                px[3] = 0;
            }
        }
        assert!(is_cutout(&logo, 64, 64));
        assert_eq!(cover_accent(&logo, 64, 64), None);
        // Pochette opaque, même avec un liseré adouci : sa couleur compte.
        let mut cover = image(64, 64, |_, _| [40, 90, 230]);
        for px in cover.chunks_exact_mut(4).take(64) {
            px[3] = 0;
        }
        assert!(!is_cutout(&cover, 64, 64));
        assert!(close(cover_accent(&cover, 64, 64).unwrap(), [40, 90, 230]));
        assert!(!is_cutout(&[], 0, 0) && !is_cutout(&[1, 2, 3], 10, 10));
    }

    #[test]
    fn readable_accent_lifts_dark_colors() {
        let [r, g, b] = readable_accent([60, 10, 10]);
        assert!(r.max(g).max(b) >= 180, "{r} {g} {b}");
        assert!(r > g && r > b, "garde la teinte");
        assert_eq!(readable_accent([249, 115, 22]), [249, 115, 22]);
    }

    #[test]
    fn sources_prefer_cover_while_playing() {
        let (cover, wall) = (Some([200, 40, 40]), Some([40, 120, 220]));
        let mut s = AccentSources { cover, playing: true, wallpaper: wall };
        assert_eq!((s.bars(), s.prisme()), (cover, cover));
        s.playing = false;
        assert_eq!((s.bars(), s.prisme()), (wall, wall), "en pause : le fond d'écran");
        s.wallpaper = None;
        assert_eq!((s.bars(), s.prisme()), (cover, None), "sans fond : dernière pochette, Prisme d'origine");
        s = AccentSources { cover: None, playing: true, wallpaper: wall };
        assert_eq!((s.bars(), s.prisme()), (wall, None), "lecture sans pochette");
        assert_eq!(AccentSources::default().bars(), None);
    }

    #[test]
    fn same_accent_tolerates_small_drift() {
        assert!(same_accent([40, 120, 220], [45, 125, 225]));
        assert!(same_accent([250, 20, 30], [250, 30, 20]), "le rouge fait le tour de la roue");
        assert!(!same_accent([40, 120, 220], [220, 40, 120]));
        assert!(!same_accent([40, 120, 220], [20, 60, 110]), "beaucoup plus sombre");
    }

    #[test]
    fn windows_palette_keeps_hue_and_orders_lightness() {
        let p = windows_palette([40, 120, 220]);
        let l: Vec<f32> = p[..7].iter().map(|c| to_hsl([c[0], c[1], c[2]]).2).collect();
        assert!(l.windows(2).all(|w| w[0] > w[1]), "du clair au sombre : {l:?}");
        for c in &p[..7] {
            let (h, _, _) = to_hsv([c[0], c[1], c[2]]);
            assert!((h - 214.0).abs() < 6.0, "teinte gardée : {h}");
            assert_eq!(c[3], 0xFF);
        }
        assert_eq!(p[7], [0, 0, 0, 0]);
        // Couleur presque grise : saturation relevée, pas de palette grise.
        let g = windows_palette([128, 120, 118]);
        assert!(to_hsl([g[3][0], g[3][1], g[3][2]]).1 >= 0.5);
    }

    #[test]
    fn accent_null_sent_once() {
        let mut s = AccentSender::default();
        assert_eq!(s.next(None), Some(None));
        assert_eq!(s.next(None), None);
        assert_eq!(s.next(Some([1, 2, 3])), Some(Some([1, 2, 3])));
        assert_eq!(s.next(Some([1, 2, 3])), None);
        assert_eq!(s.next(None), Some(None));
        s.reset();
        assert_eq!(s.next(None), Some(None));
    }
}
