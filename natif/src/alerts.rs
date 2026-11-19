//! Îlot dynamique : un seul canal pour tout ce qui s'y montre quelques secondes — notification
//! Windows qui arrive, « Claude attend », batterie faible du casque, du téléphone, de la
//! montre, serveur hors ligne ou revenu, fin du minuteur, retour d'une action, « Nouveautés ».
//! « Ne pas déranger » filtre ici, et seulement ici (`allowed`). L'affichage (durée, fondu,
//! appel du téléphone, clic) est dans `ui::island`.
//!
//! Repris de `app/src-tauri/src/alerts.rs`. Différences : la miniature d'une capture est en
//! pixels (`Thumb`, pas d'URL `data:`) ; l'annonce des « Nouveautés » (`announce_news`,
//! `app/src-tauri/src/edit.rs`) est ici.

use crate::events::EV_ALERT;
use crate::host::{AppHandle, Emitter, Manager};
use horizon_core::dnd::AlertKind;
use horizon_core::layout::WidgetKind;
use horizon_core::low_battery::LowAlert;
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Gravité : teinte de l'îlot (accent, `--warn`, `--bad`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Information (serveur revenu, notification, retour d'une action).
    Info,
    /// À regarder bientôt (minuteur terminé, Claude attend).
    Attention,
    /// Problème (batterie faible, serveur hors ligne).
    Alert,
}

/// Ce qui est montré à gauche du texte.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// Logo d'une appli (notification Windows, Claude).
    App { name: String, app_id: String },
    /// Icône de l'interface (`icons/phosphor`, noms de `lib/icons.ts`).
    Icon { icon: &'static str },
}

/// Action d'un clic sur l'alerte.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Ramène au premier plan le terminal de la session Claude Code.
    Session { id: String },
    /// Ouvre le mode édition de la barre (« Nouveautés »).
    Edit,
    /// Colle le chemin d'une capture dans le terminal de la session (`capture_send`).
    Capture { path: String, session: String },
    /// Installe tout de suite la nouvelle version prête et redémarre Horizon.
    Update,
}

/// Image RGBA déjà arrondie, à poser telle quelle.
#[derive(Debug, Clone, PartialEq)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// Miniature d'une capture : celle de l'îlot (40 × 24) et celle de la ligne de notification
/// (32 × 20), chacune à ses coins.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumb {
    pub island: Pixels,
    pub line: Pixels,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    /// Numéro unique (l'îlot rejoue son entrée à chaque alerte).
    pub id: u64,
    pub source: Source,
    pub title: String,
    pub body: String,
    pub severity: Severity,
    pub action: Option<Action>,
    /// Miniature montrée à la place du logo.
    #[serde(skip)]
    pub image: Option<Thumb>,
    /// Durée d'affichage (ms) ; `None` = celle des alertes (quelques secondes).
    pub hold_ms: Option<u64>,
}

impl Alert {
    /// Alerte d'Horizon (`icon` : nom d'icône de `lib/icons.ts`).
    pub fn icon(icon: &'static str, severity: Severity, title: String, body: String) -> Alert {
        Alert::new(Source::Icon { icon }, severity, title, body)
    }

    /// Message venu d'une appli (logo de l'appli).
    pub fn app(name: &str, app_id: &str, severity: Severity, title: String, body: String) -> Alert {
        Alert::new(Source::App { name: name.to_owned(), app_id: app_id.to_owned() }, severity, title, body)
    }

    fn new(source: Source, severity: Severity, title: String, body: String) -> Alert {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Alert {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            source,
            title,
            body,
            severity,
            action: None,
            image: None,
            hold_ms: None,
        }
    }

    pub fn with_action(mut self, action: Action) -> Alert {
        self.action = Some(action);
        self
    }

    /// Miniature à la place du logo, et durée d'affichage plus longue (le temps d'agir).
    pub fn with_image(mut self, image: Thumb, hold_ms: u64) -> Alert {
        self.image = Some(image);
        self.hold_ms = Some(hold_ms);
        self
    }
}

/// L'îlot peut-il montrer ce genre d'alerte en ce moment (« Ne pas déranger ») ?
pub fn allowed(app: &AppHandle, kind: AlertKind) -> bool {
    crate::dnd::allows(app, kind)
}

/// Montre une alerte dans l'îlot, sauf si « Ne pas déranger » la masque ; vrai si elle part.
pub fn show(app: &AppHandle, kind: AlertKind, alert: Alert) -> bool {
    let shown = allowed(app, kind);
    if kind != AlertKind::Plugin {
        let payload = serde_json::json!({ "kind": crate::plugins::events::alert_name(kind), "title": alert.title, "shown": shown });
        crate::plugins::events::emit(app, "alert", payload);
    }
    if !shown {
        log::debug!("îlot : « {} » masquée (ne pas déranger)", alert.title);
        return false;
    }
    let _ = app.emit(EV_ALERT, alert);
    true
}

/// Batterie d'un appareil suivi par le widget `kind` (seuil de la section « Alertes »).
pub struct LowBattery {
    kind: WidgetKind,
    icon: &'static str,
    alert: LowAlert,
}

impl LowBattery {
    pub fn new(kind: WidgetKind, icon: &'static str) -> LowBattery {
        LowBattery { kind, icon, alert: LowAlert::default() }
    }

    /// Nouvelle mesure de l'appareil `name` ; annonce le passage sous le seuil.
    pub fn update(&mut self, app: &AppHandle, name: &str, level: u8, charging: bool) {
        let alerts = crate::state::settings().alerts;
        let threshold = app.state::<crate::state::AppState>().read(|l| alerts.low_threshold(l, self.kind));
        let Some(t) = threshold else { return };
        if self.alert.update(level, charging, t) {
            log::info!("{name} : batterie faible ({level} %)");
            show(app, AlertKind::Battery, low_battery(self.icon, name, level));
        }
    }
}

/// Alerte « batterie faible » d'un appareil.
fn low_battery(icon: &'static str, name: &str, level: u8) -> Alert {
    Alert::icon(icon, Severity::Alert, format!("{name} : batterie faible"), format!("{level} % restants"))
}

/// Délai avant d'annoncer les nouveautés : les barres ont fini de charger.
const NEWS_DELAY: std::time::Duration = std::time::Duration::from_secs(6);

/// Texte de l'annonce des widgets ajoutés.
fn news_body(names: &[&str]) -> String {
    format!("Ajouté : {} — clic pour les voir en mode édition", names.join(", "))
}

/// « Nouveautés » : widgets ajoutés par une migration au chargement, annoncés une fois dans
/// l'îlot (clic = mode édition de la barre).
pub fn announce_news(app: &AppHandle) -> anyhow::Result<()> {
    let added = app.state::<crate::state::AppState>().take_news();
    if added.is_empty() {
        return Ok(());
    }
    let app = app.clone();
    crate::util::start_thread("nouveautes", move || {
        std::thread::sleep(NEWS_DELAY);
        let names: Vec<&str> =
            added.iter().map(|k| crate::widgets::manifest(k.as_str()).map_or(k.as_str(), |m| m.label)).collect();
        let alert = Alert::icon("squaresFour", Severity::Info, "Nouveautés dans tes barres".into(), news_body(&names))
            .with_action(Action::Edit);
        show(&app, AlertKind::Feedback, alert);
    })
}

/// Alerte d'essai de chaque genre, pour voir l'îlot sans attendre l'événement réel
/// (`horizon.exe --alerte-essai <genre>` pendant qu'Horizon tourne). Elle passe
/// par le même filtre « Ne pas déranger » ; ses actions ne visent rien de réel.
pub fn essai(app: &AppHandle, what: &str) {
    let alert = match what {
        "notification" => Some((AlertKind::Notification, Alert::app("Discord", "", Severity::Info, "Léo".into(), "On lance une partie ce soir ?".into()))),
        "claude" => Some((
            AlertKind::Claude,
            Alert::app("Claude", "", Severity::Attention, "deskbar".into(), "Claude attend ta réponse".into())
                .with_action(Action::Session { id: "essai".into() }),
        )),
        "batterie" => Some((AlertKind::Battery, low_battery("headset", "Casque", 12))),
        "serveur" => Some((
            AlertKind::Server,
            Alert::icon("hardDrives", Severity::Alert, "website est hors ligne".into(), "Injoignable deux fois de suite".into()),
        )),
        "minuteur" => Some((AlertKind::Timer, Alert::icon("timer", Severity::Attention, "Minuteur terminé".into(), "25 min écoulées".into()))),
        "fenetre" => Some((
            AlertKind::Feedback,
            Alert::icon("browsers", Severity::Attention, "Fenêtre non déplacée".into(), "fenêtre introuvable".into()),
        )),
        "nouveautes" => Some((
            AlertKind::Feedback,
            Alert::icon("squaresFour", Severity::Info, "Nouveautés dans tes barres".into(), news_body(&["Météo"])).with_action(Action::Edit),
        )),
        "capture" => Some((
            AlertKind::Claude,
            Alert::icon("camera", Severity::Info, "Envoyer à deskbar".into(), "Capture d'écran essai.png".into())
                .with_action(Action::Capture { path: String::new(), session: String::new() })
                .with_image(essai_thumb(), 15_000),
        )),
        _ => None,
    };
    match alert {
        Some((kind, a)) => {
            log::info!("alerte d'essai : {what}");
            show(app, kind, a);
        }
        None => log::warn!("alerte d'essai inconnue : {what}"),
    }
}

/// Miniature d'essai : dégradé, coins de l'îlot.
fn essai_thumb() -> Thumb {
    let make = |w: u32, h: u32| {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[(x * 255 / w) as u8, (y * 255 / h) as u8, 200, 255]);
            }
        }
        Pixels { width: w, height: h, rgba: Arc::new(rgba) }
    };
    Thumb { island: make(80, 48), line: make(64, 40) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_shape() {
        let a = Alert::app("Claude", "", Severity::Attention, "deskbar".into(), "Demande l’autorisation".into())
            .with_action(Action::Session { id: "s1".into() });
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["source"], serde_json::json!({ "kind": "app", "name": "Claude", "app_id": "" }));
        assert_eq!(v["severity"], "attention");
        assert_eq!(v["action"], serde_json::json!({ "kind": "session", "id": "s1" }));
        let b = serde_json::to_value(Alert::icon("timer", Severity::Info, "t".into(), "b".into())).unwrap();
        assert_eq!(b["source"], serde_json::json!({ "kind": "icon", "icon": "timer" }));
        assert_eq!(b["action"], serde_json::Value::Null);
        assert_eq!(b["hold_ms"], serde_json::Value::Null);
        let px = Pixels { width: 1, height: 1, rgba: Arc::new(vec![0; 4]) };
        let c = Alert::icon("camera", Severity::Info, "t".into(), "b".into())
            .with_action(Action::Capture { path: "C:/c.png".into(), session: "s".into() })
            .with_image(Thumb { island: px.clone(), line: px }, 12_000);
        assert!(c.image.is_some());
        let c = serde_json::to_value(&c).unwrap();
        assert_eq!(c["action"], serde_json::json!({ "kind": "capture", "path": "C:/c.png", "session": "s" }));
        assert_eq!(c["hold_ms"], 12_000);
        assert_ne!(a.id, serde_json::from_value::<u64>(b["id"].clone()).unwrap(), "numéros uniques");
    }

    #[test]
    fn texts_of_the_alerts() {
        let a = low_battery("headset", "Casque", 12);
        assert_eq!((a.title.as_str(), a.body.as_str(), a.severity), ("Casque : batterie faible", "12 % restants", Severity::Alert));
        assert_eq!(news_body(&["Météo", "Agenda"]), "Ajouté : Météo, Agenda — clic pour les voir en mode édition");
    }
}
