//! Section `[features]` : fonctions facultatives (captures vers Claude, bascule audio du
//! casque, mode jeu, couleur du fond d'écran, mise à jour automatique).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureSettings {
    /// Une nouvelle capture d'écran est proposée dans l'îlot pour la session Claude qui attend.
    pub capture_to_claude: bool,
    /// Le casque s'éteint : la sortie audio par défaut passe sur `fallback_output`, et revient
    /// à la sortie d'avant quand il se rallume. Sans effet tant que `fallback_output` est vide.
    pub headset_switch: bool,
    /// Identifiant de la sortie de repli (enceintes) ; vide = pas choisie.
    pub fallback_output: String,
    /// Une appli plein écran sur l'écran principal : spectre coupé, sondages espacés.
    pub game_mode: bool,
    /// Couleur d'accent tirée du fond d'écran (relu chaque minute) : barres et ambiance de
    /// Prisme hors lecture (pendant la lecture, c'est la pochette).
    pub wallpaper_accent: bool,
    /// La couleur du fond d'écran devient aussi la couleur d'accent de Windows (celle
    /// d'origine est remise quand la fonction s'arrête).
    pub windows_accent: bool,
    /// Affichage du volume et du média dans la barre principale, à la place de celui de
    /// Windows (masqué tant que la barre principale est visible).
    pub osd: bool,
    /// Nouveau morceau : titre annoncé dans la barre principale.
    pub osd_track: bool,
    /// Pendant la lecture, les îlots s'éclairent de l'accent avec la basse et flashent à chaque
    /// grosse caisse.
    pub bar_pulse: bool,
    /// Versions publiées (Releases GitHub) : nouvelle version cherchée, téléchargée, vérifiée,
    /// puis installée au démarrage suivant.
    pub auto_update: bool,
    /// Module intégré « Sessions Claude Code » (hooks, « Claude attend », consommation du jour).
    /// `None` : jamais choisi — activé seulement s'il sert déjà (son widget est sur une barre, ou
    /// les hooks d'Horizon sont dans Claude Code).
    pub claude_sessions: Option<bool>,
    /// Module intégré « Téléphone » (liaison avec l'appli Horizon Mobile). `None` : jamais
    /// choisi — activé seulement s'il sert déjà (son widget est sur une barre, ou un téléphone
    /// est appairé).
    pub phone: Option<bool>,
}

impl Default for FeatureSettings {
    fn default() -> Self {
        FeatureSettings {
            capture_to_claude: true,
            headset_switch: false,
            fallback_output: String::new(),
            game_mode: true,
            wallpaper_accent: true,
            windows_accent: true,
            osd: true,
            osd_track: true,
            bar_pulse: true,
            auto_update: true,
            claude_sessions: None,
            phone: None,
        }
    }
}

impl FeatureSettings {
    /// Sortie de repli, si la bascule est active et la sortie choisie.
    pub fn headset_fallback(&self) -> Option<&str> {
        let id = self.fallback_output.trim();
        (self.headset_switch && !id.is_empty()).then_some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_needs_a_chosen_output() {
        let mut f = FeatureSettings::default();
        assert_eq!(f.headset_fallback(), None, "désactivée par défaut");
        f.headset_switch = true;
        assert_eq!(f.headset_fallback(), None, "pas de sortie choisie : rien");
        f.fallback_output = " {0.0.0}.{abc} ".into();
        assert_eq!(f.headset_fallback(), Some("{0.0.0}.{abc}"));
    }
}
