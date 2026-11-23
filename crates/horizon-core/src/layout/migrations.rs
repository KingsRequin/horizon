//! Migrations : changements appliqués une fois aux dispositions enregistrées par une version
//! précédente. Horizon ne pose plus de widget sur les barres existantes (chacun ajoute ce qu'il
//! veut) : les migrations qui en ajoutaient (Suggestions, Casque, Météo, Serveurs, Fenêtres,
//! Essaim…) sont retirées ; leurs marques restent dans les fichiers, sans effet.

use super::{BarLayout, BarRole, Interaction, Layout, WidgetKind};
use serde_json::Value;

/// Marque de la migration qui renomme les clés de réglages en camelCase (`low_alert`).
pub(crate) const MIGRATION_CAMEL_CASE: &str = "camel-case";
/// Marque de la migration qui passe l'écran du haut d'« Information seule » à « Survol seul ».
pub(crate) const MIGRATION_TOP_HOVER: &str = "top-hover";
/// Marque de la migration qui remplace Éclairage et Batteries (intégrés d'avant) par les
/// plugins Prisme et SharkApp, à la même place.
pub(crate) const MIGRATION_PLUGINS: &str = "plugins-prisme-sharkapp";
/// Marque de la migration qui remplace Essaim, Serveurs et Quota Claude (intégrés d'avant) par
/// leurs plugins, à la même place.
pub(crate) const MIGRATION_PLUGINS_2: &str = "plugins-essaim-serveurs-quota";
type Migration = (&'static str, fn(&mut BarLayout));

/// Migrations, dans l'ordre : marque gardée dans `Layout::migrations` et changement appliqué à
/// chaque barre. Une disposition neuve les a toutes, déjà marquées, sans qu'elles lui soient
/// appliquées ; une disposition ancienne reçoit celles qu'elle n'a pas encore
/// (`Layout::migrate`). Une nouvelle migration s'ajoute ici seulement.
pub(super) const MIGRATIONS: [Migration; 4] = [
    (MIGRATION_CAMEL_CASE, BarLayout::camel_case_settings),
    (MIGRATION_TOP_HOVER, BarLayout::top_hover),
    (MIGRATION_PLUGINS, BarLayout::migrate_to_plugins),
    (MIGRATION_PLUGINS_2, BarLayout::migrate_to_plugins_2),
];

/// Widgets intégrés devenus des plugins de la bibliothèque (ancien genre, plugin). Leurs
/// réglages sont gardés tels quels : seul Serveurs en a (`hosts`, que son plugin relit).
const NOW_PLUGINS: [(WidgetKind, WidgetKind); 3] = [
    (WidgetKind::ESSAIM, WidgetKind::ESSAIM_PLUGIN),
    (WidgetKind::SERVERS, WidgetKind::SERVEURS_PLUGIN),
    (WidgetKind::CLAUDE_QUOTA, WidgetKind::CLAUDE_QUOTA_PLUGIN),
];

/// Clés de réglages renommées pour suivre la convention camelCase de tous les autres
/// réglages de widget (ancienne clé, nouvelle).
const RENAMED_KEYS: [(&str, &str); 1] = [("low_alert", "lowAlert")];

impl BarLayout {
    /// Écran du haut : « Survol seul » (panneaux au survol, clics sans effet) au lieu
    /// d'« Information seule », s'il n'a pas été changé.
    fn top_hover(&mut self) {
        if self.role == BarRole::Top && self.interaction == Interaction::Info {
            self.interaction = Interaction::Hover;
        }
    }

    /// Éclairage → plugin Prisme, Batteries → plugin SharkApp (même identifiant, même place ;
    /// le seuil d'alerte de Batteries devient le réglage `seuil` du plugin).
    fn migrate_to_plugins(&mut self) {
        for w in self.widgets_mut() {
            if w.kind == WidgetKind::LIGHTING {
                w.kind = WidgetKind::PRISME_PLUGIN;
                w.settings = Value::Null;
            } else if w.kind == WidgetKind::BATTERIES {
                let seuil = w.settings.get("lowAlert").cloned();
                w.kind = WidgetKind::SHARKAPP_PLUGIN;
                w.settings = seuil.map_or(Value::Null, |s| serde_json::json!({ "seuil": s }));
            }
        }
    }

    /// Essaim, Serveurs et Quota Claude → leurs plugins (même identifiant, même place, mêmes
    /// réglages).
    fn migrate_to_plugins_2(&mut self) {
        for w in self.widgets_mut() {
            if let Some((_, plugin)) = NOW_PLUGINS.iter().find(|(old, _)| *old == w.kind) {
                w.kind = *plugin;
            }
        }
    }

    /// Réglages : anciennes clés renommées (une valeur déjà sous le nouveau nom l'emporte).
    fn camel_case_settings(&mut self) {
        for w in self.widgets_mut() {
            let Some(obj) = w.settings.as_object_mut() else { continue };
            for (old, new) in RENAMED_KEYS {
                if let Some(v) = obj.remove(old) {
                    obj.entry(new).or_insert(v);
                }
            }
        }
    }
}

impl Layout {
    /// Met à niveau une disposition enregistrée par une version précédente. Un plugin qu'une
    /// migration met à la place d'un widget intégré est noté dans `pending_plugins` : il reste à
    /// l'installer.
    pub fn migrate(&mut self) {
        let plugins = |l: &Layout| -> Vec<String> {
            let mut ids: Vec<String> = l.all_widgets().filter_map(|w| crate::plugin::id_of_kind(w.kind.as_str())).map(str::to_owned).collect();
            ids.sort();
            ids.dedup();
            ids
        };
        let before = plugins(self);
        for (mark, change) in MIGRATIONS {
            if !self.migrations.iter().any(|m| m == mark) {
                self.bars.iter_mut().for_each(change);
                self.profiles.iter_mut().flat_map(|p| p.bars.iter_mut()).for_each(change);
                self.migrations.push(mark.into());
            }
        }
        for id in plugins(self) {
            if !before.contains(&id) && !self.pending_plugins.contains(&id) {
                self.pending_plugins.push(id);
            }
        }
        self.ensure_profiles();
    }
}
