//! Disposition de départ : rôle de chaque écran et widgets qu'il reçoit (conception §3.5).

use super::migrations::MIGRATIONS;
use super::profiles::{DEFAULT_PROFILE, Profile};
use super::{BarLayout, BarRole, BarStyle, Interaction, LAYOUT_VERSION, Layout, TilesMode, WidgetKind, widget};
use crate::geometry::MonitorInfo;
use serde_json::{Value, json};

/// Rôle d'un écran : principal, au-dessus du principal, portrait, ou autre.
pub(crate) fn role_of(m: &MonitorInfo, all: &[MonitorInfo]) -> BarRole {
    if m.primary {
        return BarRole::Main;
    }
    if m.portrait {
        return BarRole::Portrait;
    }
    match all.iter().find(|p| p.primary) {
        Some(p) if m.rect.y + m.rect.h <= p.rect.y => BarRole::Top,
        _ => BarRole::Side,
    }
}

impl BarLayout {
    /// Disposition de départ d'un écran (conception §3.5).
    pub fn default_for(m: &MonitorInfo, role: BarRole) -> BarLayout {
        let w = |k| widget(k, Value::Null);
        let (style, interaction, left, center, right) = match role {
            BarRole::Main => (
                BarStyle::Islands,
                Interaction::Click,
                vec![w(WidgetKind::MEDIA)],
                vec![w(WidgetKind::SHORTCUTS)],
                vec![w(WidgetKind::NOTIFICATIONS), w(WidgetKind::MIC_KRISP), w(WidgetKind::SYSTEM), w(WidgetKind::CLOCK)],
            ),
            BarRole::Top => (
                BarStyle::Graphic,
                // Le défaut d'avant (« Information seule ») est remplacé par la migration
                // `top-hover` ; la valeur de départ reste celle d'avant, comme pour les widgets.
                Interaction::Info,
                vec![widget(WidgetKind::CLOCK, json!({ "showDate": true }))],
                vec![w(WidgetKind::MEDIA)],
                vec![w(WidgetKind::NOTIFICATIONS), w(WidgetKind::CLAUDE_SESSIONS), w(WidgetKind::CLAUDE_QUOTA), w(WidgetKind::SYSTEM)],
            ),
            BarRole::Portrait => (
                BarStyle::Graphic,
                Interaction::Click,
                vec![w(WidgetKind::SHORTCUTS)],
                vec![],
                vec![w(WidgetKind::NOTIFICATIONS), w(WidgetKind::MIC_KRISP), w(WidgetKind::SYSTEM), w(WidgetKind::CLOCK)],
            ),
            BarRole::Side => (
                BarStyle::Graphic,
                Interaction::Click,
                vec![w(WidgetKind::SHORTCUTS)],
                vec![w(WidgetKind::MEDIA)],
                vec![w(WidgetKind::SYSTEM), w(WidgetKind::CLOCK)],
            ),
        };
        let mut bar = BarLayout {
            monitor_id: m.id.clone(),
            monitor_pos: (m.rect.x, m.rect.y),
            role,
            style,
            interaction,
            autohide: true,
            height: super::BAND_HEIGHT,
            // Nouvelle barre : tuiles dans un volet (rien de réservé sous la bande).
            tiles_mode: TilesMode::Click,
            left,
            center,
            right,
            tiles: vec![],
        };
        for (_, add) in MIGRATIONS {
            add(&mut bar);
        }
        bar
    }
}

impl Layout {
    pub fn default_for(monitors: &[MonitorInfo]) -> Layout {
        Layout {
            version: LAYOUT_VERSION,
            bars: monitors
                .iter()
                .map(|m| BarLayout::default_for(m, role_of(m, monitors)))
                .collect(),
            migrations: MIGRATIONS.iter().map(|(mark, _)| (*mark).to_owned()).collect(),
            profiles: vec![Profile::default_profile()],
            active_profile: DEFAULT_PROFILE.into(),
            auto_from: None,
        }
    }
}
