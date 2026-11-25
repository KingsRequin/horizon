//! Animations de la musique d'une barre (`BarLayout::music`) : fond du widget Lecture en cours,
//! barre de progression, pulsation. Une barre neuve reçoit le préréglage de son style ; changer
//! de style fait suivre le préréglage s'il n'a pas été touché.

use super::{BarLayout, BarStyle, Layout};
use crate::settings::{Backdrop, MusicSettings, Progress, Pulse};
use serde::{Deserialize, Serialize};

/// Marque de la reprise des réglages globaux `[music]` de 0.7.2 et 0.7.3 (`Layout::adopt_music`).
pub(crate) const MIGRATION_MUSIC: &str = "music-per-bar";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BarMusic {
    pub backdrop: Backdrop,
    pub progress: Progress,
    pub pulse: Pulse,
}

impl Default for BarMusic {
    fn default() -> BarMusic {
        BarMusic::for_style(BarStyle::Islands)
    }
}

impl BarMusic {
    /// Préréglage d'un style : halo sur une barre en îlots, spectre sur une barre graphique ; les
    /// bords s'éclairent avec la basse.
    pub fn for_style(style: BarStyle) -> BarMusic {
        let backdrop = if style == BarStyle::Graphic { Backdrop::Spectrum } else { Backdrop::Halo };
        BarMusic { backdrop, progress: Progress::Line, pulse: Pulse::Edges }
    }

    /// Les mêmes choix, ramenés à ce qui a un sens sur une barre de ce style.
    pub fn fitted(self, style: BarStyle) -> BarMusic {
        let pulse = if self.pulse.fits(style == BarStyle::Graphic) { self.pulse } else { Pulse::Edges };
        BarMusic { pulse, ..self }
    }

    /// Ce qui est vraiment montré : rien ne suit le son si Windows demande moins d'animations.
    pub fn effective(self, reduced_motion: bool) -> BarMusic {
        if reduced_motion { BarMusic { backdrop: Backdrop::Off, progress: Progress::Line, pulse: Pulse::Off } } else { self }
    }

    /// Choix globaux de 0.7.2 et 0.7.3 sur une barre de ce style. « auto » (et rien) donnait le
    /// préréglage du style ; « Faire vivre les barres » décoché donnait aucune pulsation.
    pub fn from_legacy(legacy: &MusicSettings, bar_pulse: bool, style: BarStyle) -> BarMusic {
        let preset = BarMusic::for_style(style);
        let backdrop = legacy.backdrop.as_deref().and_then(Backdrop::parse).unwrap_or(preset.backdrop);
        let progress = legacy.progress.as_deref().and_then(Progress::parse).unwrap_or(preset.progress);
        let pulse = if bar_pulse { legacy.pulse.as_deref().and_then(Pulse::parse).unwrap_or(preset.pulse) } else { Pulse::Off };
        BarMusic { backdrop, progress, pulse }.fitted(style)
    }
}

impl BarLayout {
    /// Nouveau style : la musique suit le préréglage du nouveau style si elle avait celui de
    /// l'ancien, sinon elle est gardée (et ramenée à ce qui a un sens sur ce style).
    pub(crate) fn restyle(&mut self, style: BarStyle) {
        if style == self.style {
            return;
        }
        self.music = if self.music == BarMusic::for_style(self.style) { BarMusic::for_style(style) } else { self.music.fitted(style) };
        self.style = style;
    }
}

impl Layout {
    /// Reprend une fois les animations réglées globalement en 0.7.2 et 0.7.3 (`[music]` et
    /// `[features] bar_pulse` de settings.toml) sur chaque barre, profils compris.
    pub fn adopt_music(&mut self, legacy: &MusicSettings, bar_pulse: Option<bool>) {
        if self.migrations.iter().any(|m| m == MIGRATION_MUSIC) {
            return;
        }
        let bar_pulse = bar_pulse.unwrap_or(true);
        for b in self.bars.iter_mut().chain(self.profiles.iter_mut().flat_map(|p| p.bars.iter_mut())) {
            b.music = BarMusic::from_legacy(legacy, bar_pulse, b.style);
        }
        self.migrations.push(MIGRATION_MUSIC.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(backdrop: &str, progress: &str, pulse: &str) -> MusicSettings {
        MusicSettings { backdrop: Some(backdrop.into()), progress: Some(progress.into()), pulse: Some(pulse.into()) }
    }

    #[test]
    fn presets_follow_the_style() {
        assert_eq!(BarMusic::for_style(BarStyle::Islands).backdrop, Backdrop::Halo);
        assert_eq!(BarMusic::for_style(BarStyle::Graphic).backdrop, Backdrop::Spectrum);
        let comet = BarMusic { pulse: Pulse::Comet, ..BarMusic::default() };
        assert_eq!(comet.fitted(BarStyle::Graphic).pulse, Pulse::Edges, "pas d'îlot à faire le tour");
        assert_eq!(comet.fitted(BarStyle::Islands).pulse, Pulse::Comet);
        let off = comet.effective(true);
        assert_eq!((off.backdrop, off.progress, off.pulse), (Backdrop::Off, Progress::Line, Pulse::Off));
        assert_eq!(comet.effective(false), comet);
    }

    #[test]
    fn old_global_choices_are_taken_per_bar() {
        let auto = legacy("auto", "wave", "horizon");
        let islands = BarMusic::from_legacy(&auto, true, BarStyle::Islands);
        assert_eq!((islands.backdrop, islands.progress, islands.pulse), (Backdrop::Halo, Progress::Wave, Pulse::Horizon));
        assert_eq!(BarMusic::from_legacy(&auto, true, BarStyle::Graphic).backdrop, Backdrop::Spectrum);
        assert_eq!(BarMusic::from_legacy(&auto, false, BarStyle::Islands).pulse, Pulse::Off, "pulsation décochée");
        let ribbon = BarMusic::from_legacy(&legacy("ribbon", "line", "comet"), true, BarStyle::Graphic);
        assert_eq!((ribbon.backdrop, ribbon.pulse), (Backdrop::Ribbon, Pulse::Edges));
        assert_eq!(BarMusic::from_legacy(&MusicSettings::default(), true, BarStyle::Graphic), BarMusic::for_style(BarStyle::Graphic));
    }
}
