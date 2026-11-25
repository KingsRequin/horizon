//! Mode édition : modifications de la disposition (déplacer, ajouter, retirer, annuler,
//! rétablir, style, hauteur) et menus contextuels des barres. Repris de
//! `app/src-tauri/src/edit.rs` ; les menus sont décrits ici (`menu::Entry`) et ouverts par
//! `ui::edit` (menu Windows, `ui::menu`), dont le choix revient à [`on_menu`].
//!
//! Différences : les `#[tauri::command]` sont des fonctions appelées par l'interface ; les menus
//! ouvrent la fenêtre de réglages (réglages
//! d'un widget, « Réglages d'Horizon », profils) et la palette.

use crate::bars::Bars;
use crate::events::{EV_EDIT, EV_LAYOUT};
use crate::host::{AppHandle, Emitter, Manager};
use crate::state::AppState;
use crate::ui::menu::Entry;
use crate::util::lock;
use horizon_core::geometry::MonitorInfo;
use horizon_core::layout::{BAND_HEIGHT, BarOptionsPatch, BarRole, BarStyle, Layout, TilesMode, WidgetKind, Zone};
use serde_json::Value;

/// Envoie à chaque fenêtre de barre sa disposition, et ajuste la hauteur des barres qui en ont
/// changé. Les fenêtres sont relevées puis le verrou des barres relâché avant de lire la
/// disposition (ordre des verrous : voir `state`).
pub fn broadcast_layout(app: &AppHandle) {
    crate::bars::sync_heights(app);
    let windows: Vec<(String, String)> = {
        let bars = app.state::<Bars>();
        let list = lock(&bars.list);
        list.iter().map(|w| (w.label.clone(), w.monitor.id.clone())).collect()
    };
    let state = app.state::<AppState>();
    for (label, monitor) in windows {
        if let Some(b) = state.bar(&monitor) {
            let _ = app.emit_to(label.as_str(), EV_LAYOUT, b);
        }
    }
}

/// Applique une modification : état précédent mis dans l'historique, enregistrement, diffusion.
fn mutate(app: &AppHandle, f: impl FnOnce(&mut Layout) -> anyhow::Result<()>) -> Result<(), String> {
    app.state::<AppState>().mutate(f).map_err(|e| e.to_string())?;
    broadcast_layout(app);
    Ok(())
}

/// Glisser-déposer : le widget va dans la zone `zone` de la barre de l'écran `to_monitor_id`,
/// au rang `index`.
pub fn edit_move(app: &AppHandle, widget_id: &str, to_monitor_id: &str, zone: Zone, index: usize) -> Result<(), String> {
    mutate(app, |l| l.move_widget(widget_id, to_monitor_id, zone, index))
}

/// Réglages d'un widget (fenêtre de réglages). Toutes les modifications d'une même
/// ouverture de la fenêtre ne font qu'un état d'annulation.
pub fn edit_set_widget_settings(app: &AppHandle, widget_id: &str, settings: Value) -> Result<(), String> {
    let group = format!("settings:{widget_id}");
    app.state::<AppState>()
        .mutate_in(Some(&group), |l| l.set_widget_settings(widget_id, settings))
        .map_err(|e| e.to_string())?;
    broadcast_layout(app);
    Ok(())
}

/// Ctrl+Z : faux si l'historique est vide.
pub fn edit_undo(app: &AppHandle) -> Result<bool, String> {
    // Écrans relevés avant de toucher à la disposition (ordre des verrous : voir `state`).
    let monitors = connected_monitors(app);
    if !app.state::<AppState>().undo(&monitors) {
        return Ok(false);
    }
    broadcast_layout(app);
    Ok(true)
}

/// Rétablit la dernière modification annulée (Ctrl+Y, Ctrl+Maj+Z).
pub fn edit_redo(app: &AppHandle) -> Result<bool, String> {
    let monitors = connected_monitors(app);
    if !app.state::<AppState>().redo(&monitors) {
        return Ok(false);
    }
    broadcast_layout(app);
    Ok(true)
}

/// Termine le mode édition d'une barre (bouton « Terminé » de la bande d'aide).
pub fn edit_end(app: &AppHandle, monitor_id: &str) -> Result<(), String> {
    edit_bar(app, monitor_id, false)
}

/// Ouvre (`true`) ou termine le mode édition de la barre de l'écran `monitor_id`.
pub fn edit_bar(app: &AppHandle, monitor_id: &str, on: bool) -> Result<(), String> {
    let label = label_of(app, monitor_id).ok_or("barre inconnue")?;
    app.emit_to(&label, EV_EDIT, on).map_err(|e| e.to_string())
}

/// Ouvre le mode édition de la barre de l'écran principal (icône de notification, palette).
pub fn edit_main_bar(app: &AppHandle) -> Result<(), String> {
    let main = app.state::<AppState>().main_monitor().ok_or("pas d’écran principal")?;
    edit_bar(app, &main, true)
}

/// Écrans qui ont une fenêtre de barre en ce moment.
fn connected_monitors(app: &AppHandle) -> Vec<MonitorInfo> {
    let bars = app.state::<Bars>();
    let list = lock(&bars.list);
    list.iter().map(|b| b.monitor.clone()).collect()
}

/// Étiquette de la fenêtre de barre de l'écran `monitor_id`.
pub fn label_of(app: &AppHandle, monitor_id: &str) -> Option<String> {
    let bars = app.state::<Bars>();
    let list = lock(&bars.list);
    Some(list.iter().find(|b| b.monitor.id == monitor_id)?.label.clone())
}

/// Écran de la fenêtre de barre `label`.
pub fn monitor_of(app: &AppHandle, label: &str) -> Option<String> {
    let bars = app.state::<Bars>();
    let list = lock(&bars.list);
    Some(list.iter().find(|b| b.label == label)?.monitor.id.clone())
}

/// Nom d'un rôle de barre dont l'écran n'est pas branché.
fn role_label(role: BarRole) -> &'static str {
    match role {
        BarRole::Main => "Écran principal",
        BarRole::Top => "Écran du haut",
        BarRole::Portrait => "Écran vertical",
        BarRole::Side => "Écran latéral",
    }
}

/// Nom d'un écran pour l'utilisateur (« Écran de droite ») ; écran débranché : d'après le rôle
/// de sa barre (`settings_window::screen_name` de la version Tauri).
pub fn screen_name(app: &AppHandle, monitor_id: &str) -> String {
    let monitors = horizon_sys::monitors::list();
    if let Some((_, name)) = horizon_core::geometry::screen_names(&monitors).into_iter().find(|(id, _)| id == monitor_id) {
        return name;
    }
    let role = app.state::<AppState>().bar(monitor_id).map(|b| b.role);
    role.map_or("Écran débranché", role_label).to_owned()
}

/// Nom affiché d'un genre de widget.
fn label(kind: WidgetKind) -> &'static str {
    crate::widgets::manifest(kind.as_str()).map_or(kind.as_str(), |m| m.label)
}

/// Hauteurs proposées (pixels logiques) ; la bande seule = barre normale.
pub const HEIGHT_CHOICES: [u32; 5] = [BAND_HEIGHT, 240, 360, 480, 640];

/// Nom d'une hauteur dans les menus.
pub fn height_label(h: u32) -> String {
    if h == BAND_HEIGHT { "Normale".into() } else { format!("{h} px") }
}

/// Entrées du sous-menu « Hauteur » : hauteurs proposées, et la hauteur personnalisée en cours
/// si elle n'en fait pas partie (une autre se règle dans « Réglages d'Horizon »).
fn height_entries(monitor_id: &str, current: Option<u32>) -> Vec<Entry> {
    let mut choices = HEIGHT_CHOICES.to_vec();
    if let Some(h) = current.filter(|h| !choices.contains(h)) {
        choices.push(h);
        choices.sort_unstable();
    }
    choices
        .into_iter()
        .map(|h| {
            let text = if HEIGHT_CHOICES.contains(&h) { height_label(h) } else { format!("{h} px (personnalisée)") };
            Entry::check(format!("height|{monitor_id}|{h}"), text, current == Some(h))
        })
        .collect()
}

/// Libellé du menu qui ouvre la palette (raccourci aligné à droite).
const PALETTE: &str = "tray-palette";
/// « Réglages d'Horizon… ».
const OPTIONS: &str = "tray-options";
/// « Modifier la barre » de l'icône de notification (barre principale).
pub const TRAY_EDIT: &str = "tray-edit";

/// Menu de la barre (clic droit hors mode édition, ou sur la barre en mode édition). Sur un
/// widget qui a des réglages (`widget_id`), « Réglages de « X »… » vient en tête.
pub fn menu_bar(app: &AppHandle, monitor_id: &str, widget_id: Option<&str>) -> Vec<Entry> {
    let bar = app.state::<AppState>().bar(monitor_id);
    let style = bar.as_ref().map(|b| b.style);
    let styles = Entry::sub(
        "Style",
        vec![
            Entry::check(format!("style|{monitor_id}|islands"), "Îlots", style == Some(BarStyle::Islands)),
            Entry::check(format!("style|{monitor_id}|graphic"), "Graphique", style == Some(BarStyle::Graphic)),
        ],
    );
    // Animations de la musique de cette barre (comme dans Réglages › Barres).
    let music = bar.as_ref().map(|b| {
        let graphic = b.style == BarStyle::Graphic;
        let has_media = b.widgets().any(|w| w.kind == WidgetKind::MEDIA);
        let list = |key: &str, title: &str| {
            let items = crate::ui::options::music_entries(key, b.music, graphic, has_media)
                .into_iter()
                .map(|e| {
                    let id = format!("music|{monitor_id}|{key}|{}", e.id());
                    e.with_id(id)
                })
                .collect();
            Entry::sub(title, items).enabled(has_media || key == "pulse")
        };
        Entry::sub("Musique", vec![list("pulse", "Animation"), list("backdrop", "Fond de la lecture en cours"), list("progress", "Barre de progression")])
    });
    let mode = bar.as_ref().map(|b| b.tiles_mode);
    let tiles = Entry::sub(
        "Tuiles",
        [(TilesMode::Fixed, "fixed", "Sous la barre"), (TilesMode::Click, "click", "Volet au clic"), (TilesMode::Hover, "hover", "Volet au survol")]
            .into_iter()
            .map(|(m, id, text)| Entry::check(format!("tiles|{monitor_id}|{id}"), text, mode == Some(m)))
            .collect(),
    );
    // La hauteur n'a d'effet que sous la barre (le volet suit ses tuiles).
    let heights =
        (mode != Some(TilesMode::Click) && mode != Some(TilesMode::Hover)).then(|| Entry::sub("Hauteur", height_entries(monitor_id, bar.as_ref().map(|b| b.height))));
    let (profiles, active) = app.state::<AppState>().read(|l| (l.profiles.clone(), l.active_profile.clone()));
    let mut profile_items: Vec<Entry> =
        profiles.iter().map(|p| Entry::check(format!("profile|{}", p.id), p.name.clone(), p.id == active)).collect();
    profile_items.push(Entry::Separator);
    profile_items.push(Entry::item("profile-new", "Nouveau profil (copie de celui-ci)"));
    profile_items.push(Entry::item("profile-manage", "Gérer les profils…"));
    let kind = widget_id.and_then(|id| app.state::<AppState>().read(|l| l.widget(id).map(|w| w.kind)));
    let mut menu = Vec::new();
    if let (Some(id), Some(kind)) = (widget_id, kind) {
        menu.push(Entry::item(format!("settings|{id}"), format!("Réglages de « {} »…", label(kind))));
        menu.push(Entry::Separator);
    }
    menu.extend([
        Entry::item(format!("edit|{monitor_id}"), "Modifier la barre"),
        styles,
        tiles,
    ]);
    menu.extend(heights);
    menu.extend(music);
    menu.extend([
        Entry::sub("Profil", profile_items),
        Entry::check("dnd", "Ne pas déranger", crate::dnd::is_active(app)),
        Entry::Separator,
        Entry::item(PALETTE, crate::tray::palette_label(&crate::palette::hotkey_status(app).0)),
        Entry::item(OPTIONS, "Réglages d’Horizon…"),
        Entry::Separator,
        Entry::item("quit", "Quitter Horizon"),
    ]);
    menu
}

/// Menu d'un widget en mode édition : réglages, autre écran (branché), retirer.
pub fn menu_widget(app: &AppHandle, widget_id: &str, has_settings: bool) -> Vec<Entry> {
    let connected = connected_monitors(app);
    let connected: Vec<&str> = connected.iter().map(|m| m.id.as_str()).collect();
    let others = app.state::<AppState>().read(|l| l.move_targets(widget_id, &connected));
    let moves: Vec<Entry> =
        others.iter().map(|(monitor, _)| Entry::item(format!("move|{widget_id}|{monitor}"), screen_name(app, monitor))).collect();
    let enabled = !moves.is_empty();
    let mut menu = Vec::new();
    if has_settings {
        menu.push(Entry::item(format!("settings|{widget_id}"), "Réglages…"));
    }
    menu.push(Entry::sub("Déplacer vers l’écran", moves).enabled(enabled));
    menu.push(Entry::Separator);
    menu.push(Entry::item(format!("remove|{widget_id}"), "Retirer"));
    menu
}

/// Nom d'une zone dans les identifiants de menu (« left »…).
fn zone_name(z: Zone) -> &'static str {
    match z {
        Zone::Left => "left",
        Zone::Center => "center",
        Zone::Right => "right",
        Zone::Tiles => "tiles",
    }
}

/// Menu « Ajouter un widget » d'une zone en mode édition, avec l'icône de chaque widget ; un
/// widget déjà présent sur la barre le dit. Pour les tuiles d'une barre haute, seulement les
/// widgets qui ont un panneau.
pub fn menu_add(app: &AppHandle, monitor_id: &str, zone: Zone) -> Vec<Entry> {
    let present: Vec<WidgetKind> =
        app.state::<AppState>().bar(monitor_id).map(|b| b.widgets().map(|w| w.kind).collect()).unwrap_or_default();
    crate::widgets::MANIFESTS
        .iter()
        .filter(|m| crate::widgets::manifest(m.id).is_some())
        .chain(crate::plugins::widget_kinds())
        .filter(|m| zone != Zone::Tiles || crate::widgets::has_panel(m.id))
        .map(|m| {
            let text = if present.contains(&WidgetKind::new(m.id)) { format!("{} (déjà sur cette barre)", m.label) } else { m.label.to_owned() };
            Entry::item(format!("add|{monitor_id}|{}|{}", zone_name(zone), m.id), text).with_icon(m.icon)
        })
        .collect()
}

/// Choix d'un menu (identifiant des entrées ci-dessus, de l'icône de notification).
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    Quit,
    Dnd,
    Reload,
    EditMain,
    Options,
    Palette,
    Edit(String),
    Style(String, BarStyle),
    Height(String, u32),
    Tiles(String, TilesMode),
    /// Animation de la musique d'une barre : écran, option (backdrop, progress, pulse), choix.
    Music(String, BarOptionsPatch),
    Move(String, String),
    Remove(String),
    SuggestionBlock(String),
    Profile(String),
    ProfileNew,
    ProfileManage,
    Settings(String),
    Add(String, Zone, WidgetKind),
}

fn parse<T: serde::de::DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(Value::String(s.to_owned())).ok()
}

/// Choix d'une animation de la musique : `key` (backdrop, progress, pulse) et identifiant.
fn music_patch(key: &str, value: &str) -> Option<BarOptionsPatch> {
    use horizon_core::settings::{Backdrop, Progress, Pulse};
    let mut p = BarOptionsPatch::default();
    match key {
        "backdrop" => p.backdrop = Some(Backdrop::parse(value)?),
        "progress" => p.progress = Some(Progress::parse(value)?),
        "pulse" => p.pulse = Some(Pulse::parse(value)?),
        _ => return None,
    }
    Some(p)
}

impl MenuAction {
    /// Action d'un identifiant de menu (`None` : inconnu ou mal formé).
    pub fn parse(id: &str) -> Option<MenuAction> {
        let parts: Vec<&str> = id.split('|').collect();
        Some(match parts.as_slice() {
            ["quit"] => MenuAction::Quit,
            ["dnd"] => MenuAction::Dnd,
            ["tray-reload"] => MenuAction::Reload,
            [TRAY_EDIT] => MenuAction::EditMain,
            [OPTIONS] => MenuAction::Options,
            [PALETTE] => MenuAction::Palette,
            ["edit", monitor] => MenuAction::Edit((*monitor).into()),
            ["style", monitor, style] => MenuAction::Style((*monitor).into(), parse(style)?),
            ["height", monitor, h] => MenuAction::Height((*monitor).into(), h.parse().ok()?),
            ["tiles", monitor, mode] => MenuAction::Tiles((*monitor).into(), parse(mode)?),
            ["music", monitor, key, value] => MenuAction::Music((*monitor).into(), music_patch(key, value)?),
            ["move", widget, monitor] => MenuAction::Move((*widget).into(), (*monitor).into()),
            ["remove", widget] => MenuAction::Remove((*widget).into()),
            ["sblock", key] => MenuAction::SuggestionBlock((*key).into()),
            ["profile", id] => MenuAction::Profile((*id).into()),
            ["profile-new"] => MenuAction::ProfileNew,
            ["profile-manage"] => MenuAction::ProfileManage,
            ["settings", widget] => MenuAction::Settings((*widget).into()),
            ["add", monitor, zone, kind] => MenuAction::Add((*monitor).into(), parse(zone)?, parse(kind)?),
            _ => return None,
        })
    }
}

/// Actions des menus contextuels (fil de l'interface : le choix arrive après la fermeture du
/// menu).
pub fn on_menu(app: &AppHandle, id: &str) {
    log::debug!("menu : {id}");
    let Some(action) = MenuAction::parse(id) else {
        log::warn!("menu inconnu : {id}");
        return;
    };
    let result = match action {
        MenuAction::Quit => {
            app.exit(0);
            Ok(())
        }
        MenuAction::Dnd => {
            crate::dnd::toggle(app);
            Ok(())
        }
        MenuAction::Reload => {
            crate::bars::rebuild(app);
            Ok(())
        }
        MenuAction::EditMain => edit_main_bar(app),
        MenuAction::Options => {
            crate::settings_window::open(app, crate::settings_window::SettingsPage::Options("barres".into()));
            Ok(())
        }
        MenuAction::Palette => {
            crate::palette::open(app);
            Ok(())
        }
        MenuAction::Edit(monitor) => edit_bar(app, &monitor, true),
        MenuAction::Style(monitor, s) => mutate(app, |l| l.set_style(&monitor, s)),
        MenuAction::Height(monitor, h) => {
            mutate(app, |l| l.set_bar_options(&monitor, BarOptionsPatch { height: Some(h), ..Default::default() }))
        }
        MenuAction::Tiles(monitor, m) => {
            mutate(app, |l| l.set_bar_options(&monitor, BarOptionsPatch { tiles_mode: Some(m), ..Default::default() }))
        }
        MenuAction::Music(monitor, patch) => mutate(app, |l| l.set_bar_options(&monitor, patch)),
        MenuAction::Move(widget, monitor) => mutate(app, |l| l.move_to_monitor(&widget, &monitor)),
        MenuAction::Remove(widget) => mutate(app, |l| l.remove_widget(&widget).map(|_| ())),
        MenuAction::SuggestionBlock(key) => {
            // Requête SQL : hors du fil de l'interface.
            let a = app.clone();
            crate::util::start_thread("suggestion-exclue", move || {
                if let Err(e) = crate::widgets::suggestions::block(&a, &key) {
                    log::warn!("ne plus proposer {key} : {e}");
                }
            })
            .map_err(|e| e.to_string())
        }
        MenuAction::Profile(id) => crate::profiles::switch(app, &id),
        MenuAction::ProfileNew => crate::profiles::add_from_menu(app),
        MenuAction::ProfileManage => {
            crate::settings_window::open(app, crate::settings_window::SettingsPage::Profiles);
            Ok(())
        }
        MenuAction::Settings(widget) => {
            crate::settings_window::open(app, crate::settings_window::SettingsPage::Widget(widget));
            Ok(())
        }
        MenuAction::Add(monitor, zone, kind) => mutate(app, |l| {
            let id = l.add_widget(&monitor, zone, kind)?;
            l.set_widget_settings(&id, crate::widgets::shortcuts::initial_settings(kind))
        }),
    };
    if let Err(e) = result {
        log::warn!("menu {id} : {e}");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn menu_ids_are_parsed() {
        assert_eq!(MenuAction::parse("quit"), Some(MenuAction::Quit));
        assert_eq!(MenuAction::parse("style|m1|graphic"), Some(MenuAction::Style("m1".into(), BarStyle::Graphic)));
        assert_eq!(MenuAction::parse("style|m1|autre"), None);
        assert_eq!(MenuAction::parse("height|m1|480"), Some(MenuAction::Height("m1".into(), 480)));
        assert_eq!(MenuAction::parse("height|m1|x"), None);
        assert_eq!(MenuAction::parse("tiles|m1|hover"), Some(MenuAction::Tiles("m1".into(), TilesMode::Hover)));
        assert_eq!(MenuAction::parse("tiles|m1|x"), None);
        assert_eq!(
            MenuAction::parse("music|m1|pulse|horizon"),
            Some(MenuAction::Music("m1".into(), BarOptionsPatch { pulse: Some(horizon_core::settings::Pulse::Horizon), ..Default::default() }))
        );
        assert_eq!(MenuAction::parse("music|m1|pulse|stroboscope"), None);
        assert_eq!(MenuAction::parse("music|m1|couleur|halo"), None);
        assert_eq!(
            MenuAction::parse("add|m1|tiles|weather"),
            Some(MenuAction::Add("m1".into(), Zone::Tiles, WidgetKind::WEATHER))
        );
        assert_eq!(MenuAction::parse("add|m1|haut|weather"), None);
        assert_eq!(MenuAction::parse("move|w3|m2"), Some(MenuAction::Move("w3".into(), "m2".into())));
        assert_eq!(MenuAction::parse(TRAY_EDIT), Some(MenuAction::EditMain));
        assert_eq!(MenuAction::parse("profile|jeu"), Some(MenuAction::Profile("jeu".into())));
        assert_eq!(MenuAction::parse("inconnu|x"), None);
    }

    #[test]
    fn heights_offer_the_custom_one_in_order() {
        let texts = |c: Option<u32>| height_entries("m", c).iter().map(|e| e.text().to_owned()).collect::<Vec<_>>();
        assert_eq!(texts(Some(40)), ["Normale", "240 px", "360 px", "480 px", "640 px"]);
        assert_eq!(texts(Some(300))[2], "300 px (personnalisée)");
        let checked: Vec<String> = height_entries("m", Some(480))
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item { id, checked: Some(true), .. } => Some(id),
                _ => None,
            })
            .collect();
        assert_eq!(checked, ["height|m|480"]);
    }

    /// Hôte d'essai : disposition par défaut d'un écran, en mémoire.
    pub(crate) fn app_with_layout(name: &str) -> (AppHandle, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("horizon-natif-edit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let monitors = [MonitorInfo {
            id: "m1".into(),
            device_name: "m1".into(),
            rect: horizon_core::geometry::Rect { x: 0, y: 0, w: 1920, h: 1080 },
            work: horizon_core::geometry::Rect { x: 0, y: 40, w: 1920, h: 1040 },
            scale: 1.0,
            primary: true,
            portrait: false,
        }];
        let path = dir.join("layout.json");
        let state = AppState::new(path.clone(), horizon_core::config::load_layout(&path, &monitors));
        state.sync_monitors(&monitors);
        let app = AppHandle::new();
        app.manage(state);
        app.manage(Bars::default());
        app.manage(crate::dnd::DndHub::default());
        (app, dir)
    }

    #[test]
    fn menus_of_the_bar_and_of_a_widget() {
        let (app, dir) = app_with_layout("menus");
        let id = app.state::<AppState>().read(|l| l.bars[0].left[0].id.clone());
        let bar = menu_bar(&app, "m1", Some(&id));
        let texts: Vec<&str> = bar.iter().map(Entry::text).collect();
        assert!(texts[0].starts_with("Réglages de « "), "{texts:?}");
        assert!(texts.contains(&"Modifier la barre") && texts.contains(&"Quitter Horizon"));
        let Some(Entry::Sub { items, .. }) = bar.iter().find(|e| e.text() == "Profil") else { panic!("sous-menu Profil") };
        assert_eq!(items.last().map(Entry::id), Some("profile-manage"));
        assert!(matches!(&items[0], Entry::Item { checked: Some(true), .. }), "profil actif coché");
        // Sans widget : pas de réglages en tête.
        assert_eq!(menu_bar(&app, "m1", None)[0].id(), "edit|m1");
        // Un seul écran branché (aucune barre ouverte ici) : « Déplacer » grisé.
        let w = menu_widget(&app, &id, true);
        assert_eq!(w[0].id(), format!("settings|{id}"));
        assert!(matches!(&w[1], Entry::Sub { enabled: false, .. }));
        assert_eq!(w.last().map(Entry::id), Some(format!("remove|{id}").as_str()));
        // Menu d'ajout : les tuiles n'offrent que les widgets à panneau.
        let all = menu_add(&app, "m1", Zone::Left);
        let tiles = menu_add(&app, "m1", Zone::Tiles);
        assert!(tiles.len() < all.len() && !tiles.is_empty());
        assert!(all.iter().any(|e| e.text().ends_with("(déjà sur cette barre)")));
        assert!(all.iter().all(|e| matches!(e, Entry::Item { icon: Some(_), .. })));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn menu_actions_change_the_layout_in_memory() {
        let (app, dir) = app_with_layout("actions");
        let state = app.state::<AppState>();
        let before = state.bar("m1").unwrap();
        let id = before.left[0].id.clone();
        on_menu(&app, "style|m1|graphic");
        assert_eq!(state.bar("m1").unwrap().style, BarStyle::Graphic);
        on_menu(&app, "height|m1|480");
        assert_eq!(state.bar("m1").unwrap().height, 480);
        on_menu(&app, &format!("remove|{id}"));
        assert!(state.read(|l| l.widget(&id).is_none()));
        on_menu(&app, "add|m1|tiles|weather");
        assert_eq!(state.bar("m1").unwrap().tiles.last().map(|w| w.kind), Some(WidgetKind::WEATHER));
        edit_move(&app, &before.right[0].id, "m1", Zone::Left, 0).unwrap();
        assert_eq!(state.bar("m1").unwrap().left[0].id, before.right[0].id);
        assert!(edit_move(&app, "inconnu", "m1", Zone::Left, 0).is_err());
        // Annuler tout, puis rétablir une étape.
        while edit_undo(&app).unwrap() {}
        assert_eq!(state.bar("m1").unwrap(), before);
        assert!(edit_redo(&app).unwrap());
        assert_eq!(state.bar("m1").unwrap().style, BarStyle::Graphic);
        let _ = std::fs::remove_dir_all(dir);
    }
}
