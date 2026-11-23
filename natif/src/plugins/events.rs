//! Événements d'Horizon vers les scripts des plugins qui les ont déclarés
//! (`permissions.events`) : `accent`, `media`, `dnd`, `alert`, `timer`, `presence`,
//! `foreground`. La dernière valeur des événements d'état est gardée et rejouée à un script qui
//! se charge (pas celle des événements ponctuels `alert` et `timer`).

use crate::host::AppHandle;
use crate::util::lock;
use horizon_core::plugin::Permissions;
use serde_json::{Value as Json, json};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// Événements ponctuels (jamais rejoués).
const ONE_SHOT: [&str; 2] = ["alert", "timer"];

/// Dernière valeur de chaque événement d'état.
static LAST: Mutex<BTreeMap<String, Json>> = Mutex::new(BTreeMap::new());

/// Envoie l'événement `name` aux scripts qui l'ont déclaré (de n'importe quel fil).
pub fn emit(app: &AppHandle, name: &'static str, payload: Json) {
    if !ONE_SHOT.contains(&name) {
        lock(&LAST).insert(name.to_owned(), payload.clone());
    }
    let a = app.clone();
    let _ = slint::invoke_from_event_loop(move || super::script::event(&a, name, &payload));
}

/// Valeurs à rejouer à un script qui se charge avec ces permissions.
pub fn replay(perms: &Permissions) -> Vec<(String, Json)> {
    replay_from(&lock(&LAST), perms)
}

fn replay_from(last: &BTreeMap<String, Json>, perms: &Permissions) -> Vec<(String, Json)> {
    last.iter().filter(|(name, _)| perms.allows_event(name)).map(|(n, v)| (n.clone(), v.clone())).collect()
}

/// Couleur `#rrggbb`.
pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Application au premier plan (fil de suivi abonné au premier plan).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let rx = crate::foreground::subscribe(app);
    let a = app.clone();
    crate::util::start_thread("plugins-premier-plan", move || {
        for fg in rx {
            emit(&a, "foreground", json!({ "exe": fg.map(|f| f.app) }));
        }
    })
}

/// Nom d'un genre d'alerte pour les scripts.
pub fn alert_name(kind: horizon_core::dnd::AlertKind) -> &'static str {
    use horizon_core::dnd::AlertKind;
    match kind {
        AlertKind::Notification => "notification",
        AlertKind::Claude => "claude",
        AlertKind::Battery => "battery",
        AlertKind::Timer => "timer",
        AlertKind::Feedback => "feedback",
        AlertKind::Update => "update",
        AlertKind::Plugin => "plugin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_value_replayed_on_subscribe() {
        let mut last = BTreeMap::new();
        last.insert("dnd".to_owned(), json!({ "active": true }));
        last.insert("accent".to_owned(), json!({ "color": "#112233" }));
        let perms = Permissions { events: vec!["dnd".into()], ..Permissions::default() };
        assert_eq!(replay_from(&last, &perms), vec![("dnd".to_owned(), json!({ "active": true }))]);
        assert!(replay_from(&last, &Permissions::default()).is_empty());
    }

    #[test]
    fn hex_color() {
        assert_eq!(hex([1, 0xab, 255]), "#01abff");
    }
}
