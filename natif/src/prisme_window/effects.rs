//! Description des effets pour l'éditeur de scènes (repris de `apps/prisme/lib/effects.ts`). Les
//! valeurs par défaut reprennent celles du moteur (`crates/core/src/effects/*.rs` de `rgb-engine`).

use serde_json::{Map, Value, json};

/// Un paramètre d'effet, selon son type de contrôle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Range { min: f64, max: f64, step: f64, unit: Option<&'static str>, default: f64 },
    Hue { default: f64 },
    Bool { default: bool },
    Color { default: &'static str },
    Palette,
    Select { options: &'static [(&'static str, &'static str)], default: &'static str },
    Multi { options: &'static [(&'static str, &'static str)], default: &'static [&'static str] },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub hint: Option<&'static str>,
}

impl Param {
    /// Valeur par défaut, en JSON.
    pub fn default_value(&self) -> Value {
        match self.kind {
            Kind::Range { default, .. } | Kind::Hue { default } => json!(default),
            Kind::Bool { default } => json!(default),
            Kind::Color { default } => json!(default),
            Kind::Palette => json!([]),
            Kind::Select { default, .. } => json!(default),
            Kind::Multi { default, .. } => json!(default),
        }
    }
}

pub struct EffectInfo {
    pub key: &'static str,
    pub label: &'static str,
    pub summary: &'static str,
    pub params: &'static [Param],
}

const MUSIC_EFFECTS: &[(&str, &str)] = &[
    ("energy", "Énergie"),
    ("ripple", "Ondes"),
    ("scroll", "Défilement"),
    ("beat", "Battement"),
    ("plasma", "Plasma"),
    ("spectrum", "Spectre"),
    ("pulse", "Pulsation"),
];

const CALM: &[(&str, &str)] = &[
    ("", "Pas de changement"),
    ("energy", "Énergie"),
    ("ripple", "Ondes"),
    ("scroll", "Défilement"),
    ("beat", "Battement"),
    ("plasma", "Plasma"),
    ("spectrum", "Spectre"),
    ("pulse", "Pulsation"),
];

const ORIGINS: &[(&str, &str)] = &[
    ("center", "Milieu → côtés"),
    ("radial", "Centre → bords"),
    ("left", "Gauche → droite"),
    ("right", "Droite → gauche"),
    ("bottom", "Bas → haut"),
];

const AXES: &[(&str, &str)] = &[("x", "Gauche → droite"), ("y", "Bas → haut"), ("radial", "Centre → bords")];

const PALETTE_HINT: Option<&str> =
    Some("Vide : couleurs automatiques (pochette du morceau via Horizon, sinon tonalité de la musique).");

const fn range(key: &'static str, label: &'static str, min: f64, max: f64, step: f64, default: f64) -> Param {
    Param { key, label, kind: Kind::Range { min, max, step, unit: None, default }, hint: None }
}

const fn ranged(key: &'static str, label: &'static str, min: f64, max: f64, step: f64, unit: &'static str, default: f64) -> Param {
    Param { key, label, kind: Kind::Range { min, max, step, unit: Some(unit), default }, hint: None }
}

const fn boolean(key: &'static str, label: &'static str, default: bool) -> Param {
    Param { key, label, kind: Kind::Bool { default }, hint: None }
}

const fn palette(hint: Option<&'static str>) -> Param {
    Param { key: "palette", label: "Palette", kind: Kind::Palette, hint }
}

const fn select(key: &'static str, label: &'static str, options: &'static [(&'static str, &'static str)], default: &'static str) -> Param {
    Param { key, label, kind: Kind::Select { options, default }, hint: None }
}

const fn hue(key: &'static str, label: &'static str, default: f64) -> Param {
    Param { key, label, kind: Kind::Hue { default }, hint: None }
}

const fn fps(min: f64, max: f64, step: f64, default: f64) -> Param {
    range("fps", "Images par seconde", min, max, step, default)
}

pub const EFFECTS: &[EffectInfo] = &[
    EffectInfo {
        key: "ambient",
        label: "Ambiance lente",
        summary: "Un dégradé qui dérive doucement. Sans palette, il parcourt toutes les teintes à luminosité égale.",
        params: &[
            ranged("cycles_per_min", "Vitesse", 0.05, 6.0, 0.05, "cycle/min", 0.5),
            range("spread", "Étendue du dégradé", 0.0, 1.0, 0.01, 0.3),
            range("lightness", "Clarté", 0.3, 0.95, 0.01, 0.72),
            range("chroma", "Saturation", 0.0, 0.3, 0.01, 0.17),
            palette(Some("Deux couleurs ou plus remplacent la roue des teintes.")),
            fps(5.0, 60.0, 1.0, 15.0),
        ],
    },
    EffectInfo {
        key: "ambilight",
        label: "Ambilight",
        summary: "Chaque LED reprend la couleur de l'écran sous sa position.",
        params: &[
            range("spread", "Zone lue autour de chaque LED", 0.01, 0.3, 0.01, 0.08),
            ranged("smoothing_ms", "Lissage", 0.0, 600.0, 10.0, "ms", 120.0),
            range("saturation", "Saturation", 0.5, 2.0, 0.05, 1.1),
            boolean("letterbox", "Ignorer les bandes noires", true),
            range("black_threshold", "Seuil du noir", 0.01, 0.3, 0.01, 0.08),
            fps(10.0, 120.0, 5.0, 50.0),
        ],
    },
    EffectInfo {
        key: "mix",
        label: "Musique automatique",
        summary: "Enchaîne les effets musicaux au fil du morceau : change toutes les quelques mesures, passe à un effet percutant sur un drop et à un effet doux dans les passages calmes.",
        params: &[
            Param {
                key: "effects",
                label: "Effets enchaînés",
                kind: Kind::Multi { options: MUSIC_EFFECTS, default: &["energy", "ripple", "scroll", "beat", "spectrum"] },
                hint: None,
            },
            ranged("bars", "Changer toutes les", 1.0, 32.0, 1.0, "mesures", 8.0),
            boolean("switch_on_drop", "Effet percutant sur un drop", true),
            select("calm", "Passages calmes", CALM, "plasma"),
            palette(PALETTE_HINT),
            ranged("fade_ms", "Fondu entre effets", 200.0, 5000.0, 100.0, "ms", 1500.0),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "energy",
        label: "Énergie",
        summary: "Trois barres partent de l'origine : leur longueur suit les graves, les médiums et les aigus ; leurs couleurs se mélangent là où elles se recouvrent.",
        params: &[
            select("origin", "Origine", ORIGINS, "center"),
            palette(PALETTE_HINT),
            range("sensitivity", "Sensibilité", 0.3, 2.0, 0.05, 1.1),
            ranged("release_ms", "Retombée", 30.0, 1000.0, 10.0, "ms", 160.0),
            range("blur", "Douceur des bords", 0.01, 0.3, 0.01, 0.08),
            range("beat_shift", "Rotation des couleurs par temps", 0.0, 0.5, 0.01, 0.0),
            range("floor", "Lueur minimale", 0.0, 0.3, 0.01, 0.02),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "scroll",
        label: "Défilement",
        summary: "La couleur du son du moment naît à l'origine puis s'éloigne en s'éteignant, comme une cascade : le bureau montre la dernière seconde de musique.",
        params: &[
            select("origin", "Origine", ORIGINS, "center"),
            palette(PALETTE_HINT),
            ranged("travel_s", "Temps jusqu'au bord", 0.2, 4.0, 0.05, "s", 0.9),
            range("tail", "Intensité au bord", 0.0, 1.0, 0.05, 0.3),
            range("sparkle", "Éclats du charleston", 0.0, 1.0, 0.05, 0.35),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "ripple",
        label: "Ondes",
        summary: "Chaque grosse caisse lance un anneau, chaque caisse claire un anneau plus fin, le charleston fait scintiller des LED ; un drop lance une grande onde blanche.",
        params: &[
            palette(PALETTE_HINT),
            range("speed", "Vitesse des anneaux", 0.2, 3.0, 0.05, 0.9),
            range("width", "Épaisseur", 0.03, 0.4, 0.01, 0.12),
            boolean("centered", "Toujours depuis le centre", false),
            range("sparkle", "Scintillement", 0.0, 1.0, 0.05, 0.6),
            range("background", "Lueur de fond", 0.0, 0.5, 0.01, 0.12),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "beat",
        label: "Battement",
        summary: "Toute la pièce pulse sur le tempo détecté, change de couleur à chaque temps et alterne gauche / droite. Idéal pour les ampoules.",
        params: &[
            palette(PALETTE_HINT),
            range("sharpness", "Netteté de la pulsation", 0.5, 8.0, 0.1, 3.0),
            range("colors", "Couleurs par mesure", 1.0, 8.0, 1.0, 4.0),
            boolean("alternate", "Alterner gauche / droite", true),
            range("floor", "Lueur minimale", 0.0, 0.4, 0.01, 0.03),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "plasma",
        label: "Plasma",
        summary: "Des nappes de couleur ondulent ; les graves accélèrent le mouvement et le volume les fait émerger. Doux, pour les morceaux calmes.",
        params: &[
            palette(PALETTE_HINT),
            range("speed", "Vitesse au repos", 0.0, 1.0, 0.01, 0.12),
            range("reactivity", "Accélération par les graves", 0.0, 4.0, 0.1, 1.2),
            range("scale", "Taille des motifs", 0.3, 4.0, 0.1, 1.4),
            range("floor", "Lueur minimale", 0.0, 0.5, 0.01, 0.06),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "spectrum",
        label: "Spectre",
        summary: "Les fréquences sont réparties sur les LED, des graves aux aigus.",
        params: &[
            select("axis", "Direction", AXES, "x"),
            boolean("mirror", "Graves au centre", true),
            hue("hue_low", "Teinte des graves", 0.08),
            hue("hue_high", "Teinte des aigus", 0.75),
            range("floor", "Lueur minimale", 0.0, 0.3, 0.01, 0.04),
            ranged("attack_ms", "Montée", 0.0, 200.0, 5.0, "ms", 25.0),
            ranged("release_ms", "Retombée", 20.0, 1500.0, 10.0, "ms", 220.0),
            range("punch", "Coup de la grosse caisse", 0.0, 1.0, 0.05, 0.35),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "pulse",
        label: "Pulsation",
        summary: "Une onde part du centre au rythme des basses ; chaque battement décale la teinte.",
        params: &[
            hue("hue", "Teinte de départ", 0.85),
            ranged("hue_drift_per_min", "Dérive de teinte", 0.0, 10.0, 0.1, "tour/min", 1.0),
            range("beat_hue_step", "Saut par battement", 0.0, 0.3, 0.01, 0.06),
            range("floor", "Lueur minimale", 0.0, 0.3, 0.01, 0.05),
            ranged("release_ms", "Retombée", 20.0, 1500.0, 10.0, "ms", 300.0),
            fps(20.0, 120.0, 5.0, 60.0),
        ],
    },
    EffectInfo {
        key: "solid",
        label: "Couleur fixe",
        summary: "Une seule couleur, sans animation. Le moteur ne calcule plus rien.",
        params: &[Param { key: "color", label: "Couleur", kind: Kind::Color { default: "#ffb36b" }, hint: None }],
    },
    EffectInfo { key: "off", label: "Éteint", summary: "Toutes les LED éteintes.", params: &[] },
];

pub fn effect(key: &str) -> Option<&'static EffectInfo> {
    EFFECTS.iter().find(|e| e.key == key)
}

/// Effet avec tous ses paramètres par défaut.
pub fn default_spec(effect_key: &str) -> Value {
    let mut spec = Map::new();
    spec.insert("effect".into(), json!(effect_key));
    for p in effect(effect_key).map_or(&[][..], |e| e.params) {
        spec.insert(p.key.into(), p.default_value());
    }
    Value::Object(spec)
}

/// Nom affiché d'une scène.
pub fn scene_label(key: &str) -> String {
    match key {
        "idle" => "Ambiance".into(),
        "video" => "Ambilight".into(),
        "music" => "Musique".into(),
        "off" => "Éteint".into(),
        _ => {
            let mut c = key.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Scènes usuelles d'abord, « éteint » en dernier, les scènes personnelles entre les deux
/// (ordre alphabétique).
pub fn order_scenes<S: AsRef<str>>(list: &[S]) -> Vec<String> {
    const ORDER: [&str; 3] = ["idle", "video", "music"];
    let rank = |s: &str| if s == "off" { 1000 } else { ORDER.iter().position(|o| *o == s).unwrap_or(100) };
    let mut v: Vec<String> = list.iter().map(|s| s.as_ref().to_owned()).collect();
    v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    v
}

/// Valeur d'un curseur, avec ses décimales et son unité (`fmt` de ParamControl).
pub fn format_range(v: f64, step: f64, unit: Option<&str>) -> String {
    let decimals = if step >= 1.0 {
        0
    } else if step >= 0.1 {
        1
    } else {
        2
    };
    let n = format!("{v:.decimals$}");
    match unit {
        Some(u) => format!("{n} {u}"),
        None => n,
    }
}

/// Valeur d'un curseur ramenée à son pas (les curseurs de Slint sont en `f32`).
pub fn snap(v: f64, step: f64, min: f64, max: f64) -> f64 {
    let s = if step > 0.0 { ((v - min) / step).round() * step + min } else { v };
    (s.clamp(min, max) * 1e6).round() / 1e6
}

/// Nom de scène valide (`/^[a-z0-9_-]{1,24}$/`).
pub fn valid_scene_name(name: &str) -> bool {
    (1..=24).contains(&name.chars().count())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_spec_has_every_param() {
        let s = default_spec("mix");
        assert_eq!(s["effect"], json!("mix"));
        assert_eq!(s["effects"], json!(["energy", "ripple", "scroll", "beat", "spectrum"]));
        assert_eq!(s["calm"], json!("plasma"));
        assert_eq!(s["palette"], json!([]));
        assert_eq!(default_spec("off"), json!({ "effect": "off" }));
        assert_eq!(default_spec("solid")["color"], json!("#ffb36b"));
    }

    #[test]
    fn scenes_are_named_and_ordered() {
        assert_eq!(scene_label("idle"), "Ambiance");
        assert_eq!(scene_label("focus"), "Focus");
        assert_eq!(order_scenes(&["off", "zen", "music", "claude", "idle", "video"]), ["idle", "video", "music", "claude", "zen", "off"]);
    }

    #[test]
    fn ranges_are_formatted_and_snapped() {
        assert_eq!(format_range(0.5, 0.05, Some("cycle/min")), "0.50 cycle/min");
        assert_eq!(format_range(1.1, 0.1, None), "1.1");
        assert_eq!(format_range(120.0, 10.0, Some("ms")), "120 ms");
        assert_eq!(snap(0.150_000_01, 0.05, 0.05, 6.0), 0.15);
        assert_eq!(snap(7.0, 0.05, 0.05, 6.0), 6.0);
        assert_eq!(snap(0.33, 0.01, 0.3, 0.95), 0.33);
    }

    #[test]
    fn scene_names_are_checked() {
        assert!(valid_scene_name("zen-2"));
        assert!(!valid_scene_name("Zen"));
        assert!(!valid_scene_name(""));
        assert!(!valid_scene_name(&"a".repeat(25)));
    }
}
