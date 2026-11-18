//! Scène forcée temporaire par Prisme, puis retour à la scène forcée d'avant (ou aux règles
//! automatiques) :
//!  * bref signal (« Claude attend », fin du minuteur) : Prisme n'a pas de commande
//!    « couleur », la scène `claude` (à créer dans Prisme, effet `solid`) sert de signal ;
//!    sans elle, un bref passage par `off` fait clignoter l'éclairage ;
//!  * maintien de la scène `focus` pendant « Ne pas déranger » (rien si elle n'existe pas).

/// Scène préférée pour le signal.
pub(crate) const SIGNAL_SCENE: &str = "claude";
/// Scène de repli (éteint) quand `claude` n'existe pas.
pub(crate) const FALLBACK_SCENE: &str = "off";
/// Scène tenue pendant « Ne pas déranger ».
pub(crate) const FOCUS_SCENE: &str = "focus";

/// Nom affiché d'une scène de Prisme : celui de son interface (`ui/src/lib/effects.ts`) pour
/// les scènes livrées, sinon la clé avec une majuscule.
pub fn scene_name(key: &str) -> String {
    match key {
        "idle" => "Ambiance".into(),
        "video" => "Ambilight".into(),
        "music" => "Musique".into(),
        "off" => "Éteint".into(),
        k => {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

#[derive(Debug)]
pub struct LightSignal {
    /// Scènes possibles, par préférence.
    prefs: &'static [&'static str],
    /// Scène forcée avant le signal en cours (`Some(None)` = règles automatiques) ;
    /// `None` = pas de signal en cours.
    saved: Option<Option<String>>,
    /// Scène envoyée pour le signal en cours.
    scene: Option<String>,
    generation: u64,
}

impl Default for LightSignal {
    /// Bref signal : scène `claude`, sinon extinction.
    fn default() -> Self {
        LightSignal { prefs: &[SIGNAL_SCENE, FALLBACK_SCENE], saved: None, scene: None, generation: 0 }
    }
}

impl LightSignal {
    /// Maintien de la scène `focus` (pas de repli).
    pub fn focus() -> LightSignal {
        LightSignal { prefs: &[FOCUS_SCENE], ..LightSignal::default() }
    }

    /// Début d'un signal. Renvoie la scène à forcer et le numéro du signal à rappeler à la
    /// fin, ou `None` s'il ne faut rien faire : l'utilisateur a éteint l'éclairage, ou
    /// aucune scène ne convient. Un signal qui arrive pendant un autre le prolonge sans
    /// écraser la scène d'avant.
    pub fn start(&mut self, current_override: Option<&str>, scenes: &[String]) -> Option<(String, u64)> {
        if self.saved.is_none() && current_override == Some(FALLBACK_SCENE) {
            return None;
        }
        let scene = *self.prefs.iter().find(|s| scenes.iter().any(|x| x == *s))?;
        if self.saved.is_none() {
            self.saved = Some(current_override.map(str::to_owned));
        }
        self.scene = Some(scene.to_owned());
        self.generation += 1;
        Some((scene.to_owned(), self.generation))
    }

    /// Scène à rétablir à la fin du signal en cours (`Some(None)` = règles automatiques),
    /// `None` hors signal.
    pub fn pending(&self) -> Option<Option<String>> {
        self.saved.clone()
    }

    /// Change la scène à rétablir à la fin du signal en cours (un maintien commence ou
    /// finit pendant le signal) ; sans effet hors signal.
    pub fn set_pending(&mut self, scene: Option<String>) {
        if self.saved.is_some() {
            self.saved = Some(scene);
        }
    }

    /// Numéro du dernier signal commencé.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Fin du signal numéro `generation` : renvoie la scène forcée à rétablir
    /// (`Some(None)` = règles automatiques), ou `None` si ce signal a été prolongé par un
    /// autre ou si l'utilisateur a changé de scène entre-temps (on ne touche à rien).
    pub fn finish(&mut self, generation: u64, current_override: Option<&str>) -> Option<Option<String>> {
        if generation != self.generation || self.saved.is_none() {
            return None;
        }
        let saved = self.saved.take()?;
        let ours = self.scene.take();
        (current_override == ours.as_deref()).then_some(saved)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn scene_names_follow_prisme() {
        assert_eq!(super::scene_name("music"), "Musique");
        assert_eq!(super::scene_name("idle"), "Ambiance");
        assert_eq!(super::scene_name("focus"), "Focus");
        assert_eq!(super::scene_name(""), "");
    }

    use super::*;

    fn scenes(with_claude: bool) -> Vec<String> {
        let mut v: Vec<String> = ["idle", "music", "off", "video"].map(String::from).to_vec();
        if with_claude {
            v.push("claude".into());
        }
        v
    }

    #[test]
    fn signal_then_back_to_automatic() {
        let mut s = LightSignal::default();
        let (scene, g) = s.start(None, &scenes(true)).unwrap();
        assert_eq!(scene, "claude");
        assert_eq!(s.finish(g, Some("claude")), Some(None));
    }

    #[test]
    fn previous_override_is_restored() {
        let mut s = LightSignal::default();
        let (_, g) = s.start(Some("video"), &scenes(true)).unwrap();
        assert_eq!(s.finish(g, Some("claude")), Some(Some("video".into())));
    }

    #[test]
    fn falls_back_to_off_without_claude_scene() {
        let mut s = LightSignal::default();
        let (scene, g) = s.start(Some("music"), &scenes(false)).unwrap();
        assert_eq!(scene, "off");
        assert_eq!(s.finish(g, Some("off")), Some(Some("music".into())));
    }

    #[test]
    fn lights_off_by_user_are_left_alone() {
        let mut s = LightSignal::default();
        assert_eq!(s.start(Some("off"), &scenes(true)), None);
        assert_eq!(s.start(None, &[]), None);
    }

    #[test]
    fn user_change_during_signal_is_kept() {
        let mut s = LightSignal::default();
        let (_, g) = s.start(None, &scenes(true)).unwrap();
        assert_eq!(s.finish(g, Some("video")), None);
        // Prisme n'a jamais appliqué le signal (scène refusée) : rien à défaire non plus.
        let (_, g) = s.start(None, &scenes(true)).unwrap();
        assert_eq!(s.finish(g, None), None);
    }

    #[test]
    fn overlapping_signals_restore_the_original_scene() {
        let mut s = LightSignal::default();
        let (_, g1) = s.start(Some("video"), &scenes(true)).unwrap();
        // Deuxième alerte pendant le signal : Prisme montre déjà « claude ».
        let (_, g2) = s.start(Some("claude"), &scenes(true)).unwrap();
        assert_eq!(s.finish(g1, Some("claude")), None);
        assert_eq!(s.finish(g2, Some("claude")), Some(Some("video".into())));
        // Fin déjà traitée : plus rien.
        assert_eq!(s.finish(g2, Some("claude")), None);
    }

    #[test]
    fn focus_hold_needs_its_scene() {
        let mut h = LightSignal::focus();
        assert_eq!(h.start(Some("music"), &scenes(true)), None);
        let mut with_focus = scenes(false);
        with_focus.push("focus".into());
        let (scene, g) = h.start(Some("music"), &with_focus).unwrap();
        assert_eq!(scene, "focus");
        assert_eq!(h.finish(g, Some("focus")), Some(Some("music".into())));
    }

    #[test]
    fn signal_during_focus_hold_comes_back_to_focus_then_hold_restores() {
        let mut with_focus = scenes(true);
        with_focus.push("focus".into());
        let mut hold = LightSignal::focus();
        let mut sig = LightSignal::default();
        let (_, gh) = hold.start(None, &with_focus).unwrap();
        // Signal pendant le maintien : il revient à « focus ».
        let (_, gs) = sig.start(Some("focus"), &with_focus).unwrap();
        assert_eq!(sig.pending(), Some(Some("focus".into())));
        // Fin du maintien pendant le signal : le maintien se résout sur la scène que le
        // signal rétablira, puis le signal rétablit celle d'avant le maintien.
        let back = hold.finish(gh, sig.pending().unwrap().as_deref()).unwrap();
        sig.set_pending(back);
        assert_eq!(sig.finish(gs, Some("claude")), Some(None));
        // Hors signal, set_pending ne fait rien.
        sig.set_pending(Some("x".into()));
        assert_eq!(sig.pending(), None);
    }

    #[test]
    fn signal_over_fallback_off_still_restores() {
        // Signal de repli (« off ») en cours, nouvelle alerte : pas confondu avec un
        // éclairage éteint par l'utilisateur.
        let mut s = LightSignal::default();
        let (_, g1) = s.start(None, &scenes(false)).unwrap();
        let (_, g2) = s.start(Some("off"), &scenes(false)).unwrap();
        assert!(g2 > g1);
        assert_eq!(s.finish(g2, Some("off")), Some(None));
    }
}
