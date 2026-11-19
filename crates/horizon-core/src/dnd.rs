//! « Ne pas déranger » commun : état (bascule manuelle, séance du minuteur), options lues dans
//! les réglages du widget Notifications, et filtrage de ce que l'îlot dynamique montre.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Ce qui peut s'afficher dans l'îlot dynamique.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertKind {
    /// Notification Windows.
    Notification,
    /// « Claude attend » (îlot et signal lumineux).
    Claude,
    /// Batterie faible (casque, téléphone, montre).
    Battery,
    /// Serveur du homelab hors ligne ou revenu : alerte d'état, comme la batterie.
    Server,
    /// Fin d'un minuteur ou changement de phase du pomodoro : demandé par l'utilisateur.
    Timer,
    /// Retour d'une action que l'utilisateur vient de faire (palette de commandes…).
    Feedback,
    /// Nouvelle version d'Horizon prête : rien d'urgent (elle s'installe au démarrage suivant,
    /// et la page État la montre), donc masquée pendant le NPD.
    Update,
    /// Alerte d'un plugin (`horizon.alert`) : masquée pendant le NPD, comme une notification.
    Plugin,
}

/// Options du NPD (réglages du widget Notifications).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DndOptions {
    /// « Claude attend » reste affiché pendant le NPD.
    pub claude: bool,
    /// Les alertes d'état (batterie faible, serveur hors ligne) restent affichées.
    pub battery: bool,
    /// Discord passe en « Ne pas déranger » (statut d'avant rétabli ensuite).
    pub discord: bool,
    /// Prisme tient la scène `focus` pendant le NPD.
    pub light: bool,
}

impl Default for DndOptions {
    fn default() -> Self {
        DndOptions { claude: true, battery: true, discord: true, light: false }
    }
}

impl DndOptions {
    /// Lues dans les réglages du widget Notifications (`dndClaude`, `dndBattery`,
    /// `dndDiscord`, `dndLight`) ; une clé absente garde sa valeur par défaut.
    pub fn from_widget(settings: Option<&Value>) -> DndOptions {
        let d = DndOptions::default();
        let Some(s) = settings else { return d };
        let flag = |k: &str, def: bool| s.get(k).and_then(Value::as_bool).unwrap_or(def);
        DndOptions {
            claude: flag("dndClaude", d.claude),
            battery: flag("dndBattery", d.battery),
            discord: flag("dndDiscord", d.discord),
            light: flag("dndLight", d.light),
        }
    }
}

/// L'îlot peut-il montrer cette alerte ? Hors NPD, tout passe.
pub fn allows(kind: AlertKind, active: bool, opts: &DndOptions) -> bool {
    !active
        || match kind {
            AlertKind::Notification | AlertKind::Update | AlertKind::Plugin => false,
            AlertKind::Claude => opts.claude,
            AlertKind::Battery | AlertKind::Server => opts.battery,
            AlertKind::Timer | AlertKind::Feedback => true,
        }
}

/// État du NPD : bascule manuelle (enregistrée), séance de minuteur et plage de nuit en cours.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dnd {
    pub manual: bool,
    /// Le minuteur l'a activé pour sa séance (option « NPD pendant la séance »).
    pub session: bool,
    /// Activé par la plage de nuit (option « NPD la nuit »).
    pub night: bool,
}

impl Dnd {
    pub fn active(&self) -> bool {
        self.manual || self.session || self.night
    }

    /// Début ou fin de la nuit (option « NPD la nuit »). Le bouton coupe aussi celui-ci : il
    /// ne revient qu'à la nuit suivante.
    pub fn set_night(&mut self, on: bool) {
        self.night = on;
    }

    /// Bouton « Ne pas déranger » : coupe tout s'il est actif (séance comprise), sinon l'active.
    pub fn toggle(&mut self) {
        if self.active() {
            *self = Dnd::default();
        } else {
            self.manual = true;
        }
    }

    /// Début ou fin de la séance du minuteur.
    pub fn set_session(&mut self, on: bool) {
        self.session = on;
    }
}

/// Fichier `dnd.json` : bascule manuelle et option « NPD pendant la séance » du minuteur.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DndFile {
    pub manual: bool,
    pub timer_dnd: bool,
}

impl DndFile {
    /// Fichier illisible ou absent : rien d'actif.
    pub fn parse(text: &str) -> DndFile {
        serde_json::from_str(text).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plugin_alerts_hidden_during_dnd() {
        let o = DndOptions { claude: true, battery: true, discord: true, light: true };
        assert!(!allows(AlertKind::Plugin, true, &o));
        assert!(allows(AlertKind::Plugin, false, &o));
    }

    #[test]
    fn everything_passes_outside_dnd() {
        let o = DndOptions { claude: false, battery: false, discord: false, light: false };
        for k in [
            AlertKind::Notification,
            AlertKind::Claude,
            AlertKind::Battery,
            AlertKind::Server,
            AlertKind::Timer,
            AlertKind::Feedback,
        ] {
            assert!(allows(k, false, &o));
        }
    }

    #[test]
    fn dnd_hides_notifications_and_keeps_chosen_alerts() {
        let d = DndOptions::default();
        assert!(!allows(AlertKind::Notification, true, &d));
        assert!(allows(AlertKind::Claude, true, &d));
        assert!(allows(AlertKind::Battery, true, &d));
        assert!(allows(AlertKind::Timer, true, &d));
        let strict = DndOptions { claude: false, battery: false, ..d };
        assert!(!allows(AlertKind::Claude, true, &strict));
        assert!(!allows(AlertKind::Battery, true, &strict));
        assert!(!allows(AlertKind::Server, true, &strict));
        assert!(allows(AlertKind::Server, true, &d));
        assert!(allows(AlertKind::Timer, true, &strict));
        assert!(allows(AlertKind::Feedback, true, &strict));
    }

    #[test]
    fn options_from_widget_settings() {
        assert_eq!(DndOptions::from_widget(None), DndOptions::default());
        assert_eq!(DndOptions::from_widget(Some(&Value::Null)), DndOptions::default());
        let v = json!({ "dndClaude": false, "dndLight": true, "autre": 1 });
        assert_eq!(
            DndOptions::from_widget(Some(&v)),
            DndOptions { claude: false, battery: true, discord: true, light: true }
        );
    }

    #[test]
    fn toggle_and_session() {
        let mut d = Dnd::default();
        d.toggle();
        assert!(d.active() && d.manual);
        // Séance pendant un NPD manuel : sa fin ne coupe pas le NPD.
        d.set_session(true);
        d.set_session(false);
        assert!(d.active());
        d.toggle();
        assert!(!d.active());
        // Séance seule : active, puis le NPD finit avec elle.
        d.set_session(true);
        assert!(d.active() && !d.manual);
        d.set_session(false);
        assert!(!d.active());
        d.set_session(true);
        // Le bouton coupe aussi la séance.
        d.toggle();
        assert_eq!(d, Dnd::default());
        // Nuit : active jusqu'au matin, ou jusqu'au bouton.
        d.set_night(true);
        assert!(d.active() && !d.manual);
        d.toggle();
        assert!(!d.active());
        d.set_night(true);
        d.set_night(false);
        assert!(!d.active());
    }

    #[test]
    fn file_round_trip_and_garbage() {
        let f = DndFile { manual: true, timer_dnd: true };
        assert_eq!(DndFile::parse(&serde_json::to_string(&f).unwrap()), f);
        assert_eq!(DndFile::parse("{\"manual\":true}"), DndFile { manual: true, timer_dnd: false });
        assert_eq!(DndFile::parse("pas du json"), DndFile::default());
    }
}
