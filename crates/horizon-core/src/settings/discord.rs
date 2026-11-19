//! Section `[discord]` : application Discord utilisée par le widget Discord vocal (RPC local).
//! Vide : StreamKit, l'application de Discord pour les overlays (rien à configurer).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordSettings {
    /// Identifiant de ta propre application Discord (portail des développeurs) ; vide = StreamKit.
    pub client_id: String,
    /// Son secret (onglet OAuth2), nécessaire avec ta propre application.
    pub client_secret: String,
}

impl DiscordSettings {
    /// Ta propre application est réglée.
    pub fn own_app(&self) -> Option<(&str, &str)> {
        (!self.client_id.trim().is_empty() && !self.client_secret.trim().is_empty()).then(|| (self.client_id.trim(), self.client_secret.trim()))
    }
}
