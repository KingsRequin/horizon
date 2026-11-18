//! Fenêtre de réglages, unique : « Réglages d'Horizon », réglages d'un widget ou profils. Elle s'ouvre sur l'écran de la barre du widget (sinon sur celui de la
//! souris), avec un titre qui dit ce qu'elle règle (« Météo — Écran principal »). Les barres
//! savent si elle est ouverte (`horizon://settings-window`) : le mode édition ne se termine pas
//! quand elle prend le focus.
//!
//! Repris de `app/src-tauri/src/settings_window.rs` (règles de placement, titres, pages) ; la
//! fenêtre elle-même est menée par `ui::settings_window` sur le fil de l'interface.

use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use horizon_core::geometry::{MonitorInfo, screen_names};
use horizon_core::layout::{BarRole, WidgetKind};

/// Étiquette de la fenêtre de réglages (une seule à la fois).
pub const LABEL: &str = "settings";

/// Page de la fenêtre de réglages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsPage {
    /// Réglages d'un widget (identifiant).
    Widget(String),
    Profiles,
    /// « Réglages d'Horizon », sur une section (`barres`, `alertes`, `etat`…).
    Options(String),
}

impl SettingsPage {
    /// Page demandée par l'interface : `widget:<id>`, `profiles` ou `options:<section>`.
    pub fn parse(page: &str) -> Option<SettingsPage> {
        Some(match page.split_once(':') {
            Some(("widget", id)) if !id.is_empty() => SettingsPage::Widget(id.to_owned()),
            Some(("options", s)) if s.chars().all(|c| c.is_ascii_lowercase()) => SettingsPage::Options(s.to_owned()),
            None if page == "profiles" => SettingsPage::Profiles,
            _ => return None,
        })
    }

    /// Écran de la barre qui porte le widget.
    pub fn monitor(&self, app: &AppHandle) -> Option<String> {
        let SettingsPage::Widget(id) = self else { return None };
        app.state::<AppState>().read(|l| {
            l.bars.iter().find(|b| b.widgets().any(|w| &w.id == id)).map(|b| b.monitor_id.clone())
        })
    }

    pub fn title(&self, app: &AppHandle) -> String {
        match self {
            SettingsPage::Widget(id) => {
                let kind = app.state::<AppState>().read(|l| l.widget(id).map(|w| w.kind));
                match (kind, self.monitor(app)) {
                    (Some(k), Some(m)) => format!("{} — {}", widget_label(k), screen_name(app, &m)),
                    (Some(k), None) => widget_label(k).to_owned(),
                    _ => "Réglages du widget".into(),
                }
            }
            SettingsPage::Profiles => "Profils — Horizon".into(),
            SettingsPage::Options(_) => "Réglages d’Horizon".into(),
        }
    }

    /// Taille logique à la création.
    pub fn size(&self) -> (f32, f32) {
        match self {
            SettingsPage::Options(_) => (760.0, 640.0),
            _ => (560.0, 620.0),
        }
    }
}

/// Nom affiché d'un genre de widget (manifeste), sinon son identifiant.
fn widget_label(kind: WidgetKind) -> &'static str {
    crate::widgets::manifest(kind.as_str()).map_or(kind.as_str(), |m| m.label)
}

/// Nom de rôle d'une barre dont l'écran n'est pas branché.
fn role_label(role: BarRole) -> &'static str {
    match role {
        BarRole::Main => "Écran principal",
        BarRole::Top => "Écran du haut",
        BarRole::Portrait => "Écran vertical",
        BarRole::Side => "Écran latéral",
    }
}

/// Nom d'un écran pour l'utilisateur (« Écran de droite ») ; écran débranché : d'après le rôle
/// de sa barre.
pub fn screen_name(app: &AppHandle, monitor_id: &str) -> String {
    let monitors = horizon_sys::monitors::list();
    if let Some((_, name)) = screen_names(&monitors).into_iter().find(|(id, _)| id == monitor_id) {
        return name;
    }
    let role = app.state::<AppState>().bar(monitor_id).map(|b| b.role);
    role.map_or("Écran débranché", role_label).to_owned()
}

/// Ouvre (ou ramène) la fenêtre de réglages sur une page (de n'importe quel fil).
pub fn open(app: &AppHandle, page: SettingsPage) {
    // Nouvelle session de réglages : ses modifications feront un nouvel état d'annulation.
    app.state::<AppState>().end_undo_group();
    let a = app.clone();
    if let Err(e) = app.run_on_main_thread(move || crate::ui::settings_window::show(&a, page)) {
        log::error!("ouverture des réglages : {e:#}");
    }
}

/// Ouvre une page des réglages d'après son nom (`widget:<id>`, `profiles`, `options:<section>`).
pub fn settings_open(app: &AppHandle, page: &str) -> Result<(), String> {
    let page = SettingsPage::parse(page).ok_or_else(|| format!("page inconnue : {page}"))?;
    open(app, page);
    Ok(())
}

/// Écran où poser la fenêtre : celui de la barre du widget, sinon celui de la souris.
pub fn target_monitor(app: &AppHandle, page: &SettingsPage) -> Option<MonitorInfo> {
    let monitors = horizon_sys::monitors::list();
    if let Some(id) = page.monitor(app)
        && let Some(m) = monitors.iter().find(|m| m.id == id)
    {
        return Some(m.clone());
    }
    mouse_monitor()
}

/// Écran de la souris, sinon l'écran principal.
pub fn mouse_monitor() -> Option<MonitorInfo> {
    let monitors = horizon_sys::monitors::list();
    let (x, y) = horizon_sys::fullscreen::cursor().unwrap_or((0, 0));
    let inside = |m: &&MonitorInfo| (m.rect.x..m.rect.x + m.rect.w).contains(&x) && (m.rect.y..m.rect.y + m.rect.h).contains(&y);
    monitors.iter().find(inside).or_else(|| monitors.iter().find(|m| m.primary)).cloned()
}

/// Zone cliente (pixels physiques) d'une fenêtre de taille logique `size`, centrée dans la zone
/// de travail de `m` et bornée à elle.
pub fn centered_on(m: &MonitorInfo, size: (f32, f32)) -> (i32, i32, u32, u32) {
    let scale = if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 };
    let pw = ((size.0 * scale).round() as i32).min(m.work.w.max(1));
    let ph = ((size.1 * scale).round() as i32).min(m.work.h.max(1));
    (m.work.x + (m.work.w - pw) / 2, m.work.y + (m.work.h - ph) / 2, pw as u32, ph as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::geometry::Rect;

    #[test]
    fn settings_pages() {
        assert_eq!(SettingsPage::parse("widget:w1"), Some(SettingsPage::Widget("w1".into())));
        assert_eq!(SettingsPage::parse("profiles"), Some(SettingsPage::Profiles));
        assert_eq!(SettingsPage::parse("options:alertes"), Some(SettingsPage::Options("alertes".into())));
        assert_eq!(SettingsPage::parse("options:../x"), None);
        assert_eq!(SettingsPage::parse("autre"), None);
        assert_eq!(SettingsPage::parse("widget:"), None);
        assert_eq!(SettingsPage::Options("etat".into()).size(), (760.0, 640.0));
        assert_eq!(SettingsPage::Profiles.size(), (560.0, 620.0));
    }

    #[test]
    fn window_is_centered_in_the_work_area_at_its_scale() {
        let m = MonitorInfo {
            id: "4k".into(),
            device_name: "4k".into(),
            rect: Rect { x: -1936, y: -2160, w: 3840, h: 2160 },
            work: Rect { x: -1936, y: -2160, w: 3840, h: 2100 },
            scale: 1.5,
            primary: false,
            portrait: false,
        };
        assert_eq!(centered_on(&m, (760.0, 640.0)), (-1936 + (3840 - 1140) / 2, -2160 + (2100 - 960) / 2, 1140, 960));
        let small = MonitorInfo { work: Rect { x: 0, y: 0, w: 800, h: 600 }, scale: 1.0, ..m };
        assert_eq!(centered_on(&small, (760.0, 640.0)), (20, 0, 760, 600), "bornée à la zone de travail");
    }

    /// Les noms affichés viennent des manifestes (`widget.json`, lus par build.rs).
    #[test]
    fn widget_labels_come_from_the_manifests() {
        assert_eq!(widget_label(WidgetKind::MEDIA), "Lecture en cours");
        assert_eq!(widget_label(WidgetKind::new("inconnu")), "inconnu");
    }
}
