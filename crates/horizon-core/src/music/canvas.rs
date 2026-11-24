//! Image de l'îlot de Lecture en cours : ce qui suit le son et ne se dessine pas avec des
//! rectangles (halo, ruban, aurore, points, progression qui ondule, lumière qui tourne autour de
//! l'îlot). Elle est calculée pixel par pixel à la taille de l'îlot, coins arrondis compris (le
//! rendu logiciel de Slint ne découpe pas les coins), et reposée à chaque image de son.
//!
//! Tout est en pixels logiques (ceux de l'interface) ; `scale` donne les pixels de l'écran.

use crate::settings::Backdrop;

/// Couleurs d'une image : l'accent et deux teintes voisines (±38° sur le cercle des teintes),
/// pour les dessins à plusieurs couleurs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub a: [u8; 3],
    pub b: [u8; 3],
    pub c: [u8; 3],
}

impl Palette {
    pub fn from_accent(accent: [u8; 3]) -> Palette {
        Palette { a: accent, b: turn_hue(accent, -38.0), c: turn_hue(accent, 38.0) }
    }
}

/// La couleur `rgb`, sa teinte tournée de `degrees` (saturation et luminosité gardées).
fn turn_hue(rgb: [u8; 3], degrees: f32) -> [u8; 3] {
    let [r, g, b] = rgb.map(|v| v as f32 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    if delta < 1e-6 {
        return rgb;
    }
    let hue = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let h = (hue + degrees).rem_euclid(360.0) / 60.0;
    let x = delta * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (delta, x, 0.0),
        1 => (x, delta, 0.0),
        2 => (0.0, delta, x),
        3 => (0.0, x, delta),
        4 => (x, 0.0, delta),
        _ => (delta, 0.0, x),
    };
    [r, g, b].map(|v| ((v + min) * 255.0).round() as u8)
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

const WHITE: [u8; 3] = [255, 255, 255];

/// `cur` qui rejoint `target` : constante de temps `up` en montant, `down` en retombant.
fn ease(cur: f32, target: f32, dt: f32, up: f32, down: f32) -> f32 {
    cur + (target - cur) * (1.0 - (-dt / if target > cur { up } else { down }).exp())
}

/// Ce que les dessins tirent du son, lissé d'une image de son à l'autre (montée vive, retombée
/// lente) : c'est ce lissage, et non une transition de l'interface, qui adoucit le mouvement.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sound {
    /// Bandes du spectre (0–1, des graves aux aigus).
    pub bands: Vec<f32>,
    /// Les mêmes, bien plus lentes : seconde vague du ruban.
    pub slow: Vec<f32>,
    /// Crête de chaque bande, qui retombe doucement (points).
    pub peaks: Vec<f32>,
    pub level: f32,
    pub bass: f32,
    /// Graves, médiums, aigus (aurore).
    pub low: f32,
    pub mid: f32,
    pub high: f32,
    /// Grosse caisse qui retombe.
    pub kick: f32,
    /// Opacité du halo : niveau du moment, avec un éclat sur la grosse caisse.
    pub glow: f32,
    /// Dérive de l'aurore, qui n'avance qu'avec le son.
    pub drift: f32,
    /// Ondulation de la progression : phase et amplitude (pixels logiques).
    pub wave_phase: f32,
    pub wave_amp: f32,
    /// Place de la lumière qui tourne, en part du tour de l'îlot (0–1).
    pub comet: f32,
}

impl Sound {
    /// Une image de son, `dt` secondes après la précédente.
    pub fn feed(&mut self, level: f32, bass: f32, bands: &[f32], beat: bool, dt: f32) {
        let n = bands.len();
        if self.bands.len() != n {
            self.bands = vec![0.0; n];
            self.slow = vec![0.0; n];
            self.peaks = vec![0.0; n];
        }
        for (i, b) in bands.iter().enumerate() {
            let b = b.clamp(0.0, 1.0);
            self.bands[i] = ease(self.bands[i], b, dt, 0.04, 0.17);
            self.slow[i] = ease(self.slow[i], b, dt, 0.25, 0.7);
            self.peaks[i] = (self.peaks[i] - dt * 0.9).max(b);
        }
        let mean = |from: usize, to: usize| if to > from { self.bands[from..to].iter().sum::<f32>() / (to - from) as f32 } else { 0.0 };
        (self.low, self.mid, self.high) =
            ((mean(0, n / 4) * 1.5).min(1.0), (mean(n / 4, 2 * n / 3) * 1.8).min(1.0), (mean(2 * n / 3, n) * 2.2).min(1.0));
        let level = level.clamp(0.0, 1.0);
        self.level = ease(self.level, level, dt, 0.04, 0.17);
        self.bass = ease(self.bass, bass.clamp(0.0, 1.0), dt, 0.04, 0.17);
        self.kick = if beat { 1.0 } else { self.kick * (-dt / 0.12).exp() };
        self.glow = (0.2 + level * 0.9 + if beat { 0.15 } else { 0.0 }).min(1.0);
        self.drift += dt * (0.15 + self.level * 1.5);
        self.wave_phase = (self.wave_phase + dt * (3.0 + self.level * 9.0)).rem_euclid(std::f32::consts::TAU);
        self.wave_amp = ease(self.wave_amp, 0.5 + self.level * 1.5, dt, 0.08, 0.22);
        self.comet = (self.comet + dt * (0.04 + self.level * 0.2 + self.kick * 0.4)).rem_euclid(1.0);
    }

    /// La lecture s'arrête : tout retombe, la dérive et la place de la lumière sont gardées.
    pub fn rest(&mut self) {
        *self = Sound { drift: self.drift, comet: self.comet, bands: vec![0.0; self.bands.len()], slow: vec![0.0; self.bands.len()], peaks: vec![0.0; self.bands.len()], ..Sound::default() };
    }
}

/// Ce qu'une image montre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    /// Fond de la lecture en cours ; seuls le halo, le ruban, l'aurore et les points se
    /// dessinent ici ([`Backdrop::is_painted`]).
    pub backdrop: Backdrop,
    /// Part lue (0–1) si la progression ondule ; `None` : l'interface la dessine, droite.
    pub progress: Option<f32>,
    /// Lumière qui tourne autour de l'îlot (pulsation « comet »).
    pub comet: bool,
    pub playing: bool,
}

impl Scene {
    /// Rien à dessiner : l'image n'est pas montrée.
    pub fn is_empty(&self) -> bool {
        !self.backdrop.is_painted() && self.progress.is_none() && !self.comet
    }
}

/// Image d'un îlot de `width` × `height` pixels d'écran.
#[derive(Debug, Clone)]
pub struct Canvas {
    width: usize,
    height: usize,
    scale: f32,
    /// Rayon des coins de l'îlot (pixels logiques).
    radius: f32,
    /// Couleur prémultipliée de chaque pixel (rouge, vert, bleu, alpha, de 0 à 1).
    acc: Vec<[f32; 4]>,
    pixels: Vec<u8>,
}

impl Canvas {
    pub fn new(width: u32, height: u32, scale: f32, radius: f32) -> Canvas {
        let (width, height) = (width as usize, height as usize);
        Canvas { width, height, scale: scale.max(0.1), radius, acc: vec![[0.0; 4]; width * height], pixels: vec![0; width * height * 4] }
    }

    /// Cette image est celle d'un îlot de cette taille (sinon elle est à refaire).
    pub fn fits(&self, width: u32, height: u32, scale: f32, radius: f32) -> bool {
        (self.width, self.height, self.scale, self.radius) == (width as usize, height as usize, scale.max(0.1), radius)
    }

    /// Taille de l'îlot en pixels logiques.
    fn size(&self) -> (f32, f32) {
        (self.width as f32 / self.scale, self.height as f32 / self.scale)
    }

    /// Centre du pixel (`x`, `y`) en pixels logiques.
    fn at(&self, x: usize, y: usize) -> (f32, f32) {
        ((x as f32 + 0.5) / self.scale, (y as f32 + 0.5) / self.scale)
    }

    /// Pose `color` par-dessus le pixel, à l'opacité `alpha`.
    fn over(&mut self, x: usize, y: usize, color: [u8; 3], alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        let p = &mut self.acc[y * self.width + x];
        for (v, c) in p.iter_mut().zip(color) {
            *v = c as f32 / 255.0 * a + *v * (1.0 - a);
        }
        p[3] = a + p[3] * (1.0 - a);
    }

    /// Ajoute la lumière de `color` au pixel (les couleurs se mêlent en s'éclaircissant).
    fn plus(&mut self, x: usize, y: usize, color: [u8; 3], alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        let p = &mut self.acc[y * self.width + x];
        for (v, c) in p.iter_mut().zip(color) {
            *v = (*v + c as f32 / 255.0 * a).min(1.0);
        }
        p[3] = (p[3] + a).min(1.0);
    }

    /// Distance signée (pixels logiques) du point au bord d'un rectangle arrondi rentré de
    /// `inset` dans l'îlot : négative dedans.
    fn edge(&self, lx: f32, ly: f32, inset: f32) -> f32 {
        let (w, h) = self.size();
        let r = (self.radius - inset).clamp(0.0, (h / 2.0 - inset).max(0.0));
        let (qx, qy) = ((lx - w / 2.0).abs() - (w / 2.0 - inset - r), (ly - h / 2.0).abs() - (h / 2.0 - inset - r));
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
    }

    /// Dessine `scene` et rend les pixels (RGBA prémultiplié, `width` × `height`).
    pub fn paint(&mut self, scene: &Scene, sound: &Sound, colors: &Palette) -> &[u8] {
        self.acc.fill([0.0; 4]);
        match scene.backdrop {
            Backdrop::Halo => self.halo(if scene.playing { sound.glow } else { 0.12 }, colors),
            Backdrop::Ribbon => self.ribbon(sound, colors),
            Backdrop::Aurora => self.aurora(sound, colors, scene.playing),
            Backdrop::Dots => self.dots(sound, colors),
            Backdrop::Auto | Backdrop::Off | Backdrop::Spectrum | Backdrop::Capsules => {}
        }
        if let Some(progress) = scene.progress {
            self.squiggle(progress.clamp(0.0, 1.0), sound, colors);
        }
        if scene.comet && scene.playing {
            self.comet(sound, colors);
        }
        // Coins de l'îlot, puis octets.
        for y in 0..self.height {
            for x in 0..self.width {
                let (lx, ly) = self.at(x, y);
                let inside = (0.5 - self.edge(lx, ly, 0.0) * self.scale).clamp(0.0, 1.0);
                let i = y * self.width + x;
                for c in 0..4 {
                    self.pixels[i * 4 + c] = (self.acc[i][c] * inside * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        &self.pixels
    }

    /// Halo : ellipse de 70 % × 140 % de l'îlot, centrée à 18 % du bord gauche, en bas ; pleine
    /// à 55 % au centre, éteinte à 72 % du rayon (`radial-gradient` de la version Tauri).
    fn halo(&mut self, glow: f32, colors: &Palette) {
        let (w, h) = self.size();
        for y in 0..self.height {
            for x in 0..self.width {
                let (lx, ly) = self.at(x, y);
                let (dx, dy) = ((lx - 0.18 * w) / (0.7 * w), (ly - h) / (1.4 * h));
                self.over(x, y, colors.a, halo_alpha(dx, dy) * glow);
            }
        }
    }

    /// Ruban : les bandes fondues en une vague pleine dans l'accent, et derrière elle une
    /// seconde vague plus lente dans la teinte voisine. Sans son, deux traits au bas de l'îlot.
    fn ribbon(&mut self, sound: &Sound, colors: &Palette) {
        let (w, h) = self.size();
        for (bands, color, alpha, gain) in [(&sound.slow, colors.b, 0.2, 1.25), (&sound.bands, colors.a, 0.4, 1.05)] {
            let points = ribbon_points(bands);
            for x in 0..self.width {
                let lx = (x as f32 + 0.5) / self.scale;
                // Les points de la vague débordent un peu de l'îlot, pour qu'elle n'y soit pas pincée.
                // Courbe qui relève les sons moyens : la vague se voit sans attendre un son fort.
                let v = (curve(&points, (lx / w + 0.04) / 1.08) * gain).clamp(0.0, 1.0).powf(0.7);
                let top = h - 3.0 - v * (h - 6.0);
                for y in 0..self.height {
                    let ly = (y as f32 + 0.5) / self.scale;
                    let below = ((ly - top) * self.scale + 0.5).clamp(0.0, 1.0);
                    let fill = alpha * (1.0 - 0.88 * ly / h) * below;
                    let line = (alpha * 1.9).min(0.9) * (1.0 - (ly - top).abs() / 0.9).max(0.0);
                    self.over(x, y, color, fill.max(line));
                }
            }
        }
    }

    /// Aurore : trois nappes de lumière (graves dans l'accent, médiums et aigus dans les teintes
    /// voisines) qui gonflent avec leur part du spectre et dérivent avec le son.
    fn aurora(&mut self, sound: &Sound, colors: &Palette, playing: bool) {
        let (w, h) = self.size();
        let (ph, on) = (sound.drift, if playing { 1.0 } else { 0.45 });
        let blobs = [
            (0.2 + 0.09 * (ph * 0.9).sin(), 0.95, 1.0 + 1.7 * sound.low, colors.a, sound.low),
            (0.55 + 0.15 * (ph * 0.6 + 2.0).sin(), 0.25 + 0.3 * (ph * 0.8 + 1.0).sin(), 0.9 + 1.6 * sound.mid, colors.b, sound.mid),
            (0.86 + 0.08 * (ph * 0.7 + 4.0).sin(), 0.8, 0.8 + 1.7 * sound.high, colors.c, sound.high),
        ];
        for (bx, by, size, color, energy) in blobs {
            let (cx, cy, r, alpha) = (w * bx, h * by, h * size * 1.6, (0.2 + 0.5 * energy) * on);
            for y in 0..self.height {
                for x in 0..self.width {
                    let (lx, ly) = self.at(x, y);
                    let d = ((lx - cx).powi(2) + (ly - cy).powi(2)).sqrt() / r;
                    if d < 1.0 {
                        self.plus(x, y, color, alpha * (1.0 - d).powf(1.5));
                    }
                }
            }
        }
    }

    /// Points : une trame de points qui s'allument par colonnes, et le point de crête de chaque
    /// colonne dans la teinte voisine.
    fn dots(&mut self, sound: &Sound, colors: &Palette) {
        const PITCH: f32 = 6.0;
        const R: f32 = 1.35;
        let (w, h) = self.size();
        let (cols, rows) = (((w - 10.0) / PITCH).floor() as usize, ((h - 6.0) / PITCH).floor() as usize);
        if cols < 2 || rows < 1 {
            return;
        }
        let (x0, bottom) = ((w - (cols - 1) as f32 * PITCH) / 2.0, h / 2.0 + (rows - 1) as f32 * PITCH / 2.0);
        for j in 0..cols {
            let at = j as f32 / (cols - 1) as f32;
            let (lit, peak) = (sample(&sound.bands, at).powf(0.8) * rows as f32, sample(&sound.peaks, at).powf(0.8));
            let peak_row = ((peak * rows as f32) as usize).min(rows - 1);
            for row in 0..rows {
                let on = (lit - row as f32).clamp(0.0, 1.0);
                let (color, alpha) = if peak > 0.1 && row == peak_row && on < 0.5 { (colors.b, 0.75) } else { (colors.a, 0.06 + on * 0.36) };
                self.disc(x0 + j as f32 * PITCH, bottom - row as f32 * PITCH, R, color, alpha);
            }
        }
    }

    /// Disque plein de rayon `r` centré en (`cx`, `cy`), bord adouci.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, color: [u8; 3], alpha: f32) {
        let s = self.scale;
        let span = |c: f32, max: usize| (((c - r) * s).floor().max(0.0) as usize, (((c + r) * s).ceil().max(0.0) as usize).min(max));
        let ((xa, xb), (ya, yb)) = (span(cx, self.width), span(cy, self.height));
        for y in ya..yb {
            for x in xa..xb {
                let (lx, ly) = self.at(x, y);
                let d = ((lx - cx).powi(2) + (ly - cy).powi(2)).sqrt();
                self.over(x, y, color, alpha * ((r - d) * s + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// Progression qui ondule : la part lue est une onde dans l'accent, d'autant plus ample que
    /// le son est fort (droite sans son), le reste un trait discret, et un point à la jonction.
    /// Elle tient dans les 6 px du bas de l'îlot, sous la ligne de l'artiste.
    fn squiggle(&mut self, progress: f32, sound: &Sound, colors: &Palette) {
        const HALF: f32 = 0.9;
        const LENGTH: f32 = 16.0;
        let (w, h) = self.size();
        let (x0, x1, base) = (10.0, w - 10.0, h - 4.0);
        if x1 <= x0 {
            return;
        }
        let head = x0 + (x1 - x0) * progress;
        let k = std::f32::consts::TAU / LENGTH;
        for x in 0..self.width {
            let lx = (x as f32 + 0.5) / self.scale;
            // Bouts adoucis, à gauche de la barre et à droite.
            let ends = ((lx - x0) * self.scale + 0.5).clamp(0.0, 1.0) * ((x1 - lx) * self.scale + 0.5).clamp(0.0, 1.0);
            if ends <= 0.0 {
                continue;
            }
            // L'onde s'aplatit à ses deux bouts.
            let taper = ((lx - x0).min(head - lx) / 8.0).clamp(0.0, 1.0);
            let (centre, slope) = if lx <= head {
                let a = sound.wave_amp * taper;
                (base + ((lx - x0) * k - sound.wave_phase).sin() * a, ((lx - x0) * k - sound.wave_phase).cos() * a * k)
            } else {
                (base, 0.0)
            };
            let (color, alpha) = if lx <= head { (colors.a, 1.0) } else { (WHITE, 0.12) };
            for y in 0..self.height {
                let ly = (y as f32 + 0.5) / self.scale;
                // Distance au trait, mesurée en travers de sa pente.
                let d = (ly - centre).abs() / (1.0 + slope * slope).sqrt();
                self.over(x, y, color, alpha * ends * ((HALF - d) * self.scale + 0.5).clamp(0.0, 1.0));
            }
        }
        self.disc(head, base, 2.2, mix(colors.a, WHITE, 0.3), 1.0);
    }

    /// Lumière qui tourne : deux traînées (l'accent et sa teinte voisine) font le tour de l'îlot.
    /// La basse règle leur éclat, le niveau leur longueur ; leur place avance dans [`Sound::feed`].
    fn comet(&mut self, sound: &Sound, colors: &Palette) {
        const INSET: f32 = 0.8;
        let (w, h) = self.size();
        let r = (self.radius - INSET).clamp(0.0, (h / 2.0 - INSET).max(0.0));
        let (a, b, arc) = (w - 2.0 * INSET - 2.0 * r, h - 2.0 * INSET - 2.0 * r, std::f32::consts::FRAC_PI_2 * r);
        let around = 2.0 * a + 2.0 * b + 4.0 * arc;
        if a < 0.0 || b < 0.0 || around <= 0.0 {
            return;
        }
        let (light, tail) = (0.45 + 0.55 * sound.bass, around * (0.2 + 0.12 * sound.level));
        for y in 0..self.height {
            for x in 0..self.width {
                let (lx, ly) = self.at(x, y);
                let d = self.edge(lx, ly, INSET);
                if !(-3.0..=1.0).contains(&d) {
                    continue;
                }
                // Trait net sur le bord, lueur vers l'intérieur.
                let (core, soft) = ((1.0 - d.abs() / 1.1).max(0.0), if d < 0.0 { (1.0 + d / 3.0).max(0.0) * 0.3 } else { 0.0 });
                self.over(x, y, colors.a, (0.12 + 0.1 * light) * core);
                // Place du point sur le tour (du coin haut gauche, dans le sens des aiguilles).
                let (dx, dy) = (lx - lx.clamp(INSET + r, w - INSET - r), ly - ly.clamp(INSET + r, h - INSET - r));
                let s = if dx == 0.0 && ly < h / 2.0 {
                    lx - INSET - r
                } else if dx == 0.0 {
                    a + arc + b + arc + (w - INSET - r - lx)
                } else if dy == 0.0 && lx > w / 2.0 {
                    a + arc + (ly - INSET - r)
                } else if dy == 0.0 {
                    2.0 * a + b + 3.0 * arc + (h - INSET - r - ly)
                } else if dx > 0.0 && dy < 0.0 {
                    a + r * dx.atan2(-dy)
                } else if dx > 0.0 {
                    a + arc + b + r * dy.atan2(dx)
                } else if dy > 0.0 {
                    2.0 * a + b + 2.0 * arc + r * (-dx).atan2(dy)
                } else {
                    2.0 * a + 2.0 * b + 3.0 * arc + r * (-dy).atan2(-dx)
                };
                for (turn, color) in [(0.0, colors.a), (0.5, colors.b)] {
                    let behind = ((sound.comet + turn) * around - s).rem_euclid(around);
                    if behind < tail {
                        let f = (1.0 - behind / tail).powf(1.5) * light;
                        self.over(x, y, mix(color, WHITE, f * 0.35), f * core.max(soft));
                    }
                }
            }
        }
    }
}

/// Opacité du halo au point (`dx`, `dy`) de son ellipse (−1 à 1 sur chaque axe).
fn halo_alpha(dx: f32, dy: f32) -> f32 {
    0.55 * (1.0 - (dx * dx + dy * dy).sqrt() / 0.72).max(0.0)
}

/// Valeur de `values` à la place `at` (0–1), entre deux bandes voisines.
fn sample(values: &[f32], at: f32) -> f32 {
    let Some(last) = values.len().checked_sub(1) else { return 0.0 };
    let f = at.clamp(0.0, 1.0) * last as f32;
    let i = f as usize;
    let next = values[(i + 1).min(last)];
    (values[i] + (next - values[i]) * (f - i as f32)).clamp(0.0, 1.0)
}

/// Points de passage du ruban : les bandes par paires, adoucies entre voisines, et rabattues
/// aux deux bouts pour que la vague se lise comme une forme.
fn ribbon_points(bands: &[f32]) -> Vec<f32> {
    let n = bands.len() / 2;
    let pairs: Vec<f32> = (0..n).map(|k| (bands[2 * k] + bands[2 * k + 1]) / 2.0).collect();
    (0..n)
        .map(|k| {
            let soft = (pairs[k.saturating_sub(1)] + 2.0 * pairs[k] + pairs[(k + 1).min(n - 1)]) / 4.0;
            soft * (std::f32::consts::PI * (k as f32 + 0.5) / n as f32).sin().sqrt()
        })
        .collect()
}

/// Courbe lisse qui passe par `points` (Catmull-Rom), lue à la place `at` (0–1).
fn curve(points: &[f32], at: f32) -> f32 {
    let Some(last) = points.len().checked_sub(1) else { return 0.0 };
    let f = at.clamp(0.0, 1.0) * last as f32;
    let i = (f as usize).min(last);
    let t = f - i as f32;
    let p = |k: isize| points[(i as isize + k).clamp(0, last as isize) as usize];
    let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
    (0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENT: [u8; 3] = [249, 115, 22];

    fn scene(backdrop: Backdrop) -> Scene {
        Scene { backdrop, progress: None, comet: false, playing: true }
    }

    /// Îlot de 300 × 34 à 100 %, coins de 12.
    fn canvas() -> Canvas {
        Canvas::new(300, 34, 1.0, 12.0)
    }

    fn loud() -> Sound {
        let mut s = Sound::default();
        for _ in 0..40 {
            s.feed(0.8, 0.9, &[0.9; 24], false, 0.05);
        }
        s
    }

    fn alpha(pixels: &[u8], x: usize, y: usize) -> u8 {
        pixels[(y * 300 + x) * 4 + 3]
    }

    #[test]
    fn neighbour_hues_keep_the_brightness() {
        let p = Palette::from_accent(ACCENT);
        assert_eq!(p.a, ACCENT);
        assert_ne!(p.b, p.a);
        assert_ne!(p.c, p.b);
        for c in [p.b, p.c] {
            assert_eq!(c.iter().max(), ACCENT.iter().max(), "même luminosité");
            assert_eq!(c.iter().min(), ACCENT.iter().min(), "même saturation");
        }
        assert_eq!(turn_hue([255, 0, 0], 120.0), [0, 255, 0]);
        assert_eq!(turn_hue([255, 0, 0], -120.0), [0, 0, 255]);
        assert_eq!(turn_hue([90, 90, 90], 38.0), [90, 90, 90], "un gris n'a pas de teinte");
    }

    #[test]
    fn sound_rises_fast_and_falls_slowly() {
        let mut s = Sound::default();
        s.feed(1.0, 1.0, &[1.0; 24], true, 0.05);
        assert!(s.bands[0] > 0.6 && s.slow[0] < 0.3, "vague vive et vague lente");
        assert_eq!((s.kick, s.peaks[0], s.glow), (1.0, 1.0, 1.0));
        let quick = s.bands[0];
        s.feed(0.0, 0.0, &[0.0; 24], false, 0.05);
        assert!(s.bands[0] > quick * 0.6, "retombée lente");
        assert!(s.kick < 0.7 && s.peaks[0] > 0.9 && s.glow < 0.25);
        let (drift, comet) = (s.drift, s.comet);
        s.rest();
        assert_eq!((s.level, s.kick, s.wave_amp, s.bands.len()), (0.0, 0.0, 0.0, 24));
        assert_eq!((s.drift, s.comet), (drift, comet), "la dérive reprend où elle en était");
        s.feed(0.5, 0.5, &[0.5; 12], false, 0.05);
        assert_eq!(s.bands.len(), 12, "autre nombre de bandes");
    }

    #[test]
    fn nothing_to_draw_is_an_empty_scene() {
        for b in [Backdrop::Off, Backdrop::Spectrum, Backdrop::Capsules] {
            assert!(scene(b).is_empty(), "{b:?}");
        }
        for b in [Backdrop::Halo, Backdrop::Ribbon, Backdrop::Aurora, Backdrop::Dots] {
            assert!(!scene(b).is_empty(), "{b:?}");
        }
        assert!(!Scene { progress: Some(0.3), ..scene(Backdrop::Off) }.is_empty());
        assert!(!Scene { comet: true, ..scene(Backdrop::Capsules) }.is_empty());
        let mut c = canvas();
        assert!(c.paint(&scene(Backdrop::Off), &loud(), &Palette::from_accent(ACCENT)).iter().all(|v| *v == 0));
        assert!(c.fits(300, 34, 1.0, 12.0) && !c.fits(300, 34, 1.5, 12.0) && !c.fits(280, 34, 1.0, 12.0));
    }

    #[test]
    fn halo_fades_from_its_centre_and_follows_the_corners() {
        assert!((halo_alpha(0.0, 0.0) - 0.55).abs() < 1e-6);
        assert_eq!(halo_alpha(0.72, 0.0), 0.0);
        assert!((halo_alpha(0.0, 0.36) - 0.275).abs() < 1e-6);
        let mut c = canvas();
        let mut s = Sound::default();
        s.feed(1.0, 0.0, &[], true, 0.05);
        let px = c.paint(&scene(Backdrop::Halo), &s, &Palette::from_accent(ACCENT)).to_vec();
        // Centre de l'ellipse : 18 % du bord gauche, en bas.
        assert!(alpha(&px, 54, 32) > 120, "{}", alpha(&px, 54, 32));
        assert_eq!(alpha(&px, 290, 5), 0, "loin du centre");
        assert_eq!(alpha(&px, 0, 33), 0, "coin bas gauche découpé");
        assert!(alpha(&px, 12, 33) > 60, "bord bas gardé après le coin");
        // En pause : un halo faible, le même quel que soit le son.
        let quiet = c.paint(&Scene { playing: false, ..scene(Backdrop::Halo) }, &s, &Palette::from_accent(ACCENT)).to_vec();
        assert!((14..=20).contains(&alpha(&quiet, 54, 32)), "{}", alpha(&quiet, 54, 32));
    }

    #[test]
    fn ribbon_rises_with_the_sound_and_lies_flat_without() {
        let mut c = canvas();
        let p = Palette::from_accent(ACCENT);
        let loud = c.paint(&scene(Backdrop::Ribbon), &loud(), &p).to_vec();
        assert!(alpha(&loud, 150, 10) > 40, "vague haute au milieu : {}", alpha(&loud, 150, 10));
        let quiet = c.paint(&scene(Backdrop::Ribbon), &Sound::default(), &p).to_vec();
        assert_eq!(alpha(&quiet, 150, 10), 0);
        assert!(alpha(&quiet, 150, 31) > 60, "trait au bas de l'îlot : {}", alpha(&quiet, 150, 31));
    }

    #[test]
    fn curve_passes_through_its_points() {
        let pts = [0.0, 1.0, 0.5, 0.25];
        assert!((curve(&pts, 0.0) - 0.0).abs() < 1e-6);
        assert!((curve(&pts, 1.0 / 3.0) - 1.0).abs() < 1e-5);
        assert!((curve(&pts, 1.0) - 0.25).abs() < 1e-6);
        assert_eq!(curve(&[], 0.5), 0.0);
        assert_eq!(ribbon_points(&[1.0; 24]).len(), 12);
        assert!(ribbon_points(&[1.0; 24])[0] < ribbon_points(&[1.0; 24])[6], "rabattue aux bouts");
    }

    #[test]
    fn aurora_glows_and_dims_when_paused() {
        let mut c = canvas();
        let p = Palette::from_accent(ACCENT);
        let s = loud();
        let on = c.paint(&scene(Backdrop::Aurora), &s, &p).to_vec();
        let off = c.paint(&Scene { playing: false, ..scene(Backdrop::Aurora) }, &s, &p).to_vec();
        assert!(alpha(&on, 60, 30) > alpha(&off, 60, 30) && alpha(&off, 60, 30) > 0);
        assert_eq!(alpha(&on, 0, 0), 0, "coin découpé");
    }

    #[test]
    fn dots_light_up_by_columns() {
        let mut c = canvas();
        let p = Palette::from_accent(ACCENT);
        let lit = |px: &[u8]| (0..300 * 34).filter(|i| px[i * 4 + 3] > 80).count();
        let quiet = lit(c.paint(&scene(Backdrop::Dots), &Sound::default(), &p));
        let loud = lit(c.paint(&scene(Backdrop::Dots), &loud(), &p));
        assert_eq!(quiet, 0, "trame éteinte");
        assert!(loud > 200, "{loud}");
        assert_eq!(sample(&[0.0, 1.0], 0.5), 0.5);
        assert_eq!(sample(&[], 0.5), 0.0);
    }

    #[test]
    fn squiggle_waves_only_over_the_part_played() {
        let mut c = canvas();
        let p = Palette::from_accent(ACCENT);
        let s = loud();
        let px = c.paint(&Scene { progress: Some(0.5), ..scene(Backdrop::Off) }, &s, &p).to_vec();
        // Trait du reste, droit et discret, à 4 px du bas.
        assert!((20..=40).contains(&alpha(&px, 250, 29)), "{}", alpha(&px, 250, 29));
        assert_eq!(alpha(&px, 250, 25), 0);
        // Part lue : opaque quelque part dans chaque colonne, et pas toujours sur la même ligne.
        let row = |x: usize| (20..34).find(|y| alpha(&px, x, *y) > 200);
        let rows: Vec<_> = (40..120).map(row).collect();
        assert!(rows.iter().all(Option::is_some));
        assert!(rows.iter().any(|r| *r != rows[0]), "elle ondule");
        assert_eq!(alpha(&px, 5, 29), 0, "rien avant le début de la barre");
        // Sans son : droite.
        let flat = c.paint(&Scene { progress: Some(0.5), playing: false, ..scene(Backdrop::Off) }, &Sound::default(), &p).to_vec();
        assert!((40..120).all(|x| alpha(&flat, x, 29) > 100 && alpha(&flat, x, 26) == 0));
        // Avec du son, elle reste dans les 7 px du bas.
        assert!((40..120).all(|x| alpha(&px, x, 26) == 0), "sous la ligne de l'artiste");
    }

    #[test]
    fn comet_stays_on_the_edge_of_the_island() {
        let mut c = canvas();
        let p = Palette::from_accent(ACCENT);
        let px = c.paint(&Scene { comet: true, ..scene(Backdrop::Off) }, &loud(), &p).to_vec();
        assert_eq!(alpha(&px, 150, 17), 0, "rien au milieu");
        assert!(alpha(&px, 150, 0) > 0 && alpha(&px, 150, 33) > 0, "bord du haut et du bas");
        let brightest = (0..300).map(|x| alpha(&px, x, 0).max(alpha(&px, x, 33))).max().unwrap();
        assert!(brightest > 130, "une traînée bien visible : {brightest}");
        let paused = c.paint(&Scene { comet: true, playing: false, ..scene(Backdrop::Off) }, &loud(), &p).to_vec();
        assert!(paused.iter().all(|v| *v == 0), "éteinte en pause");
    }
}
