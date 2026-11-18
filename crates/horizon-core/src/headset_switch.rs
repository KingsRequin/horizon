//! Bascule de la sortie audio selon le casque : éteint (confirmé par plusieurs lectures),
//! la sortie par défaut passe sur la sortie de repli ; rallumé, elle revient à celle d'avant,
//! sauf si quelqu'un a choisi une autre sortie entre-temps. Anti-rebond : lectures
//! successives pour « éteint », délai minimal entre deux bascules.
//!
//! Au démarrage casque éteint, seule la sortie du casque lui-même (reconnue par son appareil
//! USB, `headset_output`) est quittée ; son déjà sur le repli, le casque la reprendra quand il
//! s'allumera. Sortie du casque inconnue (Bluetooth…) : rien au démarrage.
//! `retry_in` dit quand relire pour trancher sans attendre la lecture périodique.

/// Lectures « éteint » d'affilée avant de basculer (une réponse perdue arrive).
pub const OFF_CONFIRM: u32 = 2;
/// Attente avant la lecture qui confirme « éteint ».
pub const CONFIRM_MS: u64 = 300;
/// Délai minimal entre deux bascules (casque qui clignote en limite de portée).
pub const MIN_GAP_MS: u64 = 5_000;

/// Bascule à faire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// Vers la sortie de repli.
    ToFallback(String),
    /// Retour à la sortie d'avant.
    Back(String),
}

#[derive(Debug, Default)]
pub struct HeadsetSwitch {
    /// Dernier état confirmé du casque ; `None` = pas encore lu.
    on: Option<bool>,
    off_reads: u32,
    /// Sortie à reprendre au retour du casque (celle quittée par la bascule).
    restore: Option<String>,
    last_switch_ms: Option<u64>,
    /// Bascule due mais retenue par `MIN_GAP_MS`.
    held: bool,
}

impl HeadsetSwitch {
    /// Nouvelle lecture : casque allumé ou non, sortie par défaut actuelle, sortie de repli,
    /// sortie audio du casque si elle est connue.
    pub fn observe(
        &mut self,
        headset_on: bool,
        current: Option<&str>,
        fallback: &str,
        headset_output: Option<&str>,
        now_ms: u64,
    ) -> Option<Switch> {
        let gap_ok = self.last_switch_ms.is_none_or(|t| now_ms.saturating_sub(t) >= MIN_GAP_MS);
        self.held = false;
        if headset_on {
            self.off_reads = 0;
            if self.on == Some(false)
                && let Some(back) = self.restore.clone()
            {
                // Quelqu'un a choisi une autre sortie pendant l'absence : on la respecte.
                if current != Some(fallback) {
                    self.restore = None;
                } else if !gap_ok {
                    // Trop tôt : réessayé à la lecture suivante, état inchangé.
                    self.held = true;
                    return None;
                } else {
                    self.on = Some(true);
                    self.restore = None;
                    self.last_switch_ms = Some(now_ms);
                    return Some(Switch::Back(back));
                }
            }
            self.on = Some(true);
            return None;
        }
        self.off_reads = self.off_reads.saturating_add(1);
        if self.off_reads < OFF_CONFIRM || self.on == Some(false) {
            return None;
        }
        let was_on = self.on == Some(true);
        if was_on && !gap_ok {
            self.held = true;
            return None;
        }
        // Sortie actuelle pas encore lue (démarrage) : décidé à la lecture suivante.
        let current = current?;
        self.on = Some(false);
        if current == fallback {
            // Démarrage casque éteint, son déjà sur le repli : reprendre le casque à l'allumage.
            if !was_on && let Some(h) = headset_output {
                self.restore = Some(h.to_owned());
            }
            return None;
        }
        // Au démarrage, une autre sortie que le casque a été choisie ainsi : on la laisse.
        if !was_on && Some(current) != headset_output {
            return None;
        }
        self.restore = Some(current.to_owned());
        self.last_switch_ms = Some(now_ms);
        Some(Switch::ToFallback(fallback.to_owned()))
    }

    /// Délai avant la prochaine lecture utile quand une décision attend : « éteint » à
    /// confirmer, ou bascule retenue par le délai minimal. `None` : rien en attente.
    pub fn retry_in(&self, now_ms: u64) -> Option<u64> {
        if self.held {
            let since = self.last_switch_ms.map_or(MIN_GAP_MS, |t| now_ms.saturating_sub(t));
            return Some(MIN_GAP_MS.saturating_sub(since).max(CONFIRM_MS));
        }
        (self.off_reads > 0 && self.off_reads < OFF_CONFIRM && self.on != Some(false)).then_some(CONFIRM_MS)
    }

    /// Bascule désactivée (réglage) : rien à reprendre.
    pub fn reset(&mut self) {
        self.restore = None;
        self.held = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FB: &str = "enceintes";

    #[test]
    fn off_is_confirmed_then_back_on_return() {
        let mut s = HeadsetSwitch::default();
        assert_eq!(s.observe(true, Some("casque"), FB, None, 0), None);
        assert_eq!(s.observe(false, Some("casque"), FB, None, 30_000), None, "une lecture perdue ne suffit pas");
        assert_eq!(s.observe(false, Some("casque"), FB, None, 35_000), Some(Switch::ToFallback(FB.into())));
        assert_eq!(s.observe(false, Some(FB), FB, None, 40_000), None, "une seule bascule");
        assert_eq!(s.observe(true, Some(FB), FB, None, 120_000), Some(Switch::Back("casque".into())));
        assert_eq!(s.observe(true, Some("casque"), FB, None, 125_000), None);
    }

    #[test]
    fn single_lost_read_resets() {
        let mut s = HeadsetSwitch::default();
        s.observe(true, Some("casque"), FB, None, 0);
        s.observe(false, Some("casque"), FB, None, 5_000);
        s.observe(true, Some("casque"), FB, None, 10_000);
        assert_eq!(s.observe(false, Some("casque"), FB, None, 15_000), None);
    }

    #[test]
    fn nothing_at_startup_with_headset_off_and_unknown_output() {
        let mut s = HeadsetSwitch::default();
        assert_eq!(s.observe(false, Some("casque"), FB, None, 0), None);
        assert_eq!(s.observe(false, Some("casque"), FB, None, 5_000), None);
        // Rallumé ensuite : rien à reprendre.
        assert_eq!(s.observe(true, Some("casque"), FB, None, 10_000), None);
    }

    #[test]
    fn startup_with_headset_off_leaves_its_output() {
        let mut s = HeadsetSwitch::default();
        let hs = Some("casque");
        assert_eq!(s.observe(false, hs, FB, hs, 0), None);
        assert_eq!(s.observe(false, hs, FB, hs, 300), Some(Switch::ToFallback(FB.into())));
        assert_eq!(s.observe(true, Some(FB), FB, hs, 60_000), Some(Switch::Back("casque".into())));
    }

    #[test]
    fn startup_waits_for_the_current_output() {
        let mut s = HeadsetSwitch::default();
        let hs = Some("casque");
        s.observe(false, None, FB, hs, 0);
        assert_eq!(s.observe(false, None, FB, hs, 300), None);
        assert_eq!(s.observe(false, hs, FB, hs, 5_300), Some(Switch::ToFallback(FB.into())));
    }

    #[test]
    fn startup_on_fallback_takes_the_headset_when_it_turns_on() {
        let mut s = HeadsetSwitch::default();
        let hs = Some("casque");
        assert_eq!(s.observe(false, Some(FB), FB, hs, 0), None);
        assert_eq!(s.observe(false, Some(FB), FB, hs, 300), None);
        assert_eq!(s.observe(true, Some(FB), FB, hs, 60_000), Some(Switch::Back("casque".into())));
    }

    #[test]
    fn startup_on_another_output_is_left_alone() {
        let mut s = HeadsetSwitch::default();
        let hs = Some("casque");
        s.observe(false, Some("tv"), FB, hs, 0);
        assert_eq!(s.observe(false, Some("tv"), FB, hs, 300), None);
        assert_eq!(s.observe(true, Some("tv"), FB, hs, 60_000), None);
    }

    #[test]
    fn fallback_chosen_while_headset_on_is_kept() {
        // Son mis sur le repli casque allumé, puis casque éteint et rallumé : on n'y touche pas.
        let mut s = HeadsetSwitch::default();
        let hs = Some("casque");
        s.observe(true, Some(FB), FB, hs, 0);
        s.observe(false, Some(FB), FB, hs, 5_000);
        assert_eq!(s.observe(false, Some(FB), FB, hs, 5_300), None);
        assert_eq!(s.observe(true, Some(FB), FB, hs, 60_000), None);
    }

    #[test]
    fn manual_choice_during_absence_is_kept() {
        let mut s = HeadsetSwitch::default();
        s.observe(true, Some("casque"), FB, None, 0);
        s.observe(false, Some("casque"), FB, None, 5_000);
        assert!(s.observe(false, Some("casque"), FB, None, 10_000).is_some());
        assert_eq!(s.observe(true, Some("tv"), FB, None, 60_000), None);
    }

    #[test]
    fn already_on_fallback_does_nothing() {
        let mut s = HeadsetSwitch::default();
        s.observe(true, Some(FB), FB, None, 0);
        s.observe(false, Some(FB), FB, None, 5_000);
        assert_eq!(s.observe(false, Some(FB), FB, None, 10_000), None);
        assert_eq!(s.observe(true, Some(FB), FB, None, 60_000), None);
    }

    #[test]
    fn flapping_headset_waits_for_the_gap() {
        let mut s = HeadsetSwitch::default();
        s.observe(true, Some("casque"), FB, None, 0);
        s.observe(false, Some("casque"), FB, None, 5_000);
        assert!(s.observe(false, Some("casque"), FB, None, 10_000).is_some());
        // Rallumé 2 s après : trop tôt, réessayé quand le délai est passé.
        assert_eq!(s.observe(true, Some(FB), FB, None, 12_000), None);
        assert_eq!(s.retry_in(12_000), Some(3_000));
        assert_eq!(s.observe(true, Some(FB), FB, None, 15_000), Some(Switch::Back("casque".into())));
        assert_eq!(s.retry_in(15_000), None);
    }

    #[test]
    fn off_is_confirmed_quickly() {
        let mut s = HeadsetSwitch::default();
        s.observe(true, Some("casque"), FB, None, 0);
        assert_eq!(s.retry_in(0), None, "rien en attente");
        s.observe(false, Some("casque"), FB, None, 60_000);
        assert_eq!(s.retry_in(60_000), Some(CONFIRM_MS));
        assert!(s.observe(false, Some("casque"), FB, None, 60_300).is_some());
        assert_eq!(s.retry_in(60_300), None, "éteint confirmé : plus rien à relire");
    }
}
