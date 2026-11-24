//! Ce que les barres tirent du son en cours : les cinq capsules du widget Lecture en cours,
//! l'image de son îlot pour ce qui ne se dessine pas en rectangles (`canvas`), et le son d'essai
//! (`horizon.exe --musique-essai`), qui remplace la capture pour voir les animations sans rien
//! jouer.

pub mod canvas;

/// Capsules du fond « Capsules ».
pub const CAPSULES: usize = 5;

/// Hauteur (0–1) de chaque capsule d'après les bandes du spectre (0–1, des graves aux aigus) :
/// les graves au centre, les médiums autour, les aigus au bord droit. Les groupes peu
/// énergiques (médiums, aigus) sont relevés pour que les cinq bougent.
pub fn capsules(bands: &[f32]) -> [f32; CAPSULES] {
    // Part du spectre de chaque capsule (sur 24) et son gain, de gauche à droite.
    const GROUPS: [(usize, usize, f32); CAPSULES] = [(10, 14, 1.4), (3, 7, 1.2), (0, 3, 1.0), (6, 10, 1.4), (15, 23, 2.2)];
    let n = bands.len();
    GROUPS.map(|(from, to, gain)| {
        let (a, b) = (from * n / 24, (to * n / 24).min(n));
        if a >= b {
            return 0.0;
        }
        let mean = bands[a..b].iter().sum::<f32>() / (b - a) as f32;
        (mean * gain).clamp(0.0, 1.0)
    })
}

/// Une image du son d'essai.
#[derive(Debug, Clone, PartialEq)]
pub struct EssaiFrame {
    pub level: f32,
    pub bass: f32,
    pub bands: Vec<f32>,
    pub beat: bool,
}

/// Temps entre deux grosses caisses du son d'essai (120 à la minute).
const BEAT: f32 = 0.5;

fn bump(i: usize, centre: f32, width: f32) -> f32 {
    let d = i as f32 - centre;
    (-(d * d) / (2.0 * width * width)).exp()
}

/// Ce que joue le son d'essai : un morceau entier, ou un seul ingrédient, pour juger chaque
/// animation sur ce qui la fait bouger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Essai {
    /// Grosse caisse, basse, mélodie, charleston.
    #[default]
    Mix,
    /// Une grosse caisse par demi-seconde, rien d'autre (le coup seul, sans basse ni volume).
    Kick,
    /// La basse seule, qui monte puis redescend en 2 s, sans grosse caisse.
    Bass,
    /// Le volume et le spectre seuls, qui montent puis redescendent en 2 s, sans grosse caisse.
    Level,
}

impl Essai {
    /// Nom donné à `--musique-essai` (`mix`, `kick`, `bass`, `level`).
    pub fn parse(s: &str) -> Option<Essai> {
        match s {
            "mix" => Some(Essai::Mix),
            "kick" => Some(Essai::Kick),
            "bass" => Some(Essai::Bass),
            "level" => Some(Essai::Level),
            _ => None,
        }
    }
}

/// Son d'essai `kind` à l'instant `t` (secondes), `dt` après l'image précédente. Toujours le
/// même pour un même instant.
pub fn essai(kind: Essai, t: f32, dt: f32, bands: usize) -> EssaiFrame {
    let swell = 0.5 - 0.5 * (t * std::f32::consts::PI).cos();
    let beat = (t / BEAT).floor() != ((t - dt) / BEAT).floor();
    match kind {
        Essai::Mix => essai_frame(t, dt, bands),
        Essai::Kick => {
            let kick = (-t.rem_euclid(BEAT) / 0.13).exp();
            let values = (0..bands).map(|i| kick * bump(i, bands as f32 / 24.0, 1.5)).collect();
            EssaiFrame { level: 0.0, bass: 0.0, bands: values, beat }
        }
        Essai::Bass => EssaiFrame { level: 0.0, bass: swell, bands: vec![0.0; bands], beat: false },
        Essai::Level => {
            let values: Vec<f32> = (0..bands).map(|i| swell * (0.4 + 0.6 * bump(i, bands as f32 * 0.4, bands as f32 * 0.25))).collect();
            EssaiFrame { level: swell, bass: 0.0, bands: values, beat: false }
        }
    }
}

/// Son d'essai à l'instant `t` (secondes), `dt` après l'image précédente : une grosse caisse
/// par demi-seconde, une basse à contretemps, une mélodie qui change de note à chaque mesure et
/// une charleston. Toujours le même pour un même instant.
pub fn essai_frame(t: f32, dt: f32, bands: usize) -> EssaiFrame {
    let phase = t.rem_euclid(BEAT);
    let beat = (t / BEAT).floor() != ((t - dt) / BEAT).floor();
    let kick = (-phase / 0.13).exp();
    let off = (t + BEAT / 2.0).rem_euclid(BEAT);
    let bass = (-off / 0.2).exp() * 0.8;
    let hat = (-(t.rem_euclid(BEAT / 2.0)) / 0.04).exp() * 0.5;
    let note = [12.0, 14.0, 11.0, 15.0][(t / (4.0 * BEAT)).floor().rem_euclid(4.0) as usize];
    let lead = (-(t.rem_euclid(2.0 * BEAT)) / 0.3).exp() * 0.7;
    // Positions pensées pour 24 bandes, ramenées au nombre demandé.
    let scale = bands as f32 / 24.0;
    let values: Vec<f32> = (0..bands)
        .map(|i| {
            let at = |centre: f32, width: f32| bump(i, centre * scale, width * scale);
            let pad = 0.18 * at(9.0, 6.0) * (0.75 + 0.25 * (t * 1.3 + i as f32 * 0.6).sin());
            (kick * at(1.0, 1.5) + bass * at(3.0, 1.7) + lead * at(note, 2.0) + hat * at(21.0, 2.2) + pad).clamp(0.0, 1.0)
        })
        .collect();
    let mean = if bands == 0 { 0.0 } else { values.iter().sum::<f32>() / bands as f32 };
    EssaiFrame { level: (mean * 2.6).clamp(0.0, 1.0), bass: kick.max(bass).clamp(0.0, 1.0), bands: values, beat }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsules_follow_their_part_of_the_spectrum() {
        assert_eq!(capsules(&[0.0; 24]), [0.0; CAPSULES]);
        assert_eq!(capsules(&[]), [0.0; CAPSULES]);
        let mut bands = [0.0f32; 24];
        bands[..3].fill(0.8);
        let c = capsules(&bands);
        assert!((c[2] - 0.8).abs() < 1e-6, "les graves au centre");
        assert_eq!([c[0], c[1], c[3], c[4]], [0.0; 4]);
        bands.fill(1.0);
        assert_eq!(capsules(&bands), [1.0; CAPSULES], "jamais au-delà de 1");
        // Un autre nombre de bandes : les mêmes parts.
        let mut twelve = [0.0f32; 12];
        twelve[0] = 1.0;
        assert!(capsules(&twelve)[2] > 0.9);
    }

    #[test]
    fn essai_sound_has_one_kick_per_half_second() {
        let dt = 0.05;
        // Une image de plus que 10 s : la vingtième grosse caisse tombe sur la dernière ou
        // l'avant-dernière selon l'arrondi.
        let frames: Vec<EssaiFrame> = (1..=201).map(|i| essai_frame(i as f32 * dt, dt, 24)).collect();
        assert_eq!(frames.iter().filter(|f| f.beat).count(), 20, "10 s à 120 à la minute");
        for f in &frames {
            assert_eq!(f.bands.len(), 24);
            assert!(f.bands.iter().all(|v| (0.0..=1.0).contains(v)));
            assert!((0.0..=1.0).contains(&f.level) && (0.0..=1.0).contains(&f.bass));
        }
        let on_kick = frames.iter().find(|f| f.beat).unwrap();
        assert!(on_kick.bass > 0.6 && on_kick.bands[1] > 0.6, "la grosse caisse est dans les graves");
        assert_eq!(essai_frame(1.23, dt, 24), essai_frame(1.23, dt, 24));
        assert!(essai_frame(1.0, dt, 0).bands.is_empty());
    }

    #[test]
    fn each_ingredient_plays_alone() {
        let dt = 0.05;
        let run = |kind| (1..=40).map(|i| essai(kind, i as f32 * dt, dt, 24)).collect::<Vec<_>>();
        let kick = run(Essai::Kick);
        assert_eq!(kick.iter().filter(|f| f.beat).count(), 4, "2 s : quatre coups");
        assert!(kick.iter().all(|f| f.bass == 0.0 && f.level == 0.0), "le coup seul");
        let bass = run(Essai::Bass);
        assert!(bass.iter().all(|f| !f.beat && f.level == 0.0));
        let top = bass.iter().map(|f| f.bass).fold(0.0, f32::max);
        assert!(top > 0.99 && bass.last().unwrap().bass < 0.01, "monte puis redescend : {top}");
        let level = run(Essai::Level);
        assert!(level.iter().all(|f| !f.beat && f.bass == 0.0) && level[19].level > 0.99);
        assert_eq!(essai(Essai::Mix, 1.23, dt, 24), essai_frame(1.23, dt, 24));
        assert_eq!(Essai::parse("kick"), Some(Essai::Kick));
        assert_eq!(Essai::parse("autre"), None);
    }
}
