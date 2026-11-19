//! Page « Réglages d'Horizon » de la fenêtre de réglages (`ui/options/`, global `OptionsData`),
//! sur le fil de l'interface : état lu par `options::options_get`, modifications appliquées tout
//! de suite à l'écran et enregistrées 400 ms après la dernière (`options::options_set`), options des barres aussitôt (`options_set_bar`), raccourci de la
//! palette saisi au clavier, page État relue toutes les 15 s tant qu'elle est affichée.
//! Comme OptionsApp.svelte : un changement venu d'ailleurs (`EV_OPTIONS`) relit l'état, sans
//! écraser une modification qui attend d'être enregistrée.

use super::{OptionsBar, OptionsData, SegmentChoice, SettingsWindow, StatusRow};
use crate::events::EV_OPTIONS;
use crate::host::{self, AppHandle};
use crate::options::{self, Capture, Edit, OptionsView};
use crate::settings_window::{LABEL, SettingsPage};
use crate::ui::menu::{self, Entry};
use horizon_core::layout::{BAND_HEIGHT, BarOptionsPatch, BarStyle, Interaction, MAX_BAR_HEIGHT};
use horizon_core::settings::Settings;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

/// Sections, dans l'ordre de la navigation.
pub const SECTIONS: [&str; 8] = ["barres", "alertes", "fonctions", "nuit", "minuteur", "palette", "general", "etat"];
/// Enregistrement différé : une suite de frappes part en une fois.
const SAVE_DELAY: Duration = Duration::from_millis(400);
/// Relecture de la page État.
const STATUS_PERIOD: Duration = Duration::from_secs(15);

#[derive(Default)]
struct State {
    app: Option<AppHandle>,
    window: Option<slint::Weak<SettingsWindow>>,
    view: Option<OptionsView>,
    /// Réglages modifiés qui attendent d'être enregistrés.
    pending: Option<Settings>,
    save: Rc<slint::Timer>,
    status: Rc<slint::Timer>,
    /// Barres dont la hauteur personnalisée est en cours de saisie.
    custom: HashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with_borrow_mut(f)
}

fn window() -> Option<SettingsWindow> {
    with(|s| s.window.as_ref().and_then(slint::Weak::upgrade))
}

fn app() -> Option<AppHandle> {
    with(|s| s.app.clone())
}

/// Relie une fenêtre de réglages qui vient d'être créée.
pub fn bind(app: &AppHandle, window: &SettingsWindow) {
    with(|s| {
        s.app = Some(app.clone());
        s.window = Some(window.as_weak());
    });
    let g = window.global::<OptionsData>();
    g.set_personal(crate::PERSONAL);
    g.set_styles(choices(&[("islands", "Îlots", ""), ("graphic", "Graphique", "")]));
    g.set_modes(choices(&[
        ("click", "Cliquable", "Panneaux au survol, actions au clic, molette = volume"),
        ("hover", "Survol seul", "Panneaux au survol ; les clics ne font rien (sauf sur Fenêtres)"),
        ("info", "Information seule", "Rien ne réagit à la souris (sauf le clic droit)"),
    ]));
    let mut heights = vec![(BAND_HEIGHT.to_string(), "Normale".to_owned(), format!("{BAND_HEIGHT} px : la bande seule"))];
    heights.extend(options::HEIGHT_CHOICES.iter().map(|h| (h.to_string(), h.to_string(), String::new())));
    heights.push(("custom".into(), "Autre…".into(), String::new()));
    g.set_heights(ModelRc::new(VecModel::from(
        heights.into_iter().map(|(value, label, hint)| SegmentChoice { value: value.into(), label: label.into(), hint: hint.into() }).collect::<Vec<_>>(),
    )));
    g.on_go(go);
    g.on_toggle(|id, v| {
        if id == "autostart" {
            set_autostart(v);
        } else {
            edit(&id, Edit::Bool(v));
        }
    });
    g.on_number(|id, v| edit(&id, Edit::Int(v as i64)));
    g.on_text(|id, v| edit(&id, Edit::Text(v.to_string())));
    g.on_pick(|id, x, y| pick(&id, x, y));
    g.on_bar(|monitor, key, value| set_bar(&monitor, &key, &value));
    g.on_preset_add(|typed| change(|s| options::add_preset(s, &typed)));
    g.on_preset_remove(|m| change(|s| options::remove_preset(s, m.max(0) as u32)));
    g.on_hotkey_key(hotkey_key);
    g.on_hotkey_set(|text| save_hotkey(&text));
    let a = app.clone();
    g.on_open_profiles(move || crate::settings_window::open(&a, SettingsPage::Profiles));
    g.on_status_reload(load_status);
    g.on_status_action(|id| status_action(&id));
    g.on_open_log(crate::status::open_log);
    host::listen(EV_OPTIONS, Some(LABEL), |_| reload());
}

fn choices(list: &[(&str, &str, &str)]) -> ModelRc<SegmentChoice> {
    ModelRc::new(VecModel::from(
        list.iter().map(|(v, l, h)| SegmentChoice { value: (*v).into(), label: (*l).into(), hint: (*h).into() }).collect::<Vec<_>>(),
    ))
}

/// La page s'affiche (ouverture ou réouverture) sur `section` (inconnue : « barres »).
pub fn show(_app: &AppHandle, window: &SettingsWindow, section: &str) {
    let section = if SECTIONS.contains(&section) { section } else { "barres" };
    flush();
    window.global::<OptionsData>().set_section(section.into());
    window.invoke_scroll_top();
    reload();
    follow_status(section == "etat");
}

/// Changement de section (navigation, « Changer » du raccourci sur la page État).
fn go(section: SharedString) {
    let Some(w) = window() else { return };
    flush();
    w.global::<OptionsData>().set_section(section.clone());
    w.invoke_scroll_top();
    follow_status(section == "etat");
}

/// La page n'est plus affichée (autre page de la fenêtre) : ce qui attend est enregistré, la
/// page État n'est plus relue.
pub fn leave() {
    flush();
    follow_status(false);
}

/// La fenêtre se ferme : ce qui attend est enregistré, plus rien ne tourne.
pub fn closing(_app: &AppHandle) {
    flush();
    with(|s| {
        s.status.stop();
        s.save.stop();
        s.window = None;
        s.view = None;
        s.custom.clear();
    });
}

/// Relit l'état ; une modification en attente garde la main sur les réglages.
fn reload() {
    let Some(app) = app() else { return };
    let mut view = options::options_get(&app);
    if let Some(p) = with(|s| s.pending.clone()) {
        view.settings = p;
    }
    with(|s| s.view = Some(view));
    fill();
}

/// Pose l'état dans la fenêtre.
fn fill() {
    let (Some(w), Some(view), custom) = with(|s| (s.window.as_ref().and_then(slint::Weak::upgrade), s.view.clone(), s.custom.clone())) else {
        return;
    };
    let g = w.global::<OptionsData>();
    let s = &view.settings;
    g.set_loaded(true);
    g.set_file_error(view.file_error.clone().unwrap_or_default().into());
    g.set_locked(view.file_error.is_some());
    g.set_path(view.path.clone().into());
    g.set_autostart(view.autostart);
    // Barres.
    let bars: Vec<OptionsBar> = view
        .bars
        .iter()
        .map(|b| OptionsBar {
            monitor_id: b.monitor_id.clone().into(),
            name: b.name.clone().into(),
            connected: b.connected,
            style: style_name(b.style).into(),
            interaction: interaction_name(b.interaction).into(),
            interaction_hint: interaction_hint(b.interaction).into(),
            autohide: b.autohide,
            height: b.height as i32,
            height_choice: options::height_choice(b.height, custom.contains(&b.monitor_id)).into(),
            height_hint: if b.height > BAND_HEIGHT {
                "Sous la bande du haut, des tuiles : les panneaux des widgets (« Modifier la barre »)."
            } else {
                "Barre haute : des tuiles sous la bande montrent les panneaux des widgets."
            }
            .into(),
        })
        .collect();
    super::model::sync_rc(&g.get_bars(), bars, |m| g.set_bars(m));
    g.set_reveal_px(s.autohide.reveal_px as i32);
    g.set_reveal_ms(s.autohide.reveal_ms as i32);
    // Alertes.
    let a = &s.alerts;
    g.set_claude_island(a.claude_island.unwrap_or(false));
    g.set_claude_light(a.claude_light.unwrap_or(false));
    g.set_dnd_claude(a.dnd_claude.unwrap_or(false));
    g.set_dnd_battery(a.dnd_battery.unwrap_or(false));
    g.set_dnd_discord(a.dnd_discord.unwrap_or(false));
    g.set_dnd_light(a.dnd_light.unwrap_or(false));
    g.set_headset_low(options::low_label(a.headset_low.unwrap_or(0)).into());
    g.set_devices_low(options::low_label(a.devices_low.unwrap_or(0)).into());
    // Fonctions.
    let f = &s.features;
    let chosen = !f.fallback_output.trim().is_empty();
    let (outputs, _) = with(|s| s.app.clone()).map(|a| crate::mixer::outputs(&a)).unwrap_or_default();
    let missing = chosen && !outputs.is_empty() && !outputs.iter().any(|d| d.id == f.fallback_output);
    let output_name = outputs.iter().find(|d| d.id == f.fallback_output).map(|d| d.name.clone());
    g.set_fallback_output(
        match (chosen, output_name) {
            (false, _) => "Aucune (choisir…)".to_owned(),
            (true, Some(n)) => n,
            (true, None) => "Sortie absente".to_owned(),
        }
        .into(),
    );
    g.set_fallback_chosen(chosen);
    g.set_fallback_missing(missing);
    g.set_capture_to_claude(f.capture_to_claude);
    g.set_headset_switch(f.headset_switch);
    g.set_osd(f.osd);
    g.set_osd_track(f.osd_track);
    g.set_bar_pulse(f.bar_pulse);
    g.set_wallpaper_accent(f.wallpaper_accent);
    g.set_windows_accent(f.windows_accent);
    g.set_game_mode(f.game_mode);
    g.set_phone_island(f.phone_island);
    g.set_auto_update(f.auto_update);
    g.set_updates(crate::update::ENABLED);
    // Nuit.
    g.set_night_start(s.night.start.clone().into());
    g.set_night_end(s.night.end.clone().into());
    g.set_night_percent((s.night.opacity * 100.0).round() as i32);
    g.set_night_dnd(s.night.dnd);
    // Minuteur.
    let presets: Vec<i32> = s.timer.presets.iter().map(|&m| m as i32).collect();
    super::model::sync_rc(&g.get_presets(), presets, |m| g.set_presets(m));
    g.set_timer_focus(s.timer.focus as i32);
    g.set_timer_rest(s.timer.rest as i32);
    // Palette.
    g.set_hotkey(s.palette.hotkey.clone().into());
    if g.get_hotkey_error().is_empty() {
        g.set_hotkey_error(view.hotkey_error.clone().unwrap_or_default().into());
    }
}

fn style_name(s: BarStyle) -> &'static str {
    match s {
        BarStyle::Islands => "islands",
        BarStyle::Graphic => "graphic",
    }
}

fn interaction_name(i: Interaction) -> &'static str {
    match i {
        Interaction::Click => "click",
        Interaction::Hover => "hover",
        Interaction::Info => "info",
    }
}

fn interaction_hint(i: Interaction) -> &'static str {
    match i {
        Interaction::Click => "Panneaux au survol, actions au clic, molette = volume",
        Interaction::Hover => "Panneaux au survol ; les clics ne font rien (sauf sur Fenêtres)",
        Interaction::Info => "Rien ne réagit à la souris (sauf le clic droit)",
    }
}

/// Modifie les réglages affichés (`f` renvoie vrai si quelque chose a changé) : à l'écran tout de
/// suite, enregistrés 400 ms après la dernière modification.
fn change(f: impl FnOnce(&mut Settings) -> bool) {
    let changed = with(|s| {
        let view = s.view.as_mut()?;
        if view.file_error.is_some() {
            return None;
        }
        let mut next = s.pending.clone().unwrap_or_else(|| view.settings.clone());
        if !f(&mut next) {
            return Some(false);
        }
        view.settings = next.clone();
        s.pending = Some(next);
        s.save.start(slint::TimerMode::SingleShot, SAVE_DELAY, flush);
        Some(true)
    });
    // Modifié : à l'écran ; refusé : le champ reprend la valeur en vigueur.
    if changed.is_some() {
        fill();
    }
}

fn edit(id: &str, v: Edit) {
    change(|s| match options::apply_edit(s, id, &v) {
        Ok(()) => true,
        Err(e) => {
            log::debug!("réglages : {e}");
            false
        }
    });
}

/// Enregistre ce qui attend.
fn flush() {
    let Some((settings, app)) = with(|s| {
        s.save.stop();
        let p = s.pending.take()?;
        Some((p, s.app.clone()?))
    }) else {
        return;
    };
    let result = options::options_set(&app, settings);
    let Some(w) = window() else { return };
    let g = w.global::<OptionsData>();
    match result {
        Ok(view) => {
            g.set_error("".into());
            if with(|s| s.pending.is_none()) {
                with(|s| s.view = Some(view));
                fill();
            }
        }
        Err(e) => g.set_error(e.into()),
    }
}

fn set_autostart(on: bool) {
    let Some(app) = app() else { return };
    let result = options::options_set_autostart(&app, on);
    if let Some(w) = window() {
        let g = w.global::<OptionsData>();
        g.set_autostart_error(result.err().unwrap_or_default().into());
        g.set_autostart(crate::tray::autostart_enabled());
    }
}

/// Option d'une barre, appliquée tout de suite (la barre se redessine).
fn set_bar(monitor: &str, key: &str, value: &str) {
    let Some(app) = app() else { return };
    let mut patch = BarOptionsPatch::default();
    match key {
        "style" => patch.style = serde_json::from_value(serde_json::Value::String(value.into())).ok(),
        "interaction" => patch.interaction = serde_json::from_value(serde_json::Value::String(value.into())).ok(),
        "autohide" => patch.autohide = Some(value == "true"),
        "height" if value == "custom" => {
            with(|s| s.custom.insert(monitor.to_owned()));
            return fill();
        }
        // Choix proposé (referme la saisie) ou hauteur tapée (la saisie reste ouverte).
        "height" | "height-typed" => {
            let Ok(h) = value.trim().parse::<f64>() else { return fill() };
            if key == "height" {
                with(|s| s.custom.remove(monitor));
            }
            patch.height = Some((h.round().max(0.0) as u32).clamp(BAND_HEIGHT, MAX_BAR_HEIGHT));
        }
        _ => return,
    }
    if patch == BarOptionsPatch::default() {
        return;
    }
    let result = options::options_set_bar(&app, monitor, patch);
    if let Some(w) = window() {
        w.global::<OptionsData>().set_bars_error(result.err().unwrap_or_default().into());
    }
    reload();
}

/// Liste déroulante : menu Windows sous le champ (point logique de la fenêtre).
fn pick(id: &str, x: f32, y: f32) {
    let (Some(w), Some(app)) = (window(), app()) else { return };
    let Some(hwnd) = crate::platform::hwnd(w.window()) else { return };
    let Some(view) = with(|s| s.view.clone()) else { return };
    let current = &view.settings;
    let entries: Vec<Entry> = match id {
        "alerts.headset_low" | "alerts.devices_low" => {
            let now = if id == "alerts.headset_low" { current.alerts.headset_low } else { current.alerts.devices_low }.unwrap_or(0);
            options::LOW_CHOICES.iter().map(|&v| Entry::check(v.to_string(), options::low_label(v), v == now)).collect()
        }
        "features.fallback_output" => {
            let now = current.features.fallback_output.clone();
            let (outputs, _) = crate::mixer::outputs(&app);
            let mut list = vec![Entry::check("", "Aucune (choisir…)", now.trim().is_empty())];
            list.extend(outputs.iter().map(|d| Entry::check(d.id.clone(), d.name.clone(), d.id == now)));
            if !now.trim().is_empty() && !outputs.is_empty() && !outputs.iter().any(|d| d.id == now) {
                list.push(Entry::check(now.clone(), "Sortie absente", true));
            }
            list
        }
        _ => return,
    };
    let scale = w.window().scale_factor();
    let mut p = windows::Win32::Foundation::POINT { x: (x * scale).round() as i32, y: (y * scale).round() as i32 };
    // SAFETY : fenêtre de ce fil, point local.
    let _ = unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(windows::Win32::Foundation::HWND(hwnd as *mut _), &mut p) };
    let key = id.to_owned();
    menu::show_at(hwnd, entries, true, Some((p.x, p.y)), move |value| {
        edit(&key, Edit::Text(value));
    });
}

// ---- Raccourci de la palette ----

fn hotkey_key(text: SharedString, ctrl: bool, alt: bool, shift: bool, win: bool) {
    let Some(w) = window() else { return };
    let g = w.global::<OptionsData>();
    match options::capture_hotkey(&text, ctrl, alt, shift, win) {
        Capture::Wait => g.set_hotkey_hint("Ajoute la touche…".into()),
        Capture::Refused(why) => g.set_hotkey_hint(why.into()),
        Capture::Ok(t) => save_hotkey(&t),
    }
}

fn save_hotkey(text: &str) {
    let (Some(w), Some(app)) = (window(), app()) else { return };
    let g = w.global::<OptionsData>();
    g.set_capturing(false);
    g.set_hotkey_hint("".into());
    match options::options_set_hotkey(&app, text) {
        Ok(view) => {
            g.set_hotkey_error(view.hotkey_error.clone().unwrap_or_default().into());
            g.set_hotkey_saved(
                if text.is_empty() {
                    "Plus de raccourci : la palette s’ouvre depuis les menus.".to_owned()
                } else {
                    format!("Raccourci {text} en service.")
                }
                .into(),
            );
            with(|s| {
                if let Some(v) = s.view.as_mut() {
                    v.settings.palette = view.settings.palette.clone();
                    v.hotkey_error = view.hotkey_error.clone();
                }
                if let Some(p) = s.pending.as_mut() {
                    p.palette = view.settings.palette.clone();
                }
            });
            fill();
        }
        Err(e) => {
            g.set_hotkey_error(e.into());
            g.set_hotkey_saved("".into());
        }
    }
}

// ---- Page État ----

/// Relit l'état tout de suite puis toutes les 15 s (`on`), ou arrête.
fn follow_status(on: bool) {
    let timer = with(|s| s.status.clone());
    if !on {
        timer.stop();
        return;
    }
    load_status();
    timer.start(slint::TimerMode::Repeated, STATUS_PERIOD, load_status);
}

/// Lecture hors du fil de l'interface (le journal est relu).
fn load_status() {
    let Some(app) = app() else { return };
    crate::util::spawn_named("reglages-etat", move || {
        let view = crate::status::status_get(&app);
        let now = horizon_core::time::now_ms();
        let _ = slint::invoke_from_event_loop(move || show_status(view, now));
    });
}

fn show_status(view: crate::status::StatusView, now: u64) {
    let Some(w) = window() else { return };
    let g = w.global::<OptionsData>();
    let rows: Vec<StatusRow> = view
        .checks
        .iter()
        .map(|c| {
            let detail = match &c.at {
                Some(at) => format!("{} · {} {}", c.detail, at.label, super::time::ago(at.ms, now)),
                None => c.detail.clone(),
            };
            let (action_id, action_label) = c.action.clone().unwrap_or_default();
            StatusRow {
                id: c.id.clone().into(),
                label: c.label.clone().into(),
                detail: detail.into(),
                level: match c.level {
                    crate::status::Level::Ok => 0,
                    crate::status::Level::Off => 1,
                    crate::status::Level::Error => 2,
                },
                action_id: action_id.into(),
                action_label: action_label.into(),
            }
        })
        .collect();
    g.set_failing(rows.iter().filter(|r| r.level == 2).count() as i32);
    super::model::sync_rc(&g.get_checks(), rows, |m| g.set_checks(m));
    let warnings: Vec<SharedString> = view.warnings.iter().map(|w| w.as_str().into()).collect();
    super::model::sync_rc(&g.get_warnings(), warnings, |m| g.set_warnings(m));
    g.set_log_path(view.log_path.into());
    g.set_status_error("".into());
    g.set_status_loaded(true);
}

fn status_action(id: &str) {
    // « Changer » le raccourci : section Palette de la même fenêtre.
    if id == "hotkey" {
        return go("palette".into());
    }
    let Some(app) = app() else { return };
    let result = crate::status::status_action(&app, id);
    if let Some(w) = window() {
        w.global::<OptionsData>().set_action_error(result.err().unwrap_or_default().into());
    }
    slint::Timer::single_shot(Duration::from_millis(1500), load_status);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_match_the_web_version() {
        assert_eq!(SECTIONS.len(), 8);
        for s in SECTIONS {
            assert!(crate::settings_window::SettingsPage::parse(&format!("options:{s}")).is_some(), "{s}");
        }
    }
}
