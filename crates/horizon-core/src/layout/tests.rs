//! Tests de la disposition (aides partagées avec les autres modules de tests).

use super::*;
use super::migrations::{
    MIGRATIONS, MIGRATION_CLAUDE_SESSIONS, MIGRATION_HEADSET, MIGRATION_SERVERS, MIGRATION_TOP_HOVER, MIGRATION_WIDGETS_2,
    MIGRATION_WINDOWS, MIGRATION_ESSAIM, MIGRATION_PLUGINS,
};
use super::profiles::DEFAULT_PROFILE;
use crate::geometry::{MonitorInfo, Rect};
use serde_json::json;

pub fn mon(id: &str, x: i32, y: i32, w: i32, h: i32, primary: bool) -> MonitorInfo {
    MonitorInfo {
        id: id.into(),
        device_name: id.into(),
        rect: Rect { x, y, w, h },
        work: Rect { x, y, w, h },
        scale: 1.0,
        primary,
        portrait: h > w,
    }
}

/// Les 4 écrans réels de l'utilisateur (relevés le 9 novembre 2026).
pub fn real_monitors() -> Vec<MonitorInfo> {
    vec![
        mon("main", 0, 0, 1920, 1080, true),
        mon("right", 1920, -853, 1080, 1920, false),
        mon("top", -1936, -2160, 3840, 2160, false),
        mon("left", -1920, 0, 1920, 1080, false),
    ]
}

fn kinds(v: &[WidgetInstance]) -> Vec<WidgetKind> {
    v.iter().map(|w| w.kind).collect()
}

fn bar<'a>(l: &'a Layout, id: &str) -> &'a BarLayout {
    l.bars.iter().find(|b| b.monitor_id == id).unwrap()
}

#[test]
fn default_layout_matches_spec_screens() {
    let l = Layout::default_for(&real_monitors());
    assert_eq!(l.version, LAYOUT_VERSION);
    assert_eq!(l.bars.len(), 4);

    let main = bar(&l, "main");
    assert_eq!((main.role, main.style, main.interaction), (BarRole::Main, BarStyle::Islands, Interaction::Click));
    assert_eq!(kinds(&main.left), vec![WidgetKind::MEDIA, WidgetKind::DISCORD_VOICE, WidgetKind::CALENDAR, WidgetKind::WINDOWS]);
    assert_eq!(kinds(&main.center), vec![WidgetKind::SHORTCUTS, WidgetKind::SUGGESTIONS]);
    assert_eq!(
        kinds(&main.right),
        vec![WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS, WidgetKind::HEADSET, WidgetKind::SHARKAPP_PLUGIN, WidgetKind::MIC_KRISP, WidgetKind::PRISME_PLUGIN, WidgetKind::ESSAIM, WidgetKind::SYSTEM, WidgetKind::WEATHER, WidgetKind::CLOCK]
    );

    let top = bar(&l, "top");
    assert_eq!((top.role, top.style, top.interaction), (BarRole::Top, BarStyle::Graphic, Interaction::Hover));
    assert_eq!(kinds(&top.left), vec![WidgetKind::CLOCK, WidgetKind::WEATHER, WidgetKind::CALENDAR, WidgetKind::SERVERS]);
    assert_eq!(top.left[0].settings["showDate"], json!(true));
    assert_eq!(kinds(&top.center), vec![WidgetKind::MEDIA]);
    assert_eq!(kinds(&top.right), vec![WidgetKind::NOTIFICATIONS, WidgetKind::CLAUDE_SESSIONS, WidgetKind::CLAUDE_QUOTA, WidgetKind::SYSTEM]);

    let right = bar(&l, "right");
    assert_eq!((right.role, right.style, right.interaction), (BarRole::Portrait, BarStyle::Graphic, Interaction::Click));
    assert_eq!(kinds(&right.left), vec![WidgetKind::SHORTCUTS, WidgetKind::SUGGESTIONS]);
    assert_eq!(kinds(&right.right), vec![WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS, WidgetKind::MIC_KRISP, WidgetKind::SYSTEM, WidgetKind::CLOCK]);

    let left = bar(&l, "left");
    assert_eq!((left.role, left.style, left.interaction), (BarRole::Side, BarStyle::Graphic, Interaction::Click));
    assert_eq!(kinds(&left.left), vec![WidgetKind::SHORTCUTS, WidgetKind::SUGGESTIONS]);
    assert_eq!(kinds(&left.center), vec![WidgetKind::MEDIA]);
    assert_eq!(kinds(&left.right), vec![WidgetKind::SYSTEM, WidgetKind::CLOCK]);
}

#[test]
fn new_layout_has_every_migration_and_migrate_changes_nothing() {
    let l = Layout::default_for(&real_monitors());
    let marks: Vec<&str> = MIGRATIONS.iter().map(|(m, _)| *m).collect();
    assert_eq!(l.migrations, marks);
    let mut again = l.clone();
    again.migrate();
    assert_eq!(again, l);
}

#[test]
fn fully_migrated_legacy_layout_matches_a_new_one() {
    let mut l = legacy();
    l.migrate();
    let new = Layout::default_for(&real_monitors());
    assert_eq!(l.migrations, new.migrations);
    for (a, b) in l.bars.iter().zip(&new.bars) {
        for z in [Zone::Left, Zone::Center, Zone::Right] {
            let (mut a, mut b) = (a.clone(), b.clone());
            assert_eq!(kinds(a.zone_mut(z)), kinds(b.zone_mut(z)), "barre {} zone {z:?}", a.monitor_id);
        }
    }
}

const WIDGETS_2: [WidgetKind; 5] =
    [WidgetKind::DISCORD_VOICE, WidgetKind::CALENDAR, WidgetKind::SHARKAPP_PLUGIN, WidgetKind::PRISME_PLUGIN, WidgetKind::WEATHER];

/// Disposition d'avant le widget Suggestions (version installée le 10 novembre 2026).
fn legacy() -> Layout {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        for z in [Zone::Left, Zone::Center, Zone::Right] {
            b.zone_mut(z).retain(|w| {
                !matches!(
                    w.kind,
                    WidgetKind::SUGGESTIONS | WidgetKind::HEADSET | WidgetKind::SERVERS | WidgetKind::WINDOWS | WidgetKind::ESSAIM
                )
                    && !WIDGETS_2.contains(&w.kind)
            });
        }
    }
    l.migrations.clear();
    l
}

/// Disposition installée le 11 novembre 2026 (avant les widgets de la v2).
fn before_widgets_2() -> Layout {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        for z in [Zone::Left, Zone::Center, Zone::Right] {
            b.zone_mut(z).retain(|w| {
                !WIDGETS_2.contains(&w.kind) && !matches!(w.kind, WidgetKind::SERVERS | WidgetKind::WINDOWS | WidgetKind::ESSAIM)
            });
        }
    }
    l.migrations.retain(|m| ![MIGRATION_WIDGETS_2, MIGRATION_SERVERS, MIGRATION_WINDOWS, MIGRATION_ESSAIM].contains(&m.as_str()));
    l
}

#[test]
fn migrate_adds_widgets_2_once_where_expected() {
    let mut l = before_widgets_2();
    let added = l.migrate();
    assert_eq!(added, vec![WidgetKind::DISCORD_VOICE, WidgetKind::CALENDAR, WidgetKind::SHARKAPP_PLUGIN, WidgetKind::PRISME_PLUGIN, WidgetKind::WEATHER, WidgetKind::SERVERS, WidgetKind::WINDOWS, WidgetKind::ESSAIM], "ajouts annoncés");
    assert_eq!(
        kinds(&bar(&l, "main").right),
        vec![WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS, WidgetKind::HEADSET, WidgetKind::SHARKAPP_PLUGIN, WidgetKind::MIC_KRISP, WidgetKind::PRISME_PLUGIN, WidgetKind::ESSAIM, WidgetKind::SYSTEM, WidgetKind::WEATHER, WidgetKind::CLOCK]
    );
    assert_eq!(kinds(&bar(&l, "main").left), vec![WidgetKind::MEDIA, WidgetKind::DISCORD_VOICE, WidgetKind::CALENDAR, WidgetKind::WINDOWS]);
    assert_eq!(kinds(&bar(&l, "top").left), vec![WidgetKind::CLOCK, WidgetKind::WEATHER, WidgetKind::CALENDAR, WidgetKind::SERVERS]);
    assert!(!bar(&l, "left").widgets().any(|w| WIDGETS_2.contains(&w.kind)));
    assert!(l.migrate().is_empty());
    assert_eq!(count(&l, WidgetKind::WEATHER), 2);
}

#[test]
fn new_layout_announces_nothing_and_new_profile_can_wait() {
    let mut l = Layout::default_for(&real_monitors());
    assert!(l.migrate().is_empty());
    let id = l.add_profile("Jeu", false).unwrap();
    assert_eq!(l.active_profile, DEFAULT_PROFILE, "pas activé");
    assert_eq!(l.profiles.iter().find(|p| p.id == id).unwrap().bars, l.bars, "copie de l'actif");
}

#[test]
fn move_to_monitor_keeps_the_place() {
    let mut l = Layout::default_for(&real_monitors());
    let id = bar(&l, "main").right[2].id.clone();
    l.move_to_monitor(&id, "left").unwrap();
    assert_eq!(bar(&l, "left").right[2].id, id);
    let last = bar(&l, "main").right.last().unwrap().id.clone();
    l.move_to_monitor(&last, "right").unwrap();
    assert_eq!(bar(&l, "right").right.last().unwrap().id, last, "rang borné à la fin de la zone");
    assert!(l.move_to_monitor("absent", "left").is_err());
}

#[test]
fn migrate_adds_servers_to_top_bar_once() {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        b.left.retain(|w| w.kind != WidgetKind::SERVERS);
    }
    l.migrations.retain(|m| m != MIGRATION_SERVERS);
    l.migrate();
    assert_eq!(kinds(&bar(&l, "top").left), vec![WidgetKind::CLOCK, WidgetKind::WEATHER, WidgetKind::CALENDAR, WidgetKind::SERVERS]);
    assert_eq!(count(&l, WidgetKind::SERVERS), 1);
    // Retiré ensuite par l'utilisateur : pas remis.
    for b in &mut l.bars {
        b.left.retain(|w| w.kind != WidgetKind::SERVERS);
    }
    l.migrate();
    assert_eq!(count(&l, WidgetKind::SERVERS), 0);
}

#[test]
fn weather_place_is_shared_between_copies() {
    let mut l = Layout::default_for(&real_monitors());
    let find = |l: &Layout, id: &str| bar(l, id).widgets().find(|w| w.kind == WidgetKind::WEATHER).unwrap().clone();
    let (main, top) = (find(&l, "main").id, find(&l, "top").id);
    let place = json!({ "place": { "name": "Lyon", "lat": 45.75, "lon": 4.85 } });
    l.set_widget_settings(&main, place.clone()).unwrap();
    assert_eq!(find(&l, "top").settings, place, "un exemplaire pas encore réglé suit");
    // Réglage partagé : changé depuis n'importe quel exemplaire, les autres suivent.
    let brest = json!({ "place": { "name": "Brest", "lat": 48.39, "lon": -4.49 } });
    l.set_widget_settings(&top, brest.clone()).unwrap();
    assert_eq!(find(&l, "main").settings, brest);
    // Un exemplaire réglé autrement (fichier d'une version précédente) garde le sien.
    let nice = json!({ "place": { "name": "Nice", "lat": 43.7, "lon": 7.27 } });
    l.widget_mut(&top).unwrap().settings = nice.clone();
    l.set_widget_settings(&main, place.clone()).unwrap();
    assert_eq!(find(&l, "top").settings, nice);
}

#[test]
fn old_bar_format_and_bar_options() {
    let old = json!({ "monitor_id": "m", "monitor_pos": [0, 0], "role": "top", "style": "graphic",
        "interactive": false, "left": [], "center": [], "right": [] });
    let b: BarLayout = serde_json::from_value(old).unwrap();
    assert_eq!((b.interaction, b.autohide), (Interaction::Info, true));
    let v = serde_json::to_value(&b).unwrap();
    assert_eq!(v["interaction"], json!("info"));
    assert!(v.get("interactive").is_none());
    let again: BarLayout = serde_json::from_value(v).unwrap();
    assert_eq!(again, b);
    let mut l = Layout::default_for(&real_monitors());
    let patch = BarOptionsPatch { interaction: Some(Interaction::Hover), autohide: Some(false), ..Default::default() };
    l.set_bar_options("top", patch).unwrap();
    let top = bar(&l, "top");
    assert_eq!((top.style, top.interaction, top.autohide), (BarStyle::Graphic, Interaction::Hover, false));
    assert_eq!(top.height, BAND_HEIGHT, "hauteur inchangée");
    assert!(l.set_bar_options("absent", BarOptionsPatch::default()).is_err());
}

/// Disposition d'avant le widget Casque (suggestions déjà migrées).
fn before_headset() -> Layout {
    let mut l = legacy();
    l.migrate();
    for b in &mut l.bars {
        b.right.retain(|w| w.kind != WidgetKind::HEADSET);
    }
    l.migrations.retain(|m| m != MIGRATION_HEADSET);
    l
}

#[test]
fn migrate_adds_headset_before_mic_on_main_bar_once() {
    let mut l = before_headset();
    l.migrate();
    let right: Vec<WidgetKind> = kinds(&bar(&l, "main").right).into_iter().filter(|k| !WIDGETS_2.contains(k) && *k != WidgetKind::ESSAIM).collect();
    assert_eq!(right, vec![WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS, WidgetKind::HEADSET, WidgetKind::MIC_KRISP, WidgetKind::SYSTEM, WidgetKind::CLOCK]);
    assert_eq!(count(&l, WidgetKind::HEADSET), 1);
    l.migrate();
    assert_eq!(count(&l, WidgetKind::HEADSET), 1);
}

#[test]
fn migrate_headset_without_mic_goes_first_right() {
    let mut l = before_headset();
    l.bars.iter_mut().find(|b| b.monitor_id == "main").unwrap().right.retain(|w| w.kind != WidgetKind::MIC_KRISP);
    l.migrate();
    assert_eq!(bar(&l, "main").right[0].kind, WidgetKind::HEADSET);
}

fn count(l: &Layout, k: WidgetKind) -> usize {
    l.widgets_of(k).count()
}

#[test]
fn migrate_adds_suggestions_once() {
    let mut l = legacy();
    l.migrate();
    assert_eq!(kinds(&bar(&l, "main").center), vec![WidgetKind::SHORTCUTS, WidgetKind::SUGGESTIONS]);
    assert_eq!(count(&l, WidgetKind::SUGGESTIONS), 3);
    l.migrate();
    assert_eq!(count(&l, WidgetKind::SUGGESTIONS), 3);
}

#[test]
fn migrate_skips_non_interactive_bar() {
    let mut l = legacy();
    l.migrate();
    assert!(!bar(&l, "top").widgets().any(|w| w.kind == WidgetKind::SUGGESTIONS));
}

#[test]
fn migrate_respects_removal() {
    let mut l = legacy();
    l.migrate();
    let id = bar(&l, "main").center[1].id.clone();
    l.remove_widget(&id).unwrap();
    // Rechargée depuis le fichier (marque conservée), la disposition n'est pas modifiée.
    let mut reloaded: Layout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    reloaded.migrate();
    assert_eq!(count(&reloaded, WidgetKind::SUGGESTIONS), 2);
}

#[test]
fn migrate_without_shortcuts_appends_left() {
    let mut l = legacy();
    let b = l.bars.iter_mut().find(|b| b.monitor_id == "left").unwrap();
    b.left.clear();
    l.migrate();
    assert_eq!(kinds(&bar(&l, "left").left), vec![WidgetKind::SUGGESTIONS]);
}

#[test]
fn old_snake_case_setting_keys_are_renamed_once() {
    let mut l = legacy();
    let id = l.first_of(WidgetKind::SHORTCUTS).unwrap().id.clone();
    l.set_widget_settings(&id, json!({ "low_alert": 20, "items": [] })).unwrap();
    l.migrate();
    assert_eq!(l.widget(&id).unwrap().settings, json!({ "lowAlert": 20, "items": [] }));
    assert!(l.migrations.iter().any(|m| m == "camel-case"));
    // Déjà renommé : la nouvelle clé l'emporte sur une ancienne revenue.
    let mut both = legacy();
    let id = both.first_of(WidgetKind::SHORTCUTS).unwrap().id.clone();
    both.set_widget_settings(&id, json!({ "low_alert": 5, "lowAlert": 30 })).unwrap();
    both.migrate();
    assert_eq!(both.widget(&id).unwrap().settings, json!({ "lowAlert": 30 }));
}

#[test]
fn old_file_without_migrations_field_loads() {
    let mut v = serde_json::to_value(legacy()).unwrap();
    v.as_object_mut().unwrap().remove("migrations");
    let l: Layout = serde_json::from_value(v).unwrap();
    assert!(l.migrations.is_empty());
}

#[test]
fn bar_for_reports_renamed_monitor() {
    let monitors = real_monitors();
    let mut l = Layout::default_for(&monitors);
    assert_eq!(l.bar_for_tracked(&monitors[0], &monitors).1, None);
    let mut renamed = monitors.clone();
    renamed[0].id = "main-2".into();
    let (b, old) = l.bar_for_tracked(&renamed[0], &renamed);
    assert_eq!(b.monitor_id, "main-2");
    assert_eq!(old.as_deref(), Some("main"));
}

#[test]
fn widget_ids_are_unique() {
    let l = Layout::default_for(&real_monitors());
    let mut ids: Vec<&str> = l.widgets().map(|w| w.id.as_str()).collect();
    let n = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), n);
}

#[test]
fn layout_matches_monitor_by_device_then_position() {
    let monitors = real_monitors();
    let mut l = Layout::default_for(&monitors);
    // Même identifiant : retrouvé tel quel.
    assert_eq!(l.bar_for_tracked(&monitors[2], &monitors).0.role, BarRole::Top);
    // Identifiant changé mais même position : retrouvé, et l'identifiant est mis à jour.
    let mut renamed = monitors[2].clone();
    renamed.id = "top-renamed".into();
    let mut after = monitors.clone();
    after[2] = renamed.clone();
    let b = l.bar_for_tracked(&renamed, &after).0;
    assert_eq!(b.role, BarRole::Top);
    assert_eq!(b.monitor_id, "top-renamed");
    assert_eq!(l.bars.len(), 4);
    // Écran inconnu : nouvelle barre par défaut, sans doublon.
    let new = mon("new", 5000, 0, 1920, 1080, false);
    let mut all = after.clone();
    all.push(new.clone());
    assert_eq!(l.bar_for_tracked(&new, &all).0.role, BarRole::Side);
    assert_eq!(l.bars.len(), 5);
    l.bar_for_tracked(&new, &all);
    assert_eq!(l.bars.len(), 5);
}

#[test]
fn move_widget_across_zones_and_monitors() {
    let mut l = Layout::default_for(&real_monitors());
    let (left, right) = (bar(&l, "main").left.len(), bar(&l, "main").right.len());
    let clock = bar(&l, "main").right.iter().find(|w| w.kind == WidgetKind::CLOCK).unwrap().id.clone();
    l.move_widget(&clock, "main", Zone::Left, 0).unwrap();
    assert_eq!(bar(&l, "main").left[0].id, clock);
    assert_eq!(bar(&l, "main").right.len(), right - 1);
    // Index trop grand : ajouté en fin de zone.
    l.move_widget(&clock, "left", Zone::Center, 99).unwrap();
    assert_eq!(bar(&l, "left").center.last().unwrap().id, clock);
    assert_eq!(bar(&l, "main").left.len(), left);
    assert!(l.move_widget("absent", "main", Zone::Left, 0).is_err());
    assert!(l.move_widget(&clock, "absent", Zone::Left, 0).is_err());
    // Un échec ne perd pas le widget.
    assert_eq!(bar(&l, "left").center.last().unwrap().id, clock);
}

#[test]
fn add_and_remove_widget() {
    let mut l = Layout::default_for(&real_monitors());
    let id = l.add_widget("top", Zone::Center, WidgetKind::CLOCK).unwrap();
    assert_eq!(bar(&l, "top").center.last().unwrap().id, id);
    let removed = l.remove_widget(&id).unwrap();
    assert_eq!(removed.kind, WidgetKind::CLOCK);
    assert!(l.remove_widget(&id).is_err());
    assert!(l.add_widget("absent", Zone::Left, WidgetKind::CLOCK).is_err());
}

#[test]
fn set_style_and_settings() {
    let mut l = Layout::default_for(&real_monitors());
    l.set_style("left", BarStyle::Islands).unwrap();
    assert_eq!(bar(&l, "left").style, BarStyle::Islands);
    assert!(l.set_style("absent", BarStyle::Islands).is_err());
    let id = bar(&l, "main").center[0].id.clone();
    l.set_widget_settings(&id, json!({ "items": [] })).unwrap();
    assert_eq!(bar(&l, "main").center[0].settings, json!({ "items": [] }));
    assert!(l.set_widget_settings("absent", Value::Null).is_err());
}

fn has_media(l: &Layout) -> bool {
    bar(l, "main").left.iter().any(|w| w.kind == WidgetKind::MEDIA)
}

fn profiled() -> (Layout, String) {
    let mut l = Layout::default_for(&real_monitors());
    let id = l.add_profile("Jeu", true).unwrap();
    // Le profil « Jeu » n'a plus de média sur la barre principale.
    let media = bar(&l, "main").left[0].id.clone();
    l.remove_widget(&media).unwrap();
    (l, id)
}

#[test]
fn add_profile_copies_and_activates() {
    let mut l = Layout::default_for(&real_monitors());
    assert_eq!(l.profiles.len(), 1);
    let before = l.bars.clone();
    let id = l.add_profile("Jeu", true).unwrap();
    assert_eq!(l.active_profile, id);
    assert_eq!(l.bars, before);
    assert_eq!(l.profiles.iter().find(|p| p.id == DEFAULT_PROFILE).unwrap().bars, before);
    assert!(l.profiles.iter().find(|p| p.id == id).unwrap().bars.is_empty());
    assert_eq!(l.next_profile_name(), "Profil 2");
}

#[test]
fn switching_keeps_each_profile_layout() {
    let (mut l, jeu) = profiled();
    assert!(!has_media(&l));
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    assert!(has_media(&l));
    l.switch_profile(&jeu, false).unwrap();
    assert!(!has_media(&l));
    assert!(l.switch_profile("absent", false).is_err());
    // Rechargé depuis le fichier : identique.
    let back: Layout = serde_json::from_str(&serde_json::to_string(&l).unwrap()).unwrap();
    assert_eq!(back, l);
}

#[test]
fn app_rules_switch_and_return_to_manual_profile() {
    let (mut l, jeu) = profiled();
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    l.set_profile_apps(&jeu, vec!["game.exe".into()]).unwrap();
    assert_eq!(l.profile_for_app(Some("notepad.exe")), None);
    assert_eq!(l.profile_for_app(Some("game.exe")), Some(jeu.clone()));
    l.switch_profile(&jeu, true).unwrap();
    assert_eq!(l.auto_from.as_deref(), Some(DEFAULT_PROFILE));
    // Toujours le jeu : rien à changer ; une autre appli : retour au profil de départ.
    assert_eq!(l.profile_for_app(Some("game.exe")), None);
    assert_eq!(l.profile_for_app(Some("notepad.exe")), Some(DEFAULT_PROFILE.into()));
    l.switch_profile(DEFAULT_PROFILE, true).unwrap();
    assert_eq!(l.auto_from, None);
    assert_eq!(l.profile_for_app(None), None);
}

#[test]
fn manual_switch_clears_auto_return() {
    let (mut l, jeu) = profiled();
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    l.switch_profile(&jeu, true).unwrap();
    l.switch_profile(&jeu, false).unwrap();
    assert_eq!(l.auto_from, None);
}

#[test]
fn an_app_activates_only_one_profile() {
    let (mut l, jeu) = profiled();
    l.set_profile_apps(DEFAULT_PROFILE, vec!["a.exe".into(), "b.exe".into()]).unwrap();
    l.set_profile_apps(&jeu, vec!["b.exe".into(), "b.exe".into()]).unwrap();
    assert_eq!(l.profiles[0].apps, vec!["a.exe".to_string()]);
    assert_eq!(l.profiles[1].apps, vec!["b.exe".to_string()]);
}

#[test]
fn delete_and_rename_profiles() {
    let (mut l, jeu) = profiled();
    l.rename_profile(&jeu, "  Stream ").unwrap();
    assert_eq!(l.profiles[1].name, "Stream");
    assert!(l.rename_profile(&jeu, " ").is_err());
    l.delete_profile(&jeu).unwrap();
    assert_eq!(l.active_profile, DEFAULT_PROFILE);
    assert!(has_media(&l));
    assert!(l.delete_profile(DEFAULT_PROFILE).is_err());
}

#[test]
fn layout_without_profiles_gets_default_one() {
    let mut v = serde_json::to_value(Layout::default_for(&real_monitors())).unwrap();
    for k in ["profiles", "active_profile", "auto_from"] {
        v.as_object_mut().unwrap().remove(k);
    }
    let mut l: Layout = serde_json::from_value(v).unwrap();
    l.migrate();
    assert_eq!(l.profiles.len(), 1);
    assert_eq!(l.active_profile, DEFAULT_PROFILE);
}

#[test]
fn migrations_reach_inactive_profiles() {
    let mut l = legacy();
    l.ensure_profiles();
    let jeu = l.add_profile("Jeu", true).unwrap();
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    l.migrate();
    l.switch_profile(&jeu, false).unwrap();
    assert!(l.first_of(WidgetKind::HEADSET).is_some());
}

#[test]
fn follower_switches_at_once_and_returns_after_the_delay() {
    use std::time::{Duration, Instant};
    let (mut l, game) = profiled();
    l.set_profile_apps(&game, vec!["jeu.exe".into()]).unwrap();
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    let mut f = ProfileFollower::new(Duration::from_secs(2));
    let t0 = Instant::now();
    assert_eq!(f.next(&l, t0), None, "aucune appli suivie");
    f.foreground("jeu.exe".into());
    assert_eq!(f.next(&l, t0).as_deref(), Some(game.as_str()), "activé tout de suite");
    l.switch_profile(&game, true).unwrap();
    f.foreground("autre.exe".into());
    assert_eq!(f.next(&l, t0), None, "retour différé");
    assert_eq!(f.deadline(), Some(t0 + Duration::from_secs(2)));
    assert_eq!(f.next(&l, t0 + Duration::from_secs(1)), None);
    assert_eq!(f.next(&l, t0 + Duration::from_secs(2)).as_deref(), Some(DEFAULT_PROFILE));
    // Retour sur le jeu avant la fin du délai : rien à faire, l'attente est oubliée.
    l.switch_profile(&game, true).unwrap();
    f.foreground("autre.exe".into());
    assert_eq!(f.next(&l, t0), None);
    f.foreground("jeu.exe".into());
    assert_eq!(f.next(&l, t0), None);
    assert_eq!(f.deadline(), None);
}

#[test]
fn undo_restores_previous() {
    let mut l = Layout::default_for(&real_monitors());
    let mut undo = UndoStack::default();
    let before = l.clone();
    undo.push(l.clone(), None);
    let id = bar(&l, "main").left[0].id.clone();
    l.remove_widget(&id).unwrap();
    assert_eq!(undo.undo(l.clone()), Some(before));
    assert_eq!(undo.undo(l.clone()), None);
}

#[test]
fn move_targets_skip_unplugged_screens_and_own_bar() {
    let l = Layout::default_for(&real_monitors());
    let id = bar(&l, "main").left[0].id.clone();
    // L'écran 4K (« top ») est éteint : il garde sa barre dans la disposition.
    let targets = l.move_targets(&id, &["main", "right", "left"]);
    let ids: Vec<&str> = targets.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(ids, vec!["right", "left"]);
    assert_eq!(targets[0].1, BarRole::Portrait);
    assert!(l.move_targets("inconnu", &["main"]).iter().any(|(m, _)| m == "main"));
}

#[test]
fn undo_after_plugging_a_screen_keeps_its_bar() {
    let all = real_monitors();
    let three: Vec<MonitorInfo> = all.iter().filter(|m| m.id != "top").cloned().collect();
    let mut l = Layout::default_for(&three);
    let mut undo = UndoStack::default();
    // Une modification, puis l'écran du haut est rebranché.
    undo.push(l.clone(), None);
    let id = bar(&l, "main").left[0].id.clone();
    l.remove_widget(&id).unwrap();
    l.ensure_monitors(&all);
    assert_eq!(l.bars.len(), 4);
    // Ctrl+Z : l'état d'avant le branchement revient, la barre du nouvel écran reste.
    let mut previous = undo.undo(l.clone()).unwrap();
    previous.ensure_monitors(&all);
    assert_eq!(previous.bars.len(), 4);
    assert!(previous.bars.iter().any(|b| b.monitor_id == "top"));
    assert_eq!(bar(&previous, "main").left[0].id, id);
}

#[test]
fn ensure_monitors_reports_renamed_screens() {
    let all = real_monitors();
    let mut l = Layout::default_for(&all);
    let mut moved = all.clone();
    moved[3].id = "left-2".into();
    assert_eq!(l.ensure_monitors(&moved), vec![("left".to_string(), "left-2".to_string())]);
    assert_eq!(l.ensure_monitors(&moved), vec![]);
    assert_eq!(l.bars.len(), 4);
}

#[test]
fn undo_keeps_last_20() {
    let base = Layout::default_for(&real_monitors());
    let mut undo = UndoStack::default();
    for i in 0..25 {
        let mut s = base.clone();
        s.version = i;
        undo.push(s, None);
    }
    let mut n = 0;
    let mut last = None;
    while let Some(s) = undo.undo(base.clone()) {
        n += 1;
        last = Some(s.version);
    }
    assert_eq!(n, UndoStack::DEPTH);
    assert_eq!(last, Some(5));
}

#[test]
fn claude_sessions_and_top_hover_migrations() {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        if b.role != BarRole::Top {
            b.right.retain(|w| w.kind != WidgetKind::CLAUDE_SESSIONS);
        }
        if b.role == BarRole::Top {
            b.interaction = Interaction::Info;
        }
    }
    l.migrations.retain(|m| m != MIGRATION_CLAUDE_SESSIONS && m != MIGRATION_TOP_HOVER);
    let added = l.migrate();
    assert_eq!(added, vec![WidgetKind::CLAUDE_SESSIONS], "ajout annoncé, pas le changement de mode");
    assert_eq!(kinds(&bar(&l, "main").right)[..2], [WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS]);
    assert_eq!(kinds(&bar(&l, "right").right)[..2], [WidgetKind::CLAUDE_SESSIONS, WidgetKind::NOTIFICATIONS]);
    assert!(!kinds(&bar(&l, "left").right).contains(&WidgetKind::CLAUDE_SESSIONS), "pas sur l'écran de gauche");
    assert_eq!(bar(&l, "top").interaction, Interaction::Hover);

    // Une barre du haut réglée « Cliquable » le reste ; un widget retiré ne revient pas.
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        if b.role == BarRole::Top {
            b.interaction = Interaction::Click;
        }
        b.right.retain(|w| w.kind != WidgetKind::CLAUDE_SESSIONS);
    }
    l.migrations.retain(|m| m != MIGRATION_TOP_HOVER);
    l.migrate();
    assert_eq!(bar(&l, "top").interaction, Interaction::Click);
    assert!(!kinds(&bar(&l, "main").right).contains(&WidgetKind::CLAUDE_SESSIONS));
}

#[test]
fn windows_widget_migration_reaches_main_bar_once() {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        b.left.retain(|w| w.kind != WidgetKind::WINDOWS);
    }
    l.migrations.retain(|m| m != MIGRATION_WINDOWS);
    let added = l.migrate();
    assert_eq!(added, vec![WidgetKind::WINDOWS], "annoncé dans « Nouveautés »");
    assert_eq!(kinds(&bar(&l, "main").left), vec![WidgetKind::MEDIA, WidgetKind::DISCORD_VOICE, WidgetKind::CALENDAR, WidgetKind::WINDOWS]);
    assert_eq!(count(&l, WidgetKind::WINDOWS), 1, "seulement sur la barre principale");
    // Retiré ensuite : il ne revient pas ; déjà présent : pas de doublon.
    bar_mut(&mut l, "main").left.retain(|w| w.kind != WidgetKind::WINDOWS);
    assert!(l.migrate().is_empty());
    assert_eq!(count(&l, WidgetKind::WINDOWS), 0);
    let mut l = Layout::default_for(&real_monitors());
    l.migrations.retain(|m| m != MIGRATION_WINDOWS);
    l.migrate();
    assert_eq!(count(&l, WidgetKind::WINDOWS), 1);
}

#[test]
fn migrate_adds_essaim_next_to_lighting_once() {
    let without = |l: &mut Layout| {
        for b in &mut l.bars {
            b.right.retain(|w| w.kind != WidgetKind::ESSAIM);
        }
        l.migrations.retain(|m| m != MIGRATION_ESSAIM);
    };
    let mut l = Layout::default_for(&real_monitors());
    without(&mut l);
    assert_eq!(l.migrate(), vec![WidgetKind::ESSAIM], "ajout annoncé");
    let right = kinds(&bar(&l, "main").right);
    let i = right.iter().position(|k| *k == WidgetKind::PRISME_PLUGIN).unwrap();
    assert_eq!(right[i + 1], WidgetKind::ESSAIM, "juste après l'Éclairage");
    assert_eq!(count(&l, WidgetKind::ESSAIM), 1, "barre principale seulement");
    // Retiré ensuite : il ne revient pas.
    bar_mut(&mut l, "main").right.retain(|w| w.kind != WidgetKind::ESSAIM);
    assert!(l.migrate().is_empty());
    assert_eq!(count(&l, WidgetKind::ESSAIM), 0);
    // Sans Éclairage : avant le système.
    let mut l = Layout::default_for(&real_monitors());
    without(&mut l);
    bar_mut(&mut l, "main").right.retain(|w| w.kind != WidgetKind::PRISME_PLUGIN);
    l.migrate();
    let right = kinds(&bar(&l, "main").right);
    let i = right.iter().position(|k| *k == WidgetKind::SYSTEM).unwrap();
    assert_eq!(right[i - 1], WidgetKind::ESSAIM);
}

fn bar_mut<'a>(l: &'a mut Layout, id: &str) -> &'a mut BarLayout {
    l.bars.iter_mut().find(|b| b.monitor_id == id).unwrap()
}
#[test]
fn lighting_and_batteries_become_plugins_in_place() {
    let mut l = Layout::default_for(&real_monitors());
    let main = bar_mut(&mut l, "main");
    let i = main.right.iter().position(|w| w.kind == WidgetKind::PRISME_PLUGIN).unwrap();
    // Disposition d'avant la migration : les widgets intégrés, avec un seuil réglé.
    main.right[i].kind = WidgetKind::LIGHTING;
    let j = main.right.iter().position(|w| w.kind == WidgetKind::SHARKAPP_PLUGIN).unwrap();
    main.right[j].kind = WidgetKind::BATTERIES;
    main.right[j].settings = json!({ "lowAlert": 20 });
    let ids = (main.right[i].id.clone(), main.right[j].id.clone());
    l.migrations.retain(|m| m != MIGRATION_PLUGINS);
    assert!(l.migrate().is_empty(), "rien d'annoncé");
    let main = bar(&l, "main");
    assert_eq!(main.right[i].kind, WidgetKind::PRISME_PLUGIN);
    assert_eq!(main.right[i].id, ids.0, "même exemplaire");
    assert_eq!(main.right[j].kind, WidgetKind::SHARKAPP_PLUGIN);
    assert_eq!(main.right[j].settings, json!({ "seuil": 20 }));
    assert_eq!(main.right[j].id, ids.1);
}

// --- Barre haute : hauteur et zone des tuiles ---

#[test]
fn old_bar_without_height_is_a_normal_bar() {
    let old = json!({ "monitor_id": "m", "monitor_pos": [0, 0], "role": "portrait", "style": "graphic",
        "interaction": "click", "left": [], "center": [], "right": [] });
    let b: BarLayout = serde_json::from_value(old).unwrap();
    assert_eq!(b.height, BAND_HEIGHT);
    assert!(b.tiles.is_empty());
    assert!(!b.has_tiles_area());
    let v = serde_json::to_value(&b).unwrap();
    assert_eq!((v["height"].clone(), v["tiles"].clone()), (json!(40), json!([])));
    // Une disposition neuve ne change pas de hauteur non plus.
    assert!(Layout::default_for(&real_monitors()).bars.iter().all(|b| b.height == BAND_HEIGHT && b.tiles.is_empty()));
}

#[test]
fn bar_height_is_kept_within_bounds() {
    let raw = |h: u32| {
        let v = json!({ "monitor_id": "m", "monitor_pos": [0, 0], "role": "portrait", "style": "graphic", "height": h });
        serde_json::from_value::<BarLayout>(v).unwrap().height
    };
    assert_eq!(raw(480), 480);
    assert_eq!(raw(3), BAND_HEIGHT);
    assert_eq!(raw(5000), MAX_BAR_HEIGHT);
    let mut l = Layout::default_for(&real_monitors());
    let set = |l: &mut Layout, h| {
        l.set_bar_options("right", BarOptionsPatch { height: Some(h), tiles_mode: Some(TilesMode::Fixed), ..Default::default() })
    };
    set(&mut l, 480).unwrap();
    assert_eq!(bar(&l, "right").height, 480);
    assert!(bar(&l, "right").has_tiles_area());
    set(&mut l, 10_000).unwrap();
    assert_eq!(bar(&l, "right").height, MAX_BAR_HEIGHT);
    set(&mut l, 0).unwrap();
    assert_eq!(bar(&l, "right").height, BAND_HEIGHT);
    assert_eq!(bar(&l, "right").style, BarStyle::Graphic, "le reste de la barre ne bouge pas");
}

/// Disposition réelle dont la barre de droite fait 480 px, tuiles sous la barre ; les autres
/// barres aussi sous la barre (à la hauteur de la bande).
fn tall_right() -> Layout {
    let mut l = Layout::default_for(&real_monitors());
    for b in &mut l.bars {
        b.tiles_mode = TilesMode::Fixed;
    }
    l.set_bar_options("right", BarOptionsPatch { height: Some(480), ..Default::default() }).unwrap();
    l
}

#[test]
fn tiles_zone_add_move_find_and_remove() {
    let mut l = tall_right();
    let weather = l.add_widget("right", Zone::Tiles, WidgetKind::WEATHER).unwrap();
    assert_eq!(kinds(&bar(&l, "right").tiles), vec![WidgetKind::WEATHER]);
    assert_eq!(l.find(&weather).map(|(_, z, i)| (z, i)), Some((Zone::Tiles, 0)));
    assert!(l.widget(&weather).is_some(), "les tuiles comptent parmi les widgets");
    // De la bande du haut aux tuiles, en tête…
    let system = bar(&l, "right").right.iter().find(|w| w.kind == WidgetKind::SYSTEM).unwrap().id.clone();
    l.move_widget(&system, "right", Zone::Tiles, 0).unwrap();
    assert_eq!(kinds(&bar(&l, "right").tiles), vec![WidgetKind::SYSTEM, WidgetKind::WEATHER]);
    assert!(bar(&l, "right").right.iter().all(|w| w.id != system));
    // … réordonnées, puis rendues à la bande.
    l.move_widget(&system, "right", Zone::Tiles, 9).unwrap();
    assert_eq!(kinds(&bar(&l, "right").tiles), vec![WidgetKind::WEATHER, WidgetKind::SYSTEM]);
    l.move_widget(&weather, "right", Zone::Left, 0).unwrap();
    assert_eq!(bar(&l, "right").left[0].id, weather);
    // Réglages et retrait d'une tuile.
    l.set_widget_settings(&system, json!({ "x": 1 })).unwrap();
    assert_eq!(l.widget(&system).unwrap().settings, json!({ "x": 1 }));
    assert_eq!(l.remove_widget(&system).unwrap().kind, WidgetKind::SYSTEM);
    assert!(bar(&l, "right").tiles.is_empty());
}

#[test]
fn tiles_are_hidden_but_kept_on_a_normal_bar() {
    let mut l = tall_right();
    l.add_widget("right", Zone::Tiles, WidgetKind::MEDIA).unwrap();
    assert_eq!(bar(&l, "right").tiles_shown(false).len(), 1);
    l.set_bar_options("right", BarOptionsPatch { height: Some(BAND_HEIGHT), ..Default::default() }).unwrap();
    assert!(bar(&l, "right").tiles_shown(false).is_empty());
    assert_eq!(bar(&l, "right").tiles.len(), 1, "gardée pour quand la barre regrandit");
}

#[test]
fn tile_sent_to_another_screen() {
    let mut l = tall_right();
    let id = l.add_widget("right", Zone::Tiles, WidgetKind::WEATHER).unwrap();
    // Barre normale : au bout de la zone droite, visible.
    l.move_to_monitor(&id, "main").unwrap();
    let main = bar(&l, "main");
    assert_eq!(main.right.last().unwrap().id, id);
    assert!(main.tiles.is_empty());
    // Barre haute : reste une tuile.
    l.move_widget(&id, "right", Zone::Tiles, 0).unwrap();
    l.set_bar_options("main", BarOptionsPatch { height: Some(240), ..Default::default() }).unwrap();
    l.move_to_monitor(&id, "main").unwrap();
    assert_eq!(bar(&l, "main").tiles[0].id, id);
    // Volet sans tuile : la tuile y reste une tuile.
    l.set_bar_options("left", BarOptionsPatch { tiles_mode: Some(TilesMode::Click), ..Default::default() }).unwrap();
    l.move_to_monitor(&id, "left").unwrap();
    assert_eq!(bar(&l, "left").tiles[0].id, id);
}

#[test]
fn undo_profiles_and_kind_removal_include_tiles() {
    let mut l = tall_right();
    let mut undo = UndoStack::default();
    undo.push(l.clone(), None);
    l.add_widget("right", Zone::Tiles, WidgetKind::DISCORD_VOICE).unwrap();
    let with_tile = l.clone();
    l = undo.undo(l).unwrap();
    assert!(bar(&l, "right").tiles.is_empty(), "annulé");
    l = undo.redo(l).unwrap();
    assert_eq!(l, with_tile, "rétabli");
    // Chaque profil garde sa hauteur et ses tuiles.
    let jeu = l.add_profile("Jeu", true).unwrap();
    l.set_bar_options("right", BarOptionsPatch { height: Some(BAND_HEIGHT), ..Default::default() }).unwrap();
    bar_mut(&mut l, "right").tiles.clear();
    l.switch_profile(DEFAULT_PROFILE, false).unwrap();
    assert_eq!((bar(&l, "right").height, bar(&l, "right").tiles.len()), (480, 1));
    l.switch_profile(&jeu, false).unwrap();
    assert_eq!((bar(&l, "right").height, bar(&l, "right").tiles.len()), (BAND_HEIGHT, 0));
}

#[test]
fn tiles_mode_absent_reads_as_fixed() {
    let old = json!({ "monitor_id": "m", "monitor_pos": [0, 0], "role": "portrait", "style": "graphic",
        "height": 480, "left": [], "center": [], "right": [] });
    let b: BarLayout = serde_json::from_value(old).unwrap();
    assert_eq!(b.tiles_mode, TilesMode::Fixed);
    assert_eq!(b.reserved_height(), 480);
    let v = serde_json::to_value(&b).unwrap();
    assert_eq!(v["tiles_mode"], json!("fixed"));
}

#[test]
fn drawer_reserves_only_the_band() {
    let mut l = Layout::default_for(&real_monitors());
    l.set_bar_options("right", BarOptionsPatch { height: Some(480), tiles_mode: Some(TilesMode::Fixed), ..Default::default() })
        .unwrap();
    assert_eq!(bar(&l, "right").reserved_height(), 480);
    for mode in [TilesMode::Click, TilesMode::Hover] {
        l.set_bar_options("right", BarOptionsPatch { tiles_mode: Some(mode), ..Default::default() }).unwrap();
        let b = bar(&l, "right");
        assert_eq!(b.tiles_mode, mode);
        assert_eq!(b.reserved_height(), BAND_HEIGHT);
        assert_eq!(b.height, 480, "hauteur gardée pour un retour à « Sous la barre »");
    }
}

#[test]
fn drawer_has_tiles_area_iff_tiles() {
    let mut b = bar(&Layout::default_for(&real_monitors()), "right").clone();
    b.tiles_mode = TilesMode::Click;
    b.height = 480;
    assert!(!b.has_tiles_area());
    assert!(b.tiles_shown(true).is_empty());
    b.tiles.push(widget(WidgetKind::CLOCK, serde_json::Value::Null));
    assert!(b.has_tiles_area());
    assert_eq!(b.tiles_shown(true).len(), 1);
    b.tiles_mode = TilesMode::Fixed;
    b.height = BAND_HEIGHT;
    assert!(!b.has_tiles_area(), "« Sous la barre » à la hauteur de la bande : pas de zone");
}

#[test]
fn drawer_height_is_bounded_by_screen() {
    assert_eq!(drawer_height(300.0, 1920.0), BAND_HEIGHT as f32 + 300.0);
    assert_eq!(drawer_height(5000.0, 1920.0), 1920.0 - DRAWER_MARGIN as f32);
    assert_eq!(drawer_height(0.0, 1920.0), BAND_HEIGHT as f32);
    assert_eq!(drawer_height(100.0, 60.0), BAND_HEIGHT as f32, "jamais moins que la bande");
}

#[test]
fn new_bars_default_to_click_drawer() {
    let l = Layout::default_for(&real_monitors());
    assert!(l.bars.iter().all(|b| b.tiles_mode == TilesMode::Click));
    assert!(TilesMode::Click.is_drawer() && TilesMode::Hover.is_drawer() && !TilesMode::Fixed.is_drawer());
}

#[test]
fn drawer_tiles_are_shown_only_while_open() {
    let mut b = bar(&Layout::default_for(&real_monitors()), "right").clone();
    b.tiles.push(widget(WidgetKind::SYSTEM, serde_json::Value::Null));
    b.tiles_mode = TilesMode::Click;
    assert!(b.tiles_shown(false).is_empty(), "volet fermé : rien ne tourne pour ses tuiles");
    assert_eq!(b.tiles_shown(true).len(), 1);
    b.tiles_mode = TilesMode::Fixed;
    b.height = 480;
    assert_eq!(b.tiles_shown(false).len(), 1, "sous la barre : toujours montrées");
    b.height = BAND_HEIGHT;
    assert!(b.tiles_shown(true).is_empty(), "barre revenue à la bande : gardées, pas montrées");
}
