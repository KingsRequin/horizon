//! Section `[alerts]` : alertes et « Ne pas déranger », réglages globaux. Avant elle, ces
//! options étaient lues dans le premier widget de chaque genre (Notifications pour le NPD,
//! Sessions Claude Code pour « Claude attend », Casque pour le seuil) : une
//! clé absente du fichier garde cette valeur, jusqu'au premier enregistrement depuis la
//! fenêtre « Réglages d'Horizon ».

use crate::claude_sessions::AlertOptions;
use crate::dnd::DndOptions;
use crate::layout::{Layout, WidgetKind};
use crate::low_battery;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertSettings {
    /// « Claude attend » : projet et message dans l'îlot.
    pub claude_island: Option<bool>,
    /// « Claude attend » : signal lumineux (événement `alert` des plugins, Prisme le joue).
    pub claude_light: Option<bool>,
    /// Pendant « Ne pas déranger », l'îlot montre quand même « Claude attend ».
    pub dnd_claude: Option<bool>,
    /// … et les alertes d'état (batterie faible, serveur hors ligne).
    pub dnd_battery: Option<bool>,
    /// « Ne pas déranger » passe Discord en « Ne pas déranger ».
    pub dnd_discord: Option<bool>,
    /// « Ne pas déranger » tient la scène `focus` de Prisme (événement `dnd` des plugins).
    pub dnd_light: Option<bool>,
    /// Seuil de batterie faible du casque, en % (0 = pas d'alerte).
    pub headset_low: Option<u8>,
}

/// Seuil réglé : 0 = alerte coupée.
fn threshold(v: u8) -> Option<u8> {
    (v > 0).then_some(v.min(100))
}

impl AlertSettings {
    /// Options de « Ne pas déranger ».
    pub fn dnd(&self, layout: &Layout) -> DndOptions {
        let old = DndOptions::from_widget(layout.first_of(WidgetKind::NOTIFICATIONS).map(|w| &w.settings));
        DndOptions {
            claude: self.dnd_claude.unwrap_or(old.claude),
            battery: self.dnd_battery.unwrap_or(old.battery),
            discord: self.dnd_discord.unwrap_or(old.discord),
            light: self.dnd_light.unwrap_or(old.light),
        }
    }

    /// Alerte « Claude attend ».
    pub fn claude(&self, layout: &Layout) -> AlertOptions {
        let old = AlertOptions::from_widget(layout.first_of(WidgetKind::CLAUDE_SESSIONS).map(|w| &w.settings));
        AlertOptions { island: self.claude_island.unwrap_or(old.island), light: self.claude_light.unwrap_or(old.light) }
    }

    /// Seuil de batterie faible du casque ; `None` = pas d'alerte.
    pub fn headset_threshold(&self, layout: &Layout) -> Option<u8> {
        match self.headset_low {
            Some(v) => threshold(v),
            None => low_battery::threshold_in(layout, WidgetKind::HEADSET),
        }
    }

    /// Toutes les valeurs, celles d'avant comprises (pour la fenêtre de réglages).
    pub fn resolved(&self, layout: &Layout) -> AlertSettings {
        let (dnd, claude) = (self.dnd(layout), self.claude(layout));
        AlertSettings {
            claude_island: Some(claude.island),
            claude_light: Some(claude.light),
            dnd_claude: Some(dnd.claude),
            dnd_battery: Some(dnd.battery),
            dnd_discord: Some(dnd.discord),
            dnd_light: Some(dnd.light),
            headset_low: Some(self.headset_threshold(layout).unwrap_or(0)),
        }
    }

    /// Seuils ramenés à 100 % au plus.
    pub(super) fn bounded(mut self) -> AlertSettings {
        self.headset_low = self.headset_low.map(|v| v.min(100));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::real_monitors;
    use serde_json::json;

    /// Disposition de départ avec un widget `kind` ainsi réglé (ajouté s'il n'y est pas).
    fn with(kind: WidgetKind, settings: serde_json::Value) -> Layout {
        let mut l = Layout::default_for(&real_monitors());
        let id = match l.first_of(kind) {
            Some(w) => w.id.clone(),
            None => l.add_widget("main", crate::layout::Zone::Right, kind).unwrap(),
        };
        l.set_widget_settings(&id, settings).unwrap();
        l
    }

    #[test]
    fn missing_keys_keep_the_widget_values() {
        let l = with(WidgetKind::NOTIFICATIONS, json!({ "dndDiscord": false }));
        let a = AlertSettings::default();
        assert!(!a.dnd(&l).discord);
        let a = AlertSettings { dnd_discord: Some(true), ..AlertSettings::default() };
        assert!(a.dnd(&l).discord, "la valeur du fichier l'emporte");
        let l = with(WidgetKind::CLAUDE_SESSIONS, json!({ "alertLight": false }));
        assert_eq!(AlertSettings::default().claude(&l), AlertOptions { island: true, light: false });
    }

    #[test]
    fn global_values_work_without_any_widget() {
        // Une barre neuve n'a ni Sessions Claude ni Casque.
        let l = Layout::default_for(&real_monitors());
        assert_eq!(AlertSettings::default().claude(&l), AlertOptions { island: false, light: false });
        assert_eq!(AlertSettings::default().headset_threshold(&l), None);
        let a = AlertSettings { claude_island: Some(true), headset_low: Some(20), ..AlertSettings::default() };
        assert!(a.claude(&l).island);
        assert_eq!(a.headset_threshold(&l), Some(20));
        let off = AlertSettings { headset_low: Some(0), ..AlertSettings::default() };
        assert_eq!(off.headset_threshold(&l), None);
    }

    #[test]
    fn resolved_fills_everything() {
        let mut l = with(WidgetKind::HEADSET, json!({ "lowAlert": 30 }));
        l.add_widget("main", crate::layout::Zone::Right, WidgetKind::CLAUDE_SESSIONS).unwrap();
        let r = AlertSettings::default().resolved(&l);
        assert_eq!(r.headset_low, Some(30));
        assert_eq!(r.dnd_light, Some(false));
        assert_eq!(r.claude_island, Some(true));
    }
}
