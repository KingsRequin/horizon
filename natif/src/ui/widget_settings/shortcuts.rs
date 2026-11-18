//! Réglages Raccourcis (`ui/widget-settings/shortcuts.slint`,
//! app/src/widgets/shortcuts/Settings.svelte) : liste (`settings.items`, gardée telle quelle :
//! un raccourci illisible n'est pas perdu), choix, glisser-déposer, ajout, retrait annulable ;
//! détails du raccourci choisi : nom, cible (boîte « Ouvrir »), icône (logo Simple Icons cherché
//! par nom 120 ms après la frappe, `brands::search`, et sa couleur ; image choisie ; icône de
//! l'appli). Les icônes sont celles de la barre (`widgets::shortcuts::look`).

use crate::brands;
use crate::ui::dialogs::{self, choose_color, pick_file};
use crate::ui::settings_window::offer_undo;
use crate::ui::{BrandChoice, SettingsWindow, ShortcutRow, ShortcutThumb, ShortcutsSettingsData};
use crate::undoable::Removal;
use crate::widgets::shortcuts::look;
use horizon_core::shortcuts::{Shortcut, ShortcutIcon};
use serde_json::{Value, json};
use slint::{ComponentHandle, Image, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const SEARCH_DELAY: Duration = Duration::from_millis(120);
/// Logos montrés au plus.
const BRANDS_MAX: usize = 48;
/// Nom d'un raccourci tout juste ajouté (remplacé par celui de la cible choisie).
const NEW_LABEL: &str = "Nouveau raccourci";
/// Couleur des icônes par défaut (`--icon-default`), point de départ du sélecteur.
const ICON_DEFAULT: [u8; 3] = [0xcd, 0xd6, 0xf4];

thread_local! {
    static SELECTED: Cell<usize> = const { Cell::new(0) };
    static QUERY: RefCell<String> = const { RefCell::new(String::new()) };
    static RESULTS: RefCell<Vec<brands::Brand>> = const { RefCell::new(Vec::new()) };
    static TIMER: Rc<slint::Timer> = Rc::default();
    /// Numéro de l'ouverture : une icône arrivée pour une page fermée ne la redessine pas.
    static OPENING: Cell<u64> = const { Cell::new(0) };
}

/// Raccourcis tels qu'enregistrés.
fn items() -> Vec<Value> {
    super::settings().get("items").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn icon_kind(v: &Value) -> String {
    v.get("icon").and_then(|i| i.get("kind")).and_then(Value::as_str).unwrap_or("exe").to_owned()
}

/// Rang choisi après le retrait du rang `removed` (lib/selection.ts, `afterRemoval`).
pub fn after_removal(selected: usize, removed: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let next = if removed < selected { selected - 1 } else { selected };
    next.min(len - 1)
}

/// Liste réordonnée : l'élément `from` va au rang `to`.
pub fn moved<T>(mut list: Vec<T>, from: usize, to: usize) -> Vec<T> {
    if from < list.len() && to < list.len() && from != to {
        let x = list.remove(from);
        list.insert(to, x);
    }
    list
}

/// Nom proposé pour une cible choisie : le fichier sans `.lnk`, `.exe` ni `.url`.
pub fn name_of_target(path: &str) -> String {
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    let lower = file.to_ascii_lowercase();
    [".lnk", ".exe", ".url"].iter().find(|e| lower.ends_with(*e)).map_or(file, |e| &file[..file.len() - e.len()]).to_owned()
}

/// Icône d'un raccourci pour la page (case vide tant qu'elle n'est pas prête ; `ready` la
/// redessine).
fn thumb(s: &Shortcut) -> ShortcutThumb {
    if matches!(s.icon, ShortcutIcon::Exe) && s.target.trim().is_empty() {
        return ShortcutThumb { image: Image::default(), tint_mode: 0, tint: Default::default(), blank: true };
    }
    let opening = OPENING.get();
    match look(s, move |_| {
        if OPENING.get() == opening {
            refresh();
        }
    }) {
        Some((image, tint_mode, tint)) => ShortcutThumb { image, tint_mode, tint, blank: false },
        None => ShortcutThumb { image: Image::default(), tint_mode: 0, tint: Default::default(), blank: true },
    }
}

fn parse(v: &Value) -> Option<Shortcut> {
    serde_json::from_value(v.clone()).ok()
}

fn refresh() {
    let Some(w) = super::window() else { return };
    let g = w.global::<ShortcutsSettingsData>();
    let list = items();
    let rows: Vec<ShortcutRow> = list
        .iter()
        .map(|v| ShortcutRow {
            label: text(v, "label").into(),
            target: text(v, "target").into(),
            thumb: parse(v).map(|s| thumb(&s)).unwrap_or(ShortcutThumb { image: Image::default(), tint_mode: 0, tint: Default::default(), blank: true }),
        })
        .collect();
    crate::ui::model::sync_rc(&g.get_rows(), rows, |m| g.set_rows(m));
    let selected = SELECTED.get();
    g.set_selected(selected as i32);
    let Some(current) = list.get(selected) else { return };
    g.set_label(text(current, "label").into());
    g.set_target(text(current, "target").into());
    let kind = icon_kind(current);
    g.set_icon_path(current.get("icon").and_then(|i| i.get("path")).and_then(Value::as_str).unwrap_or_default().into());
    g.set_current_slug(if kind == "simple" { current["icon"]["slug"].as_str().unwrap_or_default() } else { "" }.into());
    g.set_icon_kind(kind.into());
    let color = current.get("color").and_then(Value::as_str).filter(|c| !c.is_empty());
    g.set_has_color(color.is_some());
    let [r, gr, b] = ICON_DEFAULT;
    g.set_color(color.and_then(brands::parse_color).unwrap_or(slint::Color::from_rgb_u8(r, gr, b)));
    g.set_query(QUERY.with_borrow(|q| q.as_str().into()));
}

/// Logos trouvés, montrés avec leur couleur de marque (sans la couleur choisie).
fn show_brands() {
    let Some(w) = super::window() else { return };
    let choices: Vec<BrandChoice> = RESULTS.with_borrow(|r| {
        r.iter()
            .map(|b| {
                let s = Shortcut { label: String::new(), target: String::new(), icon: ShortcutIcon::Simple { slug: b.slug.to_owned() }, color: None };
                BrandChoice { slug: b.slug.into(), title: b.title.into(), thumb: thumb(&s) }
            })
            .collect()
    });
    w.global::<ShortcutsSettingsData>().set_brands(ModelRc::new(VecModel::from(choices)));
}

fn save(list: Vec<Value>) {
    super::save(json!({ "items": list }));
    refresh();
}

/// Modifie le raccourci choisi.
fn update(f: impl FnOnce(&mut serde_json::Map<String, Value>)) {
    let mut list = items();
    if let Some(Value::Object(m)) = list.get_mut(SELECTED.get()) {
        f(m);
        save(list);
    }
}

fn set_icon(icon: Value) {
    update(|m| {
        m.insert("icon".into(), icon);
    });
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<ShortcutsSettingsData>();
    g.on_select(|i| {
        SELECTED.set(i.max(0) as usize);
        refresh();
    });
    g.on_move(|from, to| {
        let to = to.max(0) as usize;
        SELECTED.set(to);
        save(moved(items(), from.max(0) as usize, to));
    });
    g.on_add(|| {
        let mut list = items();
        list.push(json!({ "label": NEW_LABEL, "target": "", "icon": { "kind": "exe" } }));
        SELECTED.set(list.len() - 1);
        save(list);
    });
    g.on_remove(|i| remove(i.max(0) as usize));
    g.on_set_label(|t| {
        update(|m| {
            m.insert("label".into(), json!(t.as_str()));
        })
    });
    g.on_set_target(|t| {
        update(|m| {
            m.insert("target".into(), json!(t.as_str()));
        })
    });
    g.on_browse_target(|| browse(false));
    g.on_browse_image(|| browse(true));
    g.on_icon(|kind| match kind.as_str() {
        "simple" => {
            let slug = RESULTS.with_borrow(|r| r.first().map(|b| b.slug)).unwrap_or("youtube");
            set_icon(json!({ "kind": "simple", "slug": slug }));
        }
        _ => set_icon(json!({ "kind": "exe" })),
    });
    g.on_search(|q| search(q.to_string()));
    g.on_brand(|slug| set_icon(json!({ "kind": "simple", "slug": slug.as_str() })));
    g.on_pick_color(pick_color);
    g.on_brand_color(|| {
        update(|m| {
            m.insert("color".into(), Value::Null);
        })
    });
}

pub fn show(window: &SettingsWindow, _settings: &Value) {
    OPENING.set(OPENING.get() + 1);
    SELECTED.set(0);
    QUERY.set(String::new());
    RESULTS.set(Vec::new());
    window.global::<ShortcutsSettingsData>().set_rows(ModelRc::new(VecModel::default()));
    refresh();
    show_brands();
}

pub fn leave() {
    TIMER.with(|t| t.stop());
    OPENING.set(OPENING.get() + 1);
}

/// Retrait annulable : le raccourci revient à sa place.
fn remove(i: usize) {
    let mut list = items();
    if i >= list.len() {
        return;
    }
    let removed = list.remove(i);
    SELECTED.set(after_removal(SELECTED.get(), i, list.len()));
    save(list);
    offer_undo(Removal {
        label: format!("Raccourci « {} » retiré", text(&removed, "label")),
        undo: Box::new(move || {
            let mut back = items();
            back.insert(i.min(back.len()), removed);
            SELECTED.set(i);
            save(back);
        }),
        commit: None,
    });
}

/// Recherche de logos après le délai (vide : aucun).
fn search(q: String) {
    QUERY.set(q.clone());
    TIMER.with(|t| {
        t.start(slint::TimerMode::SingleShot, SEARCH_DELAY, move || {
            RESULTS.set(brands::search(&q, BRANDS_MAX));
            show_brands();
        });
    });
}

/// Boîte « Ouvrir », hors du rappel en cours : une cible (son nom remplace « Nouveau
/// raccourci ») ou une image pour l'icône.
fn browse(images: bool) {
    let Some(hwnd) = super::hwnd() else { return };
    let opening = OPENING.get();
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(path) = pick_file(hwnd, images) else { return };
        if OPENING.get() != opening {
            return;
        }
        if images {
            set_icon(json!({ "kind": "file", "path": path }));
        } else {
            update(|m| {
                if m.get("label").and_then(Value::as_str) == Some(NEW_LABEL) {
                    m.insert("label".into(), json!(name_of_target(&path)));
                }
                m.insert("target".into(), json!(path));
            });
        }
    });
}

/// Sélecteur de couleurs de Windows, depuis la couleur choisie (sinon celle des icônes).
fn pick_color() {
    let Some(hwnd) = super::hwnd() else { return };
    let current = items().get(SELECTED.get()).and_then(|v| v.get("color")).and_then(Value::as_str).and_then(brands::parse_color);
    let initial = current.map_or(ICON_DEFAULT, |c| [c.red(), c.green(), c.blue()]);
    let opening = OPENING.get();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(rgb) = choose_color(hwnd, initial)
            && OPENING.get() == opening
        {
            update(|m| {
                m.insert("color".into(), json!(dialogs::hex(rgb)));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_after_a_removal() {
        assert_eq!(after_removal(2, 0, 3), 1, "retrait avant : recule");
        assert_eq!(after_removal(1, 2, 3), 1);
        assert_eq!(after_removal(2, 2, 2), 1, "dernier retiré : le précédent");
        assert_eq!(after_removal(0, 0, 0), 0);
    }

    #[test]
    fn reorder_and_names() {
        assert_eq!(moved(vec![1, 2, 3], 0, 2), [2, 3, 1]);
        assert_eq!(moved(vec![1, 2, 3], 2, 0), [3, 1, 2]);
        assert_eq!(moved(vec![1, 2, 3], 5, 0), [1, 2, 3]);
        assert_eq!(name_of_target(r"C:\Users\x\Steam.lnk"), "Steam");
        assert_eq!(name_of_target(r"C:\Jeux\jeu.EXE"), "jeu");
        assert_eq!(name_of_target("D:/docs/notes.txt"), "notes.txt");
    }
}
