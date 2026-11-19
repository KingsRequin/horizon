//! Page « réglages d'un widget » de la fenêtre de réglages (`ui/widget-settings/`, global
//! `WidgetSettingsData` et un global par page), sur le fil de l'interface. Repris de
//! `app/src/settings/SettingsApp.svelte` : le widget est lu dans la disposition (`widget_get`),
//! chaque modification fusionne ses champs dans les réglages de l'exemplaire (`save`), affichée
//! tout de suite et enregistrée 300 ms après la dernière (`edit::edit_set_widget_settings` :
//! annulable par Ctrl+Z, une seule étape par ouverture) ; ce qui
//! attend part au changement de page et à la fermeture. Les barres suivent aussitôt
//! (`EV_LAYOUT`, liaisons des widgets refaites).
//!
//! Une page par genre : `calendar`, `servers`, `shortcuts`, `suggestions`, `weather` ; Micro et
//! Krisp (une case) ici ; Batteries, Casque, Sessions Claude et Notifications renvoient aux
//! « Réglages d'Horizon » (`GlobalLink`).

mod calendar;
pub mod plugin;
mod servers;
mod shortcuts;
mod suggestions;
mod weather;

use super::{SettingsWindow, WidgetSettingsData};
use crate::host::{AppHandle, Manager};
use crate::settings_window::SettingsPage;
use crate::state::AppState;
use serde_json::{Map, Value};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Enregistrement différé : une suite de frappes part en une fois.
const SAVE_DELAY: Duration = Duration::from_millis(300);

#[derive(Default)]
struct State {
    app: Option<AppHandle>,
    window: Option<slint::Weak<SettingsWindow>>,
    /// Exemplaire affiché et son genre.
    id: String,
    kind: String,
    /// Réglages de l'exemplaire, modifications comprises.
    settings: Value,
    /// Des modifications attendent d'être enregistrées.
    pending: bool,
    save: Rc<slint::Timer>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

/// La fenêtre de réglages (sans passer par `ui::settings_window`, que les rappels d'annulation
/// ne doivent pas toucher).
fn window() -> Option<SettingsWindow> {
    with(|s| s.window.as_ref().and_then(slint::Weak::upgrade))
}

fn app() -> Option<AppHandle> {
    with(|s| s.app.clone())
}

/// Réglages de l'exemplaire affiché (`Null` tant que rien n'est réglé).
fn settings() -> Value {
    with(|s| s.settings.clone())
}

/// Fusionne `patch` (un objet) dans des réglages ; des réglages qui ne sont pas un objet sont
/// remplacés.
fn merged(settings: &Value, patch: Value) -> Value {
    let mut out = match settings {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    if let Value::Object(p) = patch {
        out.extend(p);
    }
    Value::Object(out)
}

/// Fusionne `patch` dans les réglages de l'exemplaire et les enregistre (différé).
fn save(patch: Value) {
    let timer = with(|s| {
        if s.id.is_empty() {
            return None;
        }
        s.settings = merged(&s.settings, patch);
        s.pending = true;
        Some(s.save.clone())
    });
    if let Some(t) = timer {
        t.start(slint::TimerMode::SingleShot, SAVE_DELAY, flush);
    }
}

/// Enregistre ce qui attend.
fn flush() {
    let Some((app, id, settings)) = with(|s| {
        s.save.stop();
        let pending = std::mem::take(&mut s.pending);
        pending.then(|| s.app.clone().map(|a| (a, s.id.clone(), s.settings.clone()))).flatten()
    }) else {
        return;
    };
    if let Err(e) = crate::edit::edit_set_widget_settings(&app, &id, settings) {
        log::warn!("réglages de {id} : {e}");
        set_error(&e);
    }
}

/// Message d'erreur sous la page (vide : aucun).
fn set_error(e: &str) {
    if let Some(w) = window() {
        w.global::<WidgetSettingsData>().set_error(e.into());
    }
}

/// Fenêtre Windows de la page (boîtes de dialogue, menus).
fn hwnd() -> Option<isize> {
    window().and_then(|w| crate::platform::hwnd(w.window()))
}

/// Ouvre le menu d'une liste déroulante (`SelectField`) sous le champ : `x`, `y` en pixels
/// logiques de la fenêtre.
fn menu_at(x: f32, y: f32, entries: Vec<crate::ui::menu::Entry>, chosen: impl FnOnce(String) + 'static) {
    let (Some(w), Some(hwnd)) = (window(), hwnd()) else { return };
    let scale = w.window().scale_factor();
    let mut p = windows::Win32::Foundation::POINT { x: (x * scale).round() as i32, y: (y * scale).round() as i32 };
    // SAFETY : fenêtre de ce fil, point local.
    let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(windows::Win32::Foundation::HWND(hwnd as *mut _), &mut p) };
    crate::ui::menu::show_at(hwnd, entries, true, Some((p.x, p.y)), chosen);
}

/// Relie une fenêtre de réglages qui vient d'être créée.
pub fn bind(app: &AppHandle, window: &SettingsWindow) {
    with(|s| {
        s.app = Some(app.clone());
        s.window = Some(window.as_weak());
    });
    let g = window.global::<WidgetSettingsData>();
    g.on_set_discord_mute(|v| {
        save(serde_json::json!({ "discordMute": v }));
        if let Some(w) = self::window() {
            w.global::<WidgetSettingsData>().set_discord_mute(v);
        }
    });
    let a = app.clone();
    g.on_open_options(move |section| crate::settings_window::open(&a, SettingsPage::Options(section.to_string())));
    calendar::bind(window);
    plugin::bind(window);
    servers::bind(window);
    shortcuts::bind(window);
    suggestions::bind(window);
    weather::bind(window);
}

/// La page s'affiche sur l'exemplaire `id` (ouverture, réouverture ou autre widget).
pub fn show(app: &AppHandle, window: &SettingsWindow, id: &str) {
    leave();
    let widget = app.state::<AppState>().read(|l| l.widget(id).cloned());
    let (kind, settings) = widget.map_or((String::new(), Value::Null), |w| (w.kind.as_str().to_owned(), w.settings));
    with(|s| {
        s.id = if kind.is_empty() { String::new() } else { id.to_owned() };
        s.kind = kind.clone();
        s.settings = settings.clone();
        s.pending = false;
    });
    let g = window.global::<WidgetSettingsData>();
    let label = crate::widgets::manifest(&kind).map_or(kind.as_str(), |m| m.label);
    g.set_label(label.into());
    g.set_error("".into());
    g.set_discord_mute(settings.get("discordMute") == Some(&Value::Bool(true)));
    match kind.as_str() {
        "calendar" => calendar::show(window, &settings),
        "servers" => servers::show(app, window, &settings),
        "shortcuts" => shortcuts::show(window, &settings),
        "suggestions" => suggestions::show(app, window),
        "weather" => weather::show(window, &settings),
        k if horizon_core::plugin::id_of_kind(k).is_some() => plugin::show(window, k, &settings),
        _ => {}
    }
    // Genre posé en dernier : la page du genre se construit avec ses données.
    g.set_kind(kind.into());
    window.invoke_scroll_top();
}

/// La page n'est plus affichée (autre page, autre widget) : ce qui attend est enregistré, plus
/// rien ne tourne.
pub fn leave() {
    flush();
    calendar::leave();
    servers::leave();
    shortcuts::leave();
    weather::leave();
    with(|s| {
        s.id.clear();
        s.kind.clear();
        s.settings = Value::Null;
    });
}

/// La fenêtre se ferme.
pub fn closing(_app: &AppHandle) {
    leave();
    with(|s| s.window = None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patches_are_merged_into_the_settings() {
        assert_eq!(merged(&Value::Null, json!({ "a": 1 })), json!({ "a": 1 }));
        assert_eq!(merged(&json!({ "a": 1, "b": 2 }), json!({ "b": 3 })), json!({ "a": 1, "b": 3 }));
        assert_eq!(merged(&json!([1]), json!({ "c": true })), json!({ "c": true }));
    }
}
