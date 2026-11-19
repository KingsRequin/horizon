//! Migrations : widgets ajoutés une fois aux dispositions enregistrées par une version
//! précédente (jamais rajoutés si l'utilisateur les retire ensuite).

use super::{BarLayout, BarRole, Interaction, Layout, WidgetKind, Zone, widget};
use serde_json::Value;

/// Marque de la migration qui ajoute le widget Suggestions aux dispositions existantes.
pub(crate) const MIGRATION_SUGGESTIONS: &str = "suggestions";
/// Marque de la migration qui ajoute le widget Casque à la barre principale.
pub(crate) const MIGRATION_HEADSET: &str = "headset";
/// Marque de la migration qui ajoute Discord vocal, Agenda, Batteries, Éclairage et Météo.
pub(crate) const MIGRATION_WIDGETS_2: &str = "widgets-2";
/// Marque de la migration qui ajoute le widget Serveurs (homelab) à l'écran du haut.
pub(crate) const MIGRATION_SERVERS: &str = "servers";
/// Marque de la migration qui renomme les clés de réglages en camelCase (`low_alert`).
pub(crate) const MIGRATION_CAMEL_CASE: &str = "camel-case";
/// Marque de la migration qui ajoute Sessions Claude Code à l'écran principal et au portrait.
pub(crate) const MIGRATION_CLAUDE_SESSIONS: &str = "claude-sessions";
/// Marque de la migration qui passe l'écran du haut d'« Information seule » à « Survol seul ».
pub(crate) const MIGRATION_TOP_HOVER: &str = "top-hover";
/// Marque de la migration qui ajoute le widget Fenêtres à la barre principale.
pub(crate) const MIGRATION_WINDOWS: &str = "windows";
/// Marque de la migration qui ajoute le widget Essaim (curseur animé) à la barre principale.
pub(crate) const MIGRATION_ESSAIM: &str = "essaim";
/// Marque de la migration qui remplace Éclairage et Batteries (intégrés d'avant) par les
/// plugins Prisme et SharkApp, à la même place.
pub(crate) const MIGRATION_PLUGINS: &str = "plugins-prisme-sharkapp";
type Migration = (&'static str, fn(&mut BarLayout));

/// Migrations qui n'ajoutent aucun widget (rien à annoncer dans « Nouveautés »).
const SILENT: [&str; 3] = [MIGRATION_CAMEL_CASE, MIGRATION_TOP_HOVER, MIGRATION_PLUGINS];

/// Migrations, dans l'ordre : marque gardée dans `Layout::migrations` et ajout appliqué à
/// chaque barre. Une disposition neuve les reçoit toutes, déjà marquées ; une disposition
/// ancienne reçoit celles qu'elle n'a pas encore (`Layout::migrate`). Une nouvelle migration
/// s'ajoute ici seulement.
pub(super) const MIGRATIONS: [Migration; 10] = [
    (MIGRATION_SUGGESTIONS, BarLayout::add_suggestions),
    (MIGRATION_HEADSET, BarLayout::add_headset),
    (MIGRATION_WIDGETS_2, BarLayout::add_widgets_2),
    (MIGRATION_SERVERS, BarLayout::add_servers),
    (MIGRATION_CAMEL_CASE, BarLayout::camel_case_settings),
    (MIGRATION_CLAUDE_SESSIONS, BarLayout::add_claude_sessions),
    (MIGRATION_TOP_HOVER, BarLayout::top_hover),
    (MIGRATION_WINDOWS, BarLayout::add_windows),
    (MIGRATION_ESSAIM, BarLayout::add_essaim),
    (MIGRATION_PLUGINS, BarLayout::migrate_to_plugins),
];

/// Clés de réglages renommées pour suivre la convention camelCase de tous les autres
/// réglages de widget (ancienne clé, nouvelle).
const RENAMED_KEYS: [(&str, &str); 1] = [("low_alert", "lowAlert")];

impl BarLayout {
    /// « Claude attend » visible là où l'on travaille : Sessions Claude Code en tête de la zone
    /// droite de l'écran principal et du portrait (avant les notifications). Le widget ne
    /// prend de place que s'il y a une session qui attend ou travaille.
    fn add_claude_sessions(&mut self) {
        if matches!(self.role, BarRole::Main | BarRole::Portrait) {
            self.insert_before(Zone::Right, WidgetKind::CLAUDE_SESSIONS, &[WidgetKind::NOTIFICATIONS]);
        }
    }

    /// Widget Fenêtres (compteur, panneau des fenêtres par écran) : à la fin de la zone gauche
    /// de la barre principale, après le média, Discord vocal et l'agenda ; la zone droite y est
    /// pleine. Ailleurs, il s'ajoute par le menu d'ajout.
    fn add_windows(&mut self) {
        if self.role == BarRole::Main {
            self.insert_before(Zone::Left, WidgetKind::WINDOWS, &[]);
        }
    }

    /// Widget Essaim (curseur animé, appli à part) : sur la barre principale, juste après
    /// l'Éclairage (Prisme), sinon avant le système et l'horloge.
    fn add_essaim(&mut self) {
        if self.role != BarRole::Main || self.widgets().any(|w| w.kind == WidgetKind::ESSAIM) {
            return;
        }
        match self.right.iter().position(|w| w.kind == WidgetKind::LIGHTING || w.kind == WidgetKind::PRISME_PLUGIN) {
            Some(i) => self.right.insert(i + 1, widget(WidgetKind::ESSAIM, Value::Null)),
            None => self.insert_before(Zone::Right, WidgetKind::ESSAIM, &[WidgetKind::SYSTEM, WidgetKind::CLOCK]),
        }
    }

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

    /// Insère `kind` dans `zone` juste avant le premier widget de `before` présent (sinon à
    /// la fin), s'il n'est pas déjà sur la barre.
    fn insert_before(&mut self, zone: Zone, kind: WidgetKind, before: &[WidgetKind]) {
        if self.widgets().any(|w| w.kind == kind) {
            return;
        }
        let v = self.zone_mut(zone);
        let i = before.iter().find_map(|b| v.iter().position(|w| w.kind == *b)).unwrap_or(v.len());
        v.insert(i, widget(kind, Value::Null));
    }

    /// Widgets de novembre 2026 : sur la barre principale, Discord vocal et Agenda à la fin de
    /// la zone gauche (après le média), Batteries après le casque, Éclairage avant le système,
    /// Météo avant l'horloge ; sur l'écran du haut (information seule), Météo et Agenda après
    /// l'horloge.
    fn add_widgets_2(&mut self) {
        match self.role {
            BarRole::Main => {
                self.insert_before(Zone::Left, WidgetKind::DISCORD_VOICE, &[]);
                self.insert_before(Zone::Left, WidgetKind::CALENDAR, &[]);
                // Batteries et Éclairage sont devenus les plugins SharkApp et Prisme.
                self.insert_before(Zone::Right, WidgetKind::SHARKAPP_PLUGIN, &[WidgetKind::MIC_KRISP, WidgetKind::SYSTEM, WidgetKind::CLOCK]);
                self.insert_before(Zone::Right, WidgetKind::PRISME_PLUGIN, &[WidgetKind::SYSTEM, WidgetKind::CLOCK]);
                self.insert_before(Zone::Right, WidgetKind::WEATHER, &[WidgetKind::CLOCK]);
            }
            BarRole::Top => {
                self.insert_before(Zone::Left, WidgetKind::WEATHER, &[]);
                self.insert_before(Zone::Left, WidgetKind::CALENDAR, &[]);
            }
            BarRole::Portrait | BarRole::Side => {}
        }
    }

    /// Widget Serveurs : à la fin de la zone gauche de l'écran du haut (après l'agenda). Ses
    /// pastilles sont de l'information, comme le reste de cette barre ; la zone droite y est
    /// déjà pleine et celle de la barre principale aussi. Le terminal SSH reste à portée par la
    /// palette de commandes, et le mode édition peut le déplacer sur une barre cliquable (panneau).
    fn add_servers(&mut self) {
        if self.role == BarRole::Top {
            self.insert_before(Zone::Left, WidgetKind::SERVERS, &[]);
        }
    }

    /// Ajoute le widget Casque à la barre principale qui n'en a pas : juste avant le voyant
    /// micro de la zone droite, sinon en tête de cette zone.
    fn add_headset(&mut self) {
        if self.role != BarRole::Main || self.widgets().any(|w| w.kind == WidgetKind::HEADSET) {
            return;
        }
        let i = self.right.iter().position(|w| w.kind == WidgetKind::MIC_KRISP).unwrap_or(0);
        self.right.insert(i, widget(WidgetKind::HEADSET, Value::Null));
    }

    /// Ajoute le widget Suggestions à une barre cliquable qui n'en a pas : juste après le
    /// premier widget Raccourcis, sinon à la fin de la zone gauche.
    fn add_suggestions(&mut self) {
        if !self.clickable() || self.widgets().any(|w| w.kind == WidgetKind::SUGGESTIONS) {
            return;
        }
        let w = widget(WidgetKind::SUGGESTIONS, Value::Null);
        // Bande du haut seulement : Suggestions n'a pas de panneau, il ne peut pas être une tuile.
        for zone in [Zone::Left, Zone::Center, Zone::Right] {
            let v = self.zone_mut(zone);
            if let Some(i) = v.iter().position(|w| w.kind == WidgetKind::SHORTCUTS) {
                v.insert(i + 1, w);
                return;
            }
        }
        self.left.push(w);
    }
}

impl Layout {
    /// Met à niveau une disposition enregistrée par une version précédente. Renvoie les genres
    /// de widgets ajoutés aux barres du profil actif (à annoncer : « Nouveautés »).
    pub fn migrate(&mut self) -> Vec<WidgetKind> {
        let mut added = Vec::new();
        for (mark, add) in MIGRATIONS {
            if !self.migrations.iter().any(|m| m == mark) {
                let before: Vec<String> = self.widgets().map(|w| w.id.clone()).collect();
                self.bars.iter_mut().for_each(add);
                self.profiles.iter_mut().flat_map(|p| p.bars.iter_mut()).for_each(add);
                self.migrations.push(mark.into());
                if !SILENT.contains(&mark) {
                    for w in self.widgets().filter(|w| !before.contains(&w.id)) {
                        if !added.contains(&w.kind) {
                            added.push(w.kind);
                        }
                    }
                }
            }
        }
        self.ensure_profiles();
        added
    }
}
