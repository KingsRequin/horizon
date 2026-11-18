//! Raccourcis par défaut du widget « Raccourcis », selon le rôle de l'écran.
//!
//! Réglages du widget : `{ "items": [Shortcut, ...] }`.

use crate::layout::BarRole;
use crate::apps::{app_key, launcher_app_key};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};

/// Icône d'un raccourci : logo Simple Icons, image choisie, ou icône de l'exécutable/raccourci.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShortcutIcon {
    Simple { slug: String },
    File { path: String },
    Exe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shortcut {
    pub label: String,
    /// Chemin (exe, .lnk, dossier), URL ou commande, ouvert par le shell Windows.
    pub target: String,
    pub icon: ShortcutIcon,
    /// Couleur de l'icône (logos) ; `None` = couleur de la marque.
    #[serde(default)]
    pub color: Option<String>,
}

fn simple(label: &str, target: &str, slug: &str) -> Shortcut {
    Shortcut {
        label: label.into(),
        target: target.into(),
        icon: ShortcutIcon::Simple { slug: slug.into() },
        color: None,
    }
}

fn exe(label: &str, target: String) -> Shortcut {
    Shortcut { label: label.into(), target, icon: ShortcutIcon::Exe, color: None }
}

/// Réglages de départ. `find_app(nom)` renvoie le raccourci du menu Démarrer d'une appli
/// installée ; une appli introuvable est simplement omise.
pub fn default_shortcuts(role: BarRole, find_app: impl Fn(&str) -> Option<String>) -> Value {
    let youtube = || Some(simple("YouTube", "https://www.youtube.com", "youtube"));
    let twitch = || Some(simple("Twitch", "https://www.twitch.tv", "twitch"));
    let discord = || find_app("Discord").map(|t| simple("Discord", &t, "discord"));
    let explorer = || Some(exe("Explorateur", r"C:\Windows\explorer.exe".into()));
    let list: Vec<Option<Shortcut>> = match role {
        BarRole::Main => vec![
            youtube(),
            discord(),
            twitch(),
            find_app("Apex Legends").map(|t| exe("Apex Legends", t)),
            find_app("Pear Desktop").map(|t| exe("Pear Desktop", t)),
            Some(simple("GitHub", "https://github.com", "github")),
            explorer(),
        ],
        BarRole::Portrait => vec![
            discord(),
            find_app("Terminal").map(|t| exe("Terminal", t)),
            explorer(),
        ],
        BarRole::Side => vec![youtube(), twitch()],
        BarRole::Top => vec![],
    };
    let items: Vec<Shortcut> = list.into_iter().flatten().collect();
    serde_json::json!({ "items": items })
}

/// Clé d'appli (`apps::app_key`) lancée par une cible de raccourci : AUMID d'un
/// `shell:AppsFolder\…`, exe d'un `.lnk` (lu par `lnk` : cible et arguments ; un lanceur
/// Squirrel désigne l'exe qu'il démarre), ou exe direct. `None` pour une URL, un dossier, un
/// `.url` (jeu Steam) : rien à apparier avec une fenêtre.
pub fn target_app_key(target: &str, lnk: impl Fn(&str) -> Option<(String, String)>) -> Option<String> {
    if let Some(aumid) = target.strip_prefix(r"shell:AppsFolder\") {
        return Some(aumid.to_owned());
    }
    let lower = target.to_ascii_lowercase();
    if lower.ends_with(".lnk") {
        let (t, a) = lnk(target)?;
        return t.to_ascii_lowercase().ends_with(".exe").then(|| launcher_app_key(&t, &a));
    }
    lower.ends_with(".exe").then(|| app_key(target))
}

/// Cibles dont l'appli a une fenêtre principale ouverte. `keys` : (cible, clé d'appli de la
/// cible) ; `running` : clés d'appli des fenêtres ouvertes.
pub fn running_targets<'a>(
    keys: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    running: &HashSet<String>,
) -> BTreeSet<String> {
    keys.into_iter()
        .filter(|(_, k)| k.is_some_and(|k| running.contains(k)))
        .map(|(t, _)| t.to_owned())
        .collect()
}

/// Cibles de tous les raccourcis d'une disposition (sans doublon).
pub fn all_targets(layout: &crate::layout::Layout) -> BTreeSet<String> {
    layout
        .widgets_of(crate::layout::WidgetKind::SHORTCUTS)
        .filter_map(|w| serde_json::from_value::<Vec<Shortcut>>(w.settings.get("items")?.clone()).ok())
        .flatten()
        .map(|s| s.target)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Raccourcis du menu Démarrer factices : cible et arguments.
    fn lnk(path: &str) -> Option<(String, String)> {
        match path {
            r"C:\Start\Discord.lnk" => Some((
                r"C:\Users\u\AppData\Local\Discord\Update.exe".into(),
                r#"--processStart "Discord.exe""#.into(),
            )),
            r"C:\Start\Pear.LNK" => Some((r"C:\Program Files\Pear Desktop\Pear Desktop.exe".into(), String::new())),
            r"C:\Start\Docs.lnk" => Some((r"C:\Users\u\Documents".into(), String::new())),
            _ => None,
        }
    }

    #[test]
    fn target_keys_match_window_keys() {
        // Fenêtre de Discord : exe dans un dossier de version Squirrel.
        let discord_window = app_key(r"C:\Users\u\AppData\Local\Discord\app-1.0.9\Discord.exe");
        assert_eq!(target_app_key(r"C:\Start\Discord.lnk", lnk), Some(discord_window));
        let pear_window = app_key(r"C:\PROGRAM FILES\PEAR DESKTOP\pear desktop.exe");
        assert_eq!(target_app_key(r"C:\Start\Pear.LNK", lnk), Some(pear_window));
        let terminal = "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App";
        assert_eq!(target_app_key(&format!(r"shell:AppsFolder\{terminal}"), lnk).as_deref(), Some(terminal));
        assert_eq!(
            target_app_key(r"C:\Windows\explorer.exe", lnk),
            Some(app_key(r"C:\WINDOWS\Explorer.EXE"))
        );
    }

    #[test]
    fn non_app_targets_have_no_key() {
        for t in [
            "https://www.youtube.com",
            r"C:\Users\u\Documents",
            r"C:\Start\Docs.lnk",
            r"C:\Start\Inconnu.lnk",
            r"C:\Users\u\Desktop\Apex Legends.url",
        ] {
            assert_eq!(target_app_key(t, lnk), None, "{t}");
        }
    }

    #[test]
    fn running_targets_follow_open_windows() {
        let keys = [
            ("discord.lnk", Some(r"discord\discord.exe")),
            ("https://github.com", None),
            ("pear.lnk", Some(r"pear desktop\pear desktop.exe")),
            ("discord-bis.lnk", Some(r"discord\discord.exe")),
        ];
        let running: HashSet<String> = [r"discord\discord.exe".to_owned(), "autre.exe".to_owned()].into();
        let got: Vec<String> = running_targets(keys.iter().map(|(t, k)| (*t, *k)), &running).into_iter().collect();
        assert_eq!(got, ["discord-bis.lnk", "discord.lnk"]);
        assert!(running_targets(keys.iter().map(|(t, k)| (*t, *k)), &HashSet::new()).is_empty());
    }

    #[test]
    fn all_targets_of_a_layout() {
        use crate::layout::{Layout, WidgetKind, Zone};
        let mut l = Layout::default_for(&[crate::layout::tests::mon("m", 0, 0, 1920, 1080, true)]);
        let id = l.add_widget("m", Zone::Left, WidgetKind::SHORTCUTS).unwrap();
        l.set_widget_settings(
            &id,
            serde_json::json!({ "items": [
                { "label": "a", "target": "a.exe", "icon": { "kind": "exe" } },
                { "label": "b", "target": "https://b.fr", "icon": { "kind": "exe" } },
            ] }),
        )
        .unwrap();
        let t = all_targets(&l);
        assert!(t.contains("a.exe") && t.contains("https://b.fr"));
    }

    fn finder(name: &str) -> Option<String> {
        match name {
            "Discord" => Some(r"C:\Start\Discord.lnk".into()),
            "Apex Legends" => Some(r"C:\Start\Apex Legends.lnk".into()),
            "Terminal" => Some(r"shell:AppsFolder\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".into()),
            _ => None,
        }
    }

    fn items(v: Value) -> Vec<Shortcut> {
        serde_json::from_value(v["items"].clone()).unwrap()
    }

    #[test]
    fn main_screen_defaults_skip_missing_apps() {
        let list = items(default_shortcuts(BarRole::Main, finder));
        let labels: Vec<&str> = list.iter().map(|s| s.label.as_str()).collect();
        // Pear Desktop introuvable ici : omis.
        assert_eq!(labels, vec!["YouTube", "Discord", "Twitch", "Apex Legends", "GitHub", "Explorateur"]);
        assert_eq!(list[0].target, "https://www.youtube.com");
        assert_eq!(list[0].icon, ShortcutIcon::Simple { slug: "youtube".into() });
        assert_eq!(list[1].target, r"C:\Start\Discord.lnk");
        assert_eq!(list[1].icon, ShortcutIcon::Simple { slug: "discord".into() });
        assert_eq!(list[3].icon, ShortcutIcon::Exe);
        assert_eq!(list[5].target, r"C:\Windows\explorer.exe");
    }

    #[test]
    fn portrait_and_side_defaults() {
        let p: Vec<String> = items(default_shortcuts(BarRole::Portrait, finder))
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert_eq!(p, vec!["Discord", "Terminal", "Explorateur"]);
        let s: Vec<String> = items(default_shortcuts(BarRole::Side, finder))
            .into_iter()
            .map(|s| s.label)
            .collect();
        assert_eq!(s, vec!["YouTube", "Twitch"]);
    }

    #[test]
    fn top_screen_has_no_shortcuts() {
        assert!(items(default_shortcuts(BarRole::Top, finder)).is_empty());
    }

    #[test]
    fn icon_serialization_is_tagged() {
        let v = serde_json::to_value(simple("a", "b", "c")).unwrap();
        assert_eq!(v["icon"], serde_json::json!({ "kind": "simple", "slug": "c" }));
        let e: Shortcut = serde_json::from_value(serde_json::json!({
            "label": "x", "target": "y", "icon": { "kind": "exe" }
        }))
        .unwrap();
        assert_eq!(e.icon, ShortcutIcon::Exe);
        assert_eq!(e.color, None);
    }
}
