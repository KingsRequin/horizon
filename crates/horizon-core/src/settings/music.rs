//! Choix des animations de la musique : fond du widget Lecture en cours, barre de progression,
//! pulsation de la barre. Ils se règlent barre par barre (`layout::BarMusic`, dans
//! `layout.json`). La section `[music]` de `settings.toml`, où ils étaient globaux en 0.7.2 et
//! 0.7.3, n'est plus que lue, une fois, pour les reprendre (`Layout::adopt_music`).
//!
//! Un choix inconnu (fichier écrit à la main, ou par une version plus récente) vaut le choix par
//! défaut : il ne rend pas le fichier illisible.

use serde::{Deserialize, Serialize};

/// Choix nommé : son identifiant dans le fichier, son libellé et son explication dans les
/// réglages.
macro_rules! choices {
    ($(#[$doc:meta])* $name:ident { $($(#[$attr:meta])* $variant:ident = $id:literal, $label:literal, $hint:literal;)+ }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
        #[serde(from = "String", into = "String")]
        pub enum $name {
            $($(#[$attr])* $variant,)+
        }

        impl $name {
            /// Tous les choix, dans l'ordre des listes.
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];

            /// Identifiant écrit dans le fichier.
            pub fn id(self) -> &'static str {
                match self {
                    $($name::$variant => $id,)+
                }
            }

            fn base_label(self) -> &'static str {
                match self {
                    $($name::$variant => $label,)+
                }
            }

            fn base_hint(self) -> &'static str {
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
        Off = "none", "Aucun", "Rien ne suit le son derrière le widget.";
        #[default]
        Halo = "halo", "Halo", "Une lueur derrière la pochette, plus forte quand le son monte.";
        Aurora = "aurora", "Aurore", "Trois nappes de couleur pour les graves, les médiums et les aigus, qui dérivent avec le son.";
        Spectrum = "spectrum", "Spectre", "Vingt-quatre barres derrière tout le widget.";
        Ribbon = "ribbon", "Ruban", "Le spectre fondu en une vague pleine, et une seconde vague plus lente derrière.";
        Dots = "dots", "Points", "Une trame de points qui s’allument par colonnes, avec leur point de crête.";
        Capsules = "capsules", "Capsules", "Cinq capsules à côté du titre, rien derrière le texte.";
    }
}

choices! {
    /// Barre de progression du widget Lecture en cours.
    Progress {
        #[default]
        Line = "line", "Simple", "Un trait qui se remplit.";
        Wave = "wave", "Vivante", "La part déjà lue ondule, d’autant plus que le son est fort, et se retend en pause.";
    }
}

choices! {
    /// Animation de la barre pendant la lecture.
    Pulse {
        Off = "none", "Aucune", "La barre ne suit pas la musique.";
        #[default]
        Edges = "edges", "Bords des îlots", "Le bord des îlots s’éclaire avec la basse et flashe à chaque grosse caisse.";
        Breath = "breath", "Respiration", "Le fond des îlots se teinte lentement avec le volume ; la pochette bat sur la grosse caisse.";
        Wave = "wave", "Onde de kick", "À chaque grosse caisse, une vague de lumière traverse chaque îlot de bout en bout.";
        Horizon = "horizon", "Ligne d’horizon", "Un filet de lumière au bas des îlots, d’où partent deux éclats à chaque grosse caisse.";
        Comet = "comet", "Lumière qui tourne", "Deux traînées de lumière font le tour de l’îlot de la lecture en cours.";
    }
}

impl Backdrop {
    pub fn label(self) -> &'static str {
        self.base_label()
    }

    pub fn hint(self) -> &'static str {
        self.base_hint()
    }

    /// Ce fond dessine les bandes du spectre (le halo ne suit que le niveau).
    pub fn wants_bands(self) -> bool {
        !matches!(self, Backdrop::Off | Backdrop::Halo)
    }

    /// Ce fond est une image de l'îlot (`music::canvas`), pas des rectangles de l'interface.
    pub fn is_painted(self) -> bool {
        matches!(self, Backdrop::Halo | Backdrop::Aurora | Backdrop::Ribbon | Backdrop::Dots)
    }
}

impl Progress {
    pub fn label(self) -> &'static str {
        self.base_label()
    }

    pub fn hint(self) -> &'static str {
        self.base_hint()
    }
}

impl Pulse {
    /// Libellé sur une barre de ce style (`graphic` : une bande, pas des îlots).
    pub fn label(self, graphic: bool) -> &'static str {
        match self {
            Pulse::Edges if graphic => "Bord de la bande",
            p => p.base_label(),
        }
    }

    /// Explication sur une barre de ce style.
    pub fn hint(self, graphic: bool) -> &'static str {
        match self {
            Pulse::Edges if graphic => "Le bord de la bande s’éclaire avec la basse et flashe à chaque grosse caisse.",
            Pulse::Breath if graphic => "La bande se teinte lentement avec le volume ; la pochette bat sur la grosse caisse.",
            Pulse::Wave if graphic => "À chaque grosse caisse, une vague de lumière traverse la bande de bout en bout.",
            Pulse::Horizon if graphic => "Un filet de lumière au bas de la bande, d’où partent deux éclats à chaque grosse caisse.",
            p => p.base_hint(),
        }
    }

    /// Ce choix a un sens sur une barre de ce style : la lumière qui tourne fait le tour d'un
    /// îlot, qu'une barre graphique n'a pas (son îlot est transparent).
    pub fn fits(self, graphic: bool) -> bool {
        !(graphic && self == Pulse::Comet)
    }
}

/// Section `[music]` de 0.7.2 et 0.7.3, lue telle quelle pour la reprise (jamais écrite).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MusicSettings {
    pub backdrop: Option<String>,
    pub progress: Option<String>,
    pub pulse: Option<String>,
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
            assert!(!p.label(false).is_empty() && !p.hint(true).is_empty());
        }
        for p in Progress::ALL {
            assert_eq!(Progress::parse(p.id()), Some(*p));
            assert!(!p.label().is_empty() && !p.hint().is_empty());
        }
        assert_eq!(Backdrop::parse(" capsules "), Some(Backdrop::Capsules));
        assert_eq!(Backdrop::parse("auto"), None, "plus de choix « automatique »");
        assert_eq!(Pulse::parse("inconnu"), None);
    }

    #[test]
    fn wording_follows_the_bar_style() {
        assert_eq!(Pulse::Edges.label(false), "Bords des îlots");
        assert_eq!(Pulse::Edges.label(true), "Bord de la bande");
        assert!(Pulse::Wave.hint(true).contains("bande") && Pulse::Wave.hint(false).contains("îlot"));
        assert!(Pulse::Comet.fits(false) && !Pulse::Comet.fits(true));
        assert!(Pulse::Horizon.fits(true));
    }

    #[test]
    fn what_each_backdrop_needs() {
        assert!(Backdrop::Spectrum.wants_bands() && Backdrop::Capsules.wants_bands() && Backdrop::Ribbon.wants_bands());
        assert!(!Backdrop::Halo.wants_bands() && !Backdrop::Off.wants_bands());
        assert!(Backdrop::Halo.is_painted() && Backdrop::Aurora.is_painted() && Backdrop::Dots.is_painted());
        assert!(!Backdrop::Spectrum.is_painted() && !Backdrop::Capsules.is_painted() && !Backdrop::Off.is_painted());
    }

    #[test]
    fn old_section_is_read_as_written() {
        assert_eq!(Settings::parse("").unwrap().music, MusicSettings::default());
        let s = Settings::parse("[music]\nbackdrop = \"auto\"\nprogress = \"wave\"\npulse = \"horizon\"\n").unwrap();
        assert_eq!(s.music.backdrop.as_deref(), Some("auto"));
        assert_eq!(s.music.pulse.as_deref(), Some("horizon"));
        assert!(!s.to_toml().contains("[music]"), "plus écrite");
    }
}
