//! Plugins (logique pure) : manifeste `plugin.json`, permissions du bac à sable, réglages
//! déclarés, budget de performance. L'hôte (chargement, rendu, scripts) est dans
//! `natif/src/plugins/`. Conception : `.superpowers/docs/specs/2026-11-19-plugins-design.md`.

pub mod budget;
pub mod library;
mod manifest;
mod permissions;
mod settings;

pub use manifest::{ManifestError, PluginManifest, WidgetSpec, parse_manifest};
pub use permissions::Permissions;
pub use settings::{SettingKind, SettingSpec, coerce, defaults};

/// Versions de l'API des plugins acceptées. 0 = préversion (jusqu'à l'ouverture de la
/// bibliothèque, où elle est figée en 1).
pub const API_MIN: u32 = 0;
pub const API_MAX: u32 = 0;

/// Préfixe du genre d'un widget de plugin dans `layout.json` (`plugin:<id>`).
pub const KIND_PREFIX: &str = "plugin:";

/// Événements d'Horizon qu'un plugin peut recevoir (`permissions.events`).
pub const EVENTS: [&str; 7] = ["accent", "media", "dnd", "alert", "timer", "presence", "foreground"];

/// Genre de layout du plugin `id`.
pub fn kind_of(id: &str) -> String {
    format!("{KIND_PREFIX}{id}")
}

/// Identifiant du plugin d'un genre de layout (`None` : widget intégré).
pub fn id_of_kind(kind: &str) -> Option<&str> {
    kind.strip_prefix(KIND_PREFIX).filter(|id| !id.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_roundtrip() {
        assert_eq!(kind_of("prisme"), "plugin:prisme");
        assert_eq!(id_of_kind("plugin:prisme"), Some("prisme"));
        assert_eq!(id_of_kind("clock"), None);
        assert_eq!(id_of_kind("plugin:"), None);
    }
}
