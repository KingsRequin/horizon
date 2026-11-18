//! Suggestion telle qu'affichée (nom lisible, raison) et éléments jamais suggérés.
//!
//! Repris tel quel de `app/src-tauri/src/widgets/suggestions/view.rs`.

use horizon_core::layout::{BarLayout, WidgetKind};
use horizon_core::shortcuts::Shortcut;
use horizon_core::suggest::ItemKey;
use horizon_core::suggest::keys::site_host;
use horizon_core::suggest::store::Store;
use std::collections::HashSet;

use super::SuggestionView;

/// Éléments déjà présents dans les raccourcis fixes d'une barre : jamais suggérés.
/// `app_key_of` donne la clé d'appli d'une cible (`.lnk`, `shell:AppsFolder\…`, exe).
pub fn fixed_keys(bar: &BarLayout, mut app_key_of: impl FnMut(&str) -> Option<String>) -> HashSet<ItemKey> {
    bar.widgets()
        .filter(|w| w.kind == WidgetKind::SHORTCUTS)
        .filter_map(|w| serde_json::from_value::<Vec<Shortcut>>(w.settings.get("items")?.clone()).ok())
        .flatten()
        .filter_map(|s| match site_host(&s.target) {
            Some(h) => Some(ItemKey::Site(h)),
            None => app_key_of(&s.target).map(ItemKey::App),
        })
        .collect()
}

pub(super) fn view(store: &Store, item: &ItemKey, reason: String) -> SuggestionView {
    let (kind, label) = match item {
        ItemKey::Site(h) => ("site", store.label(item).unwrap_or_else(|| h.clone())),
        ItemKey::App(k) => {
            let label = store.label(item).unwrap_or_else(|| {
                let raw = store.last_raw_app(k).unwrap_or_else(|| k.clone());
                let name = horizon_core::apps::display_name(&raw, store.launch_target(k).as_deref());
                let _ = store.set_label(item, &name);
                name
            });
            ("app", label)
        }
    };
    SuggestionView { key: item.as_str(), label, kind, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::layout::{BarRole, BarStyle, WidgetInstance};
    use serde_json::json;

    #[test]
    fn fixed_keys_cover_urls_lnk_and_aumid() {
        let shortcuts = WidgetInstance {
            id: "w".into(),
            kind: WidgetKind::SHORTCUTS,
            settings: json!({ "items": [
                { "label": "YT", "target": "https://www.youtube.com", "icon": { "kind": "exe" } },
                { "label": "D", "target": r"C:\Start\Discord.lnk", "icon": { "kind": "exe" } },
                { "label": "T", "target": r"shell:AppsFolder\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", "icon": { "kind": "exe" } },
                { "label": "Dossier", "target": r"C:\Users", "icon": { "kind": "exe" } },
            ] }),
        };
        let bar = BarLayout {
            monitor_id: "M".into(),
            monitor_pos: (0, 0),
            role: BarRole::Main,
            style: BarStyle::Islands,
            interaction: horizon_core::layout::Interaction::Click,
            autohide: true,
            height: horizon_core::layout::BAND_HEIGHT,
            left: vec![shortcuts],
            center: vec![],
            right: vec![],
            tiles: vec![],
        };
        let keys = fixed_keys(&bar, |t| match t {
            r"C:\Start\Discord.lnk" => Some(r"discord\discord.exe".into()),
            t if t.starts_with("shell:") => Some(t.trim_start_matches(r"shell:AppsFolder\").into()),
            _ => None,
        });
        let expected: HashSet<ItemKey> = [
            ItemKey::Site("youtube.com".into()),
            ItemKey::App(r"discord\discord.exe".into()),
            ItemKey::App("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".into()),
        ]
        .into();
        assert_eq!(keys, expected);
    }
}
