//! Section `[music]` : ce que les barres montrent du son pendant la lecture (fond du widget
//! Lecture en cours, animation de la pulsation).
//!
//! Un choix inconnu (fichier écrit à la main, ou par une version plus récente) vaut le choix par
//! défaut : il ne rend pas `settings.toml` illisible.

use serde::{Deserialize, Serialize};

/// Choix nommé de `settings.toml` : son identifiant dans le fichier, son libellé et son
/// explication dans la fenêtre des réglages.
macro_rules! choices {
    ($(#[$doc:meta])* $name:ident { $($(#[$attr:meta])* $variant:ident = $id:literal, $label:literal, $hint:literal;)+ }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
        #[serde(from = "String", into = "String")]
        pub enum $name {
            $($(#[$attr])* $variant,)+
        }

        impl $name {
            /// Tous les choix, dans l'ordre de la liste des réglages.
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];

            /// Identifiant écrit dans `settings.toml`.
            pub fn id(self) -> &'static str {
                match self {
                    $($name::$variant => $id,)+
                }
            }

            pub fn label(self) -> &'static str {
                match self {
                    $($name::$variant => $label,)+
                }
            }

            /// Ce que le choix montre, en une phrase.
            pub fn hint(self) -> &'static str {
                match self {
                    $($name::$variant => $hint,)+
                }
            }

            pub fn parse(id: &str) -> Option<$name> {
                $name::ALL.iter().copied().find(|c| c.id() == id.trim())
            }
        }

        impl From<String> for $name {
            fn from(id: String) -> $name {
                $name::parse(&id).unwrap_or_default()
            }
        }

        impl From<$name> for String {
            fn from(c: $name) -> String {
                c.id().to_owned()
            }
        }
    };
}

choices! {
    /// Fond du widget Lecture en cours pendant la lecture.
    Backdrop {
        #[default]
        Auto = "auto", "Automatique", "Halo sur une barre en îlots, spectre sur une barre graphique.";
        Off = "none", "Aucun", "Rien ne suit le son dans le widget.";
        Halo = "halo", "Halo", "Une lueur derrière la pochette, plus forte quand le son monte.";
        Spectrum = "spectrum", "Spectre", "Vingt-quatre barres derrière le titre.";
        Capsules = "capsules", "Capsules", "Cinq capsules à côté du titre, rien derrière le texte.";
    }
}

choices! {
    /// Animation des barres pendant la lecture (`[features] bar_pulse` actif).
    Pulse {
        #[default]
        Edges = "edges", "Bords des îlots", "Le bord des îlots s’éclaire avec la basse et flashe à chaque grosse caisse.";
        Breath = "breath", "Respiration", "Le fond des îlots se teinte lentement avec le son ; la pochette bat sur la grosse caisse.";
        Wave = "wave", "Onde de kick", "À chaque grosse caisse, une vague de lumière traverse les îlots.";
        Horizon = "horizon", "Ligne d’horizon", "Un filet de lumière sous la barre, d’où partent deux éclats à chaque grosse caisse.";
    }
}

impl Backdrop {
    /// Le fond d'une barre de ce style (`graphic` : style graphique) : `Auto` devient le halo
    /// ou le spectre.
    pub fn resolved(self, graphic: bool) -> Backdrop {
        match self {
            Backdrop::Auto if graphic => Backdrop::Spectrum,
            Backdrop::Auto => Backdrop::Halo,
            b => b,
        }
    }

    /// Ce fond dessine les bandes du spectre (le halo ne suit que le niveau).
    pub fn wants_bands(self) -> bool {
        matches!(self, Backdrop::Spectrum | Backdrop::Capsules)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    pub backdrop: Backdrop,
    pub pulse: Pulse,
}

#[cfg(test)]
mod tests {
    use super::super::Settings;
    use super::*;

    #[test]
    fn every_choice_reads_back_from_its_id() {
        for b in Backdrop::ALL {
            assert_eq!(Backdrop::parse(b.id()), Some(*b));
            assert!(!b.label().is_empty() && !b.hint().is_empty());
        }
        for p in Pulse::ALL {
            assert_eq!(Pulse::parse(p.id()), Some(*p));
            assert!(!p.label().is_empty() && !p.hint().is_empty());
        }
        assert_eq!(Backdrop::parse(" capsules "), Some(Backdrop::Capsules));
        assert_eq!(Pulse::parse("inconnu"), None);
    }

    #[test]
    fn auto_follows_the_bar_style() {
        assert_eq!(Backdrop::Auto.resolved(false), Backdrop::Halo);
        assert_eq!(Backdrop::Auto.resolved(true), Backdrop::Spectrum);
        assert_eq!(Backdrop::Capsules.resolved(true), Backdrop::Capsules);
        assert_eq!(Backdrop::Off.resolved(false), Backdrop::Off);
        assert!(Backdrop::Spectrum.wants_bands() && Backdrop::Capsules.wants_bands());
        assert!(!Backdrop::Halo.wants_bands() && !Backdrop::Off.wants_bands());
    }

    #[test]
    fn section_is_read_and_an_unknown_choice_is_the_default() {
        assert_eq!(Settings::parse("").unwrap().music, MusicSettings::default());
        let s = Settings::parse("[music]\nbackdrop = \"capsules\"\npulse = \"horizon\"\n").unwrap();
        assert_eq!(s.music, MusicSettings { backdrop: Backdrop::Capsules, pulse: Pulse::Horizon });
        let s = Settings::parse("[music]\nbackdrop = \"hologramme\"\n").unwrap();
        assert_eq!(s.music, MusicSettings::default(), "le fichier reste lisible");
    }
}
