//! « Réglages d'Horizon » : réglages globaux (`settings.toml`), options de chaque barre (style,
//! interaction, masquage en plein écran, hauteur, dans `layout.json`) et démarrage avec Windows.
//! Tout s'applique à chaud. Repris de `app/src-tauri/src/options.rs` ; différences : les
//! commandes sont des fonctions appelées par la fenêtre (`ui::options`).
//!
//! `settings.toml` est aussi relu toutes les 5 s (modifié à la main, ou par la version
//! installée) : un changement prévient ce qui ne relit pas les réglages de lui-même, et les
//! fenêtres reçoivent `EV_OPTIONS`.

use crate::events::EV_OPTIONS;
use crate::host::{AppHandle, Emitter, Manager};
use crate::state::{AppState, settings, settings_path};
use crate::util::{lock, start_thread};
use horizon_core::layout::{BarOptionsPatch, BarRole, BarStyle, Interaction, TilesMode};
use horizon_core::settings::{Settings, SettingsFile};
use std::sync::Mutex;
use std::time::Duration;

const PERIOD: Duration = Duration::from_secs(5);

/// Derniers réglages vus (relecture du fichier, enregistrement par la fenêtre).
static LAST: Mutex<Option<Settings>> = Mutex::new(None);

/// Options d'une barre.
#[derive(Debug, Clone, PartialEq)]
pub struct BarOptions {
    pub monitor_id: String,
    /// « Écran principal », « Écran de droite »…
    pub name: String,
    pub role: BarRole,
    pub style: BarStyle,
    pub interaction: Interaction,
    pub autohide: bool,
    /// Hauteur en pixels logiques (40 = barre normale ; au-delà, des tuiles sous la bande).
    pub height: u32,
    /// Tuiles sous la barre ou dans un volet.
    pub tiles_mode: TilesMode,
    /// L'écran est branché.
    pub connected: bool,
}

#[derive(Debug, Clone)]
pub struct OptionsView {
    /// Réglages en vigueur ; les alertes avec les valeurs d'avant (widgets) comprises.
    pub settings: Settings,
    pub bars: Vec<BarOptions>,
    pub autostart: bool,
    /// `settings.toml` illisible : la raison (rien n'est enregistré tant qu'il l'est).
    pub file_error: Option<String>,
    /// Raccourci de la palette demandé qui n'a pas pu être pris.
    pub hotkey_error: Option<String>,
    pub path: String,
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    *lock(&LAST) = Some(settings());
    let app = app.clone();
    start_thread("reglages", move || {
        loop {
            std::thread::sleep(PERIOD);
            horizon_sys::presence::wait_while_away();
            let now = settings();
            let changed = {
                let mut last = lock(&LAST);
                let changed = last.as_ref() != Some(&now);
                *last = Some(now);
                changed
            };
            if changed {
                log::info!("réglages modifiés");
                applied(&app);
            }
        }
    })
}

/// État de la fenêtre des réglages.
pub fn options_get(app: &AppHandle) -> OptionsView {
    let mut s = settings();
    let layout = app.state::<AppState>().snapshot();
    s.alerts = s.alerts.resolved(&layout);
    let connected = horizon_sys::monitors::list();
    let bars = layout
        .bars
        .iter()
        .map(|b| BarOptions {
            monitor_id: b.monitor_id.clone(),
            name: crate::settings_window::screen_name(app, &b.monitor_id),
            role: b.role,
            style: b.style,
            interaction: b.interaction,
            autohide: b.autohide,
            height: b.height,
            tiles_mode: b.tiles_mode,
            connected: connected.iter().any(|m| m.id == b.monitor_id),
        })
        .collect();
    let file_error = match Settings::inspect(&settings_path()) {
        Ok(SettingsFile::Invalid(why)) => Some(why),
        Err(e) => Some(e.to_string()),
        _ => None,
    };
    OptionsView {
        settings: s,
        bars,
        autostart: crate::tray::autostart_enabled(),
        file_error,
        hotkey_error: crate::palette::hotkey_status(app).1,
        path: settings_path().display().to_string(),
    }
}

/// Après une modification : les fonctions qui ne relisent pas les réglages d'elles-mêmes sont
/// prévenues, et les fenêtres reçoivent le nouvel état.
fn applied(app: &AppHandle) {
    app.state::<crate::comfort::Night>().wake();
    app.state::<crate::autohide::Autohide>().wake();
    app.state::<crate::accent::Accent>().wake();
    crate::osd::settings_changed();
    crate::timer::broadcast(app);
    crate::dnd::reapply(app);
    crate::status::wake(app);
    let _ = app.emit(EV_OPTIONS, ());
}

/// Enregistre les réglages globaux (le raccourci de la palette a sa propre commande).
pub fn options_set(app: &AppHandle, settings: Settings) -> Result<OptionsView, String> {
    let mut s = settings;
    s.palette = crate::state::settings().palette;
    crate::state::save_settings(&s).map_err(|e| format!("{e:#}"))?;
    *lock(&LAST) = Some(crate::state::settings());
    applied(app);
    Ok(options_get(app))
}

/// Change le raccourci de la palette : pris à chaud, puis enregistré. Déjà pris par une autre
/// application : l'ancien reste, rien n'est enregistré, l'erreur est renvoyée.
pub fn options_set_hotkey(app: &AppHandle, hotkey: &str) -> Result<OptionsView, String> {
    let mut s = crate::state::settings();
    if let Ok(SettingsFile::Invalid(why)) = Settings::inspect(&settings_path()) {
        return Err(format!("settings.toml est illisible ({why}) : corrige-le ou supprime-le d’abord"));
    }
    let result = crate::palette::set_hotkey(app, hotkey);
    crate::status::wake(app);
    result?;
    s.palette.hotkey = hotkey.trim().to_owned();
    crate::state::save_settings(&s).map_err(|e| format!("{e:#}"))?;
    *lock(&LAST) = Some(crate::state::settings());
    crate::tray::sync_palette(app);
    applied(app);
    Ok(options_get(app))
}

/// Options d'une barre ; `None` = inchangée. Annulable comme toute modification de la
/// disposition (Ctrl+Z du mode édition), comme le style changé par le menu de la barre.
pub fn options_set_bar(app: &AppHandle, monitor_id: &str, patch: BarOptionsPatch) -> Result<(), String> {
    app.state::<AppState>().mutate(|l| l.set_bar_options(monitor_id, patch)).map_err(|e| e.to_string())?;
    crate::edit::broadcast_layout(app);
    applied(app);
    Ok(())
}

pub fn options_set_autostart(app: &AppHandle, on: bool) -> Result<(), String> {
    let result = crate::tray::set_autostart(on);
    let _ = app.emit(EV_OPTIONS, ());
    result
}

// ---- Règles de la fenêtre (logique pure, testée) ----

/// Seuils de batterie faible proposés (`LOW_CHOICES` de lib/battery.ts) ; 0 = pas d'alerte.
pub const LOW_CHOICES: [u8; 7] = [0, 5, 10, 15, 20, 25, 30];
/// Hauteurs proposées au-delà de la barre normale (`HEIGHTS` de BarsSection.svelte).
pub const HEIGHT_CHOICES: [u32; 4] = [240, 360, 480, 640];
/// Durées proposées au plus par le minuteur.
const MAX_PRESETS: usize = 6;

/// Valeur d'une modification venue de la fenêtre.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Bool(bool),
    Int(i64),
    Text(String),
}

/// Libellé d'un seuil de batterie faible.
pub fn low_label(v: u8) -> String {
    if v == 0 { "Pas d’alerte".into() } else { format!("Sous {v} %") }
}

/// Choix de hauteur affiché : une hauteur proposée, sinon « custom » (aussi pendant la saisie).
pub fn height_choice(height: u32, custom: bool) -> String {
    let preset = height == horizon_core::layout::BAND_HEIGHT || HEIGHT_CHOICES.contains(&height);
    if custom || !preset { "custom".into() } else { height.to_string() }
}

/// Heure « HH:MM » normalisée (`<input type="time">`) : « 7:5 », « 07h05 », « 0705 » → « 07:05 ».
pub fn normalize_time(s: &str) -> Option<String> {
    let s = s.trim();
    let (h, m) = match s.split_once([':', 'h', 'H']) {
        Some((h, m)) => (h, if m.is_empty() { "0" } else { m }),
        None if s.len() == 4 && s.chars().all(|c| c.is_ascii_digit()) => s.split_at(2),
        None if (1..=2).contains(&s.len()) => (s, "0"),
        None => return None,
    };
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

/// Applique la modification `id` (chemin dans `settings.toml`) ; refusée si l'option est
/// inconnue ou la valeur illisible. Mêmes règles que les sections Svelte (bornes, sortie de
/// repli vidée = bascule du casque coupée).
pub fn apply_edit(s: &mut Settings, id: &str, v: &Edit) -> Result<(), String> {
    let bad = || format!("valeur illisible pour {id} : {v:?}");
    let flag = || match v {
        Edit::Bool(b) => Ok(*b),
        _ => Err(bad()),
    };
    let int = |lo: i64, hi: i64| match v {
        Edit::Int(n) => Ok((*n).clamp(lo, hi)),
        Edit::Text(t) => t.trim().parse::<f64>().map(|n| (n.round() as i64).clamp(lo, hi)).map_err(|_| bad()),
        Edit::Bool(_) => Err(bad()),
    };
    let text = || match v {
        Edit::Text(t) => Ok(t.clone()),
        _ => Err(bad()),
    };
    let (a, f, n) = (&mut s.alerts, &mut s.features, &mut s.night);
    match id {
        "alerts.claude_island" => a.claude_island = Some(flag()?),
        "alerts.claude_light" => a.claude_light = Some(flag()?),
        "alerts.dnd_claude" => a.dnd_claude = Some(flag()?),
        "alerts.dnd_battery" => a.dnd_battery = Some(flag()?),
        "alerts.dnd_discord" => a.dnd_discord = Some(flag()?),
        "alerts.dnd_light" => a.dnd_light = Some(flag()?),
        "alerts.headset_low" => a.headset_low = Some(int(0, 100)? as u8),
        "features.capture_to_claude" => f.capture_to_claude = flag()?,
        "features.headset_switch" => f.headset_switch = flag()? && !f.fallback_output.trim().is_empty(),
        "features.fallback_output" => {
            f.fallback_output = text()?;
            if f.fallback_output.trim().is_empty() {
                f.headset_switch = false;
            }
        }
        "features.osd" => f.osd = flag()?,
        "features.osd_track" => f.osd_track = flag()?,
        "features.bar_pulse" => f.bar_pulse = flag()?,
        "features.wallpaper_accent" => f.wallpaper_accent = flag()?,
        "features.windows_accent" => f.windows_accent = flag()?,
        "features.game_mode" => f.game_mode = flag()?,
        "features.auto_update" => f.auto_update = flag()?,
        "night.start" => n.start = normalize_time(&text()?).ok_or_else(bad)?,
        "night.end" => n.end = normalize_time(&text()?).ok_or_else(bad)?,
        "night.opacity" => n.opacity = int(10, 100)? as f32 / 100.0,
        "night.dnd" => n.dnd = flag()?,
        "autohide.reveal" => s.autohide.reveal = flag()?,
        "autohide.reveal_px" => s.autohide.reveal_px = int(0, 200)? as u32,
        "autohide.reveal_ms" => s.autohide.reveal_ms = int(0, 5000)? as u64,
        "timer.focus" => s.timer.focus = int(1, 600)? as u32,
        "timer.rest" => s.timer.rest = int(1, 600)? as u32,
        _ => return Err(format!("option inconnue : {id}")),
    }
    Ok(())
}

/// Ajoute une durée proposée (minutes tapées) : bornée à 1–600, sans doublon, six au plus,
/// triées. Faux si rien n'est ajouté.
pub fn add_preset(s: &mut Settings, typed: &str) -> bool {
    let typed = typed.trim();
    let Ok(m) = typed.parse::<f64>() else { return false };
    let m = (m.round() as i64).clamp(1, 600) as u32;
    let p = &mut s.timer.presets;
    if p.contains(&m) || p.len() >= MAX_PRESETS {
        return false;
    }
    p.push(m);
    p.sort_unstable();
    true
}

/// Retire une durée proposée (il en reste toujours une).
pub fn remove_preset(s: &mut Settings, minutes: u32) -> bool {
    let p = &mut s.timer.presets;
    if p.len() < 2 || !p.contains(&minutes) {
        return false;
    }
    p.retain(|&x| x != minutes);
    true
}

/// Raccourci saisi au clavier (`captureHotkey` de lib/hotkey.ts).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capture {
    /// Seuls des modificateurs sont enfoncés : attendre la touche.
    Wait,
    Refused(&'static str),
    /// Texte du raccourci, écrit comme `settings.toml` l'attend (« Ctrl+Alt+P »).
    Ok(String),
}

/// Le texte d'un événement clavier de Slint est la touche `k`.
fn is_key(text: &str, k: &slint::platform::Key) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c == char::from(*k))
}

/// Nom de touche lu par `Hotkey::parse` d'après le texte d'un événement clavier de Slint ;
/// `None` pour une touche non prise en charge.
fn key_name(text: &str) -> Option<String> {
    use slint::platform::Key;
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase().to_string());
    }
    let named = [
        (Key::Space, "Space"),
        (Key::Return, "Enter"),
        (Key::Tab, "Tab"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
        (Key::End, "End"),
        (Key::Home, "Home"),
        (Key::LeftArrow, "Left"),
        (Key::UpArrow, "Up"),
        (Key::RightArrow, "Right"),
        (Key::DownArrow, "Down"),
        (Key::Insert, "Insert"),
        (Key::Delete, "Delete"),
    ];
    if let Some((_, name)) = named.iter().find(|(k, _)| is_key(text, k)) {
        return Some((*name).to_owned());
    }
    let functions = [
        Key::F1, Key::F2, Key::F3, Key::F4, Key::F5, Key::F6, Key::F7, Key::F8, Key::F9, Key::F10, Key::F11, Key::F12,
        Key::F13, Key::F14, Key::F15, Key::F16, Key::F17, Key::F18, Key::F19, Key::F20, Key::F21, Key::F22, Key::F23,
        Key::F24,
    ];
    functions.iter().position(|k| is_key(text, k)).map(|i| format!("F{}", i + 1))
}

/// Un modificateur seul (Ctrl, Alt, Maj, Win, AltGr).
fn is_modifier(text: &str) -> bool {
    use slint::platform::Key;
    [Key::Shift, Key::ShiftR, Key::Control, Key::ControlR, Key::Alt, Key::AltGr, Key::Meta, Key::MetaR]
        .iter()
        .any(|k| is_key(text, k))
}

/// Raccourci d'un appui de touche (texte de l'événement Slint et modificateurs).
pub fn capture_hotkey(text: &str, ctrl: bool, alt: bool, shift: bool, win: bool) -> Capture {
    if is_modifier(text) {
        return Capture::Wait;
    }
    let Some(key) = key_name(text) else { return Capture::Refused("touche non prise en charge") };
    if !ctrl && !alt && !win {
        return Capture::Refused("il faut Ctrl, Alt ou Win avec la touche");
    }
    let mods = [(ctrl, "Ctrl"), (alt, "Alt"), (shift, "Shift"), (win, "Win")];
    let mut parts: Vec<&str> = mods.iter().filter(|(on, _)| *on).map(|(_, m)| *m).collect();
    parts.push(&key);
    Capture::Ok(parts.join("+"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::Key;

    fn key(k: Key) -> String {
        char::from(k).to_string()
    }

    #[test]
    fn hotkeys_are_captured_like_the_web_version() {
        assert_eq!(capture_hotkey(&key(Key::Control), true, false, false, false), Capture::Wait);
        assert_eq!(capture_hotkey("p", true, true, false, false), Capture::Ok("Ctrl+Alt+P".into()));
        assert_eq!(capture_hotkey("5", false, true, true, false), Capture::Ok("Alt+Shift+5".into()));
        assert_eq!(capture_hotkey(&key(Key::F5), false, false, false, true), Capture::Ok("Win+F5".into()));
        assert_eq!(capture_hotkey(&key(Key::F12), true, false, false, false), Capture::Ok("Ctrl+F12".into()));
        assert_eq!(capture_hotkey(" ", true, false, false, false), Capture::Ok("Ctrl+Space".into()));
        assert_eq!(capture_hotkey(&key(Key::LeftArrow), true, false, false, false), Capture::Ok("Ctrl+Left".into()));
        assert_eq!(capture_hotkey("p", false, false, true, false), Capture::Refused("il faut Ctrl, Alt ou Win avec la touche"));
        assert_eq!(capture_hotkey("€", true, true, false, false), Capture::Refused("touche non prise en charge"));
        // Chaque raccourci produit se relit.
        for t in ["Ctrl+Alt+P", "Alt+Shift+5", "Win+F5", "Ctrl+Space", "Ctrl+Left", "Ctrl+F12"] {
            assert!(horizon_core::hotkey::Hotkey::parse(t).is_some(), "{t}");
        }
    }

    #[test]
    fn edits_follow_the_sections_rules() {
        let mut s = Settings::default();
        apply_edit(&mut s, "features.osd", &Edit::Bool(false)).unwrap();
        assert!(!s.features.osd);
        apply_edit(&mut s, "alerts.headset_low", &Edit::Int(20)).unwrap();
        assert_eq!(s.alerts.headset_low, Some(20));
        // Bascule du casque : seulement avec une sortie de repli, coupée quand on la retire.
        apply_edit(&mut s, "features.fallback_output", &Edit::Text(String::new())).unwrap();
        apply_edit(&mut s, "features.headset_switch", &Edit::Bool(true)).unwrap();
        assert!(!s.features.headset_switch);
        apply_edit(&mut s, "features.fallback_output", &Edit::Text("{id}".into())).unwrap();
        apply_edit(&mut s, "features.headset_switch", &Edit::Bool(true)).unwrap();
        assert!(s.features.headset_switch);
        apply_edit(&mut s, "features.fallback_output", &Edit::Text(String::new())).unwrap();
        assert!(!s.features.headset_switch);
        // Bornes.
        apply_edit(&mut s, "autohide.reveal_px", &Edit::Int(900)).unwrap();
        assert_eq!(s.autohide.reveal_px, 200);
        apply_edit(&mut s, "autohide.reveal_ms", &Edit::Text("75.4".into())).unwrap();
        assert_eq!(s.autohide.reveal_ms, 75);
        apply_edit(&mut s, "timer.focus", &Edit::Int(0)).unwrap();
        assert_eq!(s.timer.focus, 1);
        apply_edit(&mut s, "night.opacity", &Edit::Int(55)).unwrap();
        assert!((s.night.opacity - 0.55).abs() < 1e-6);
        apply_edit(&mut s, "night.start", &Edit::Text("7h5".into())).unwrap();
        assert_eq!(s.night.start, "07:05");
        // Refus : rien ne change.
        let before = s.clone();
        assert!(apply_edit(&mut s, "night.end", &Edit::Text("25:00".into())).is_err());
        assert!(apply_edit(&mut s, "features.osd", &Edit::Int(1)).is_err());
        assert!(apply_edit(&mut s, "inconnu", &Edit::Bool(true)).is_err());
        assert_eq!(s, before);
    }

    #[test]
    fn times_are_normalized() {
        assert_eq!(normalize_time("22:00").as_deref(), Some("22:00"));
        assert_eq!(normalize_time(" 7:05 ").as_deref(), Some("07:05"));
        assert_eq!(normalize_time("0705").as_deref(), Some("07:05"));
        assert_eq!(normalize_time("7h").as_deref(), Some("07:00"));
        assert_eq!(normalize_time("23").as_deref(), Some("23:00"));
        assert_eq!(normalize_time("24:00"), None);
        assert_eq!(normalize_time("12:60"), None);
        assert_eq!(normalize_time("midi"), None);
    }

    #[test]
    fn presets_stay_sorted_unique_and_bounded() {
        let mut s = Settings::default();
        assert_eq!(s.timer.presets, [5, 15, 25, 50]);
        assert!(add_preset(&mut s, "10"));
        assert_eq!(s.timer.presets, [5, 10, 15, 25, 50]);
        assert!(!add_preset(&mut s, "10"), "doublon");
        assert!(!add_preset(&mut s, ""), "vide");
        assert!(!add_preset(&mut s, "abc"));
        assert!(add_preset(&mut s, "9999"));
        assert_eq!(s.timer.presets.last(), Some(&600));
        assert!(!add_preset(&mut s, "3"), "six au plus");
        assert!(remove_preset(&mut s, 600));
        assert!(!remove_preset(&mut s, 42));
        s.timer.presets = vec![5];
        assert!(!remove_preset(&mut s, 5), "il en reste une");
    }

    #[test]
    fn heights_and_thresholds_have_their_labels() {
        assert_eq!(height_choice(40, false), "40");
        assert_eq!(height_choice(480, false), "480");
        assert_eq!(height_choice(500, false), "custom");
        assert_eq!(height_choice(480, true), "custom");
        assert_eq!(low_label(0), "Pas d’alerte");
        assert_eq!(low_label(15), "Sous 15 %");
    }
}
