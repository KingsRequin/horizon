//! Widget « Raccourcis » : une rangée d'icônes (logo Simple Icons, image choisie ou icône de
//! l'exe / du raccourci), pastille sous celles dont l'appli est ouverte ; clic = l'appli ouverte
//! revient au premier plan, sinon elle est lancée ; Maj+clic = nouvelle instance. Sans raccourci,
//! le nom du widget ; un clic ouvre ses réglages.
//!
//! Repris de `app/src-tauri/src/widgets/shortcuts/` (service : `running.rs`) et de
//! `app/src/widgets/shortcuts/Widget.svelte` + `lib/ShortcutIcon.svelte` (rangée : `bind`).
//! Différences : les icônes vont à Slint en images (`ui::images`), plus d'URL `data:` ; la boîte
//! « Ouvrir » des réglages est celle de Windows (`ui::dialogs`).

pub mod running;

use super::Backend;
use crate::brands;
use crate::host::{AppHandle, Manager};
use crate::ui::images::{self, Pixels};
use crate::ui::{BarWindow, ShortcutItem, ShortcutsData};
use horizon_core::layout::{BarLayout, Layout, WidgetKind};
use horizon_core::shortcuts::{Shortcut, ShortcutIcon, default_shortcuts};
use serde_json::Value;
use slint::{ComponentHandle, Image, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::rc::{Rc, Weak};

pub const BACKEND: Backend = Backend {
    id: "shortcuts",
    name: "raccourcis",
    manage: |app| {
        app.manage(running::Running::default());
    },
    start: Some(running::start),
    bind: Some(bind),
    ..Backend::NONE
};

/// Remplit les réglages vides des widgets Raccourcis avec les raccourcis par défaut du rôle.
pub fn fill_defaults(layout: &mut Layout) {
    for bar in &mut layout.bars {
        let role = bar.role;
        for w in bar.widgets_mut() {
            if w.kind == WidgetKind::SHORTCUTS && w.settings.is_null() {
                w.settings = default_shortcuts(role, horizon_sys::launch::find_app);
            }
        }
    }
}

/// Réglages par défaut d'un widget ajouté : rien sauf pour les raccourcis (liste vide).
pub fn initial_settings(kind: WidgetKind) -> Value {
    match kind {
        WidgetKind::SHORTCUTS => serde_json::json!({ "items": [] }),
        _ => Value::Null,
    }
}

/// Raccourcis affichés d'un exemplaire : ceux qui se lisent, sans ceux qui n'ont pas de cible
/// (en cours de saisie dans les réglages).
pub fn items(settings: &Value) -> Vec<Shortcut> {
    settings
        .get("items")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| serde_json::from_value::<Shortcut>(v.clone()).ok())
                .filter(|s| !s.target.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Icône d'un exe, raccourci, dossier ou appli (48 px), décodée (fil des chargements).
pub fn exe_icon(target: &str) -> Option<Pixels> {
    let px = horizon_sys::exeicon::icon_png(target).and_then(|png| images::decode(&png)).map(Pixels::Rgba);
    if px.is_none() {
        log::warn!("pas d'icône pour {target}");
    }
    px
}

/// Image d'une icône d'exe : gardée, sinon demandée (`ready` à son arrivée).
pub fn exe_image(target: &str, ready: impl FnOnce(&Image) + 'static) -> Option<Image> {
    let t = target.to_owned();
    images::get(&format!("exe:{target}"), move || exe_icon(&t), ready)
}

// ---- Rangée d'une barre (fil de l'interface) ----

/// Teinte d'une icône (`ShortcutItem.tint-mode`).
const TINT_NONE: i32 = 0;
const TINT_COLOR: i32 = 1;
const TINT_TEXT: i32 = 2;

thread_local! {
    /// Logos déjà rendus (identifiant → image).
    static LOGOS: RefCell<HashMap<String, Option<Image>>> = RefCell::default();
}

fn logo(slug: &str) -> Option<(Image, &'static str)> {
    let b = brands::brand(slug)?;
    let img = LOGOS.with_borrow_mut(|l| l.entry(slug.to_owned()).or_insert_with(|| Some(brands::image(&b))).clone())?;
    Some((img, b.hex))
}

/// Ce que montre un raccourci : logo teinté, image, ou case vide en attendant (`ShortcutIcon`) ;
/// aussi les icônes de la palette (`ui::palette`).
pub(crate) fn look(s: &Shortcut, ready: impl FnOnce(&Image) + 'static) -> Option<(Image, i32, slint::Color)> {
    match &s.icon {
        ShortcutIcon::Simple { slug } => {
            let (img, hex) = logo(slug)?;
            // Couleur choisie, sinon celle de la marque si elle se voit sur fond sombre.
            let tint = s.color.as_deref().and_then(brands::parse_color).or_else(|| brands::readable_on_dark(hex));
            Some(match tint {
                Some(c) => (img, TINT_COLOR, c),
                None => (img, TINT_TEXT, slint::Color::default()),
            })
        }
        ShortcutIcon::Exe => exe_image(&s.target, ready).map(|i| (i, TINT_NONE, slint::Color::default())),
        ShortcutIcon::File { path } => {
            let p = path.clone();
            let fetch = move || {
                images::load_file(&p).map_err(|e| log::warn!("icône {p} : {e}")).ok()
            };
            images::get(&format!("file:{path}"), fetch, ready).map(|i| (i, TINT_NONE, slint::Color::default()))
        }
    }
}

/// Infobulle d'un raccourci.
pub fn title(label: &str, running: bool) -> String {
    if running { format!("{label} — ouvert (Maj+clic : nouvelle fenêtre)") } else { label.to_owned() }
}

/// Raccourcis d'une barre : leur liste par exemplaire et le modèle de chaque rangée.
struct BarShortcuts {
    items: RefCell<HashMap<String, Vec<Shortcut>>>,
    models: RefCell<HashMap<String, Rc<VecModel<ShortcutItem>>>>,
    running: RefCell<BTreeSet<String>>,
}

impl BarShortcuts {
    fn model(&self, instance: &str) -> Rc<VecModel<ShortcutItem>> {
        self.models.borrow_mut().entry(instance.to_owned()).or_default().clone()
    }

    /// Recalcule chaque rangée ; seules les lignes changées sont reposées.
    fn refresh(self: &Rc<Self>) {
        let items = self.items.borrow().clone();
        let running = self.running.borrow().clone();
        for (instance, list) in &items {
            let rows: Vec<ShortcutItem> = list
                .iter()
                .map(|s| {
                    let me = Rc::downgrade(self);
                    let on = running.contains(&s.target);
                    let (image, tint_mode, tint) = match look(s, move |_| refresh_weak(&me)) {
                        Some((i, m, c)) => (i, m, c),
                        None => (Image::default(), TINT_NONE, slint::Color::default()),
                    };
                    ShortcutItem { blank: image.size().width == 0, image, tint_mode, tint, running: on, title: title(&s.label, on).into() }
                })
                .collect();
            crate::ui::model::sync(&self.model(instance), rows);
        }
    }
}

fn refresh_weak(me: &Weak<BarShortcuts>) {
    if let Some(me) = me.upgrade() {
        me.refresh();
    }
}

fn read_items(bar: &BarLayout) -> HashMap<String, Vec<Shortcut>> {
    bar.widgets().filter(|w| w.kind == WidgetKind::SHORTCUTS).map(|w| (w.id.as_str().to_owned(), items(&w.settings))).collect()
}

/// Relie les widgets Raccourcis d'une barre (fil de l'interface).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let me = Rc::new(BarShortcuts {
        items: RefCell::new(read_items(bar)),
        models: RefCell::default(),
        running: RefCell::new(running::shortcuts_running(app)),
    });
    let g = window.global::<ShortcutsData>();
    let m = me.clone();
    g.on_items(move |instance| ModelRc::from(m.model(&instance)));
    let m = me.clone();
    g.on_open(move |instance, index, shift| {
        let target = m.items.borrow().get(instance.as_str()).and_then(|l| l.get(index as usize)).map(|s| s.target.clone());
        if let Some(t) = target {
            running::shortcuts_open(t, shift);
        }
    });
    me.refresh();
    let m = Rc::downgrade(&me);
    crate::host::listen(running::EV_RUNNING, Some(label), move |e| {
        if let (Some(set), Some(m)) = (e.payload::<BTreeSet<String>>(), m.upgrade())
            && *m.running.borrow() != *set
        {
            *m.running.borrow_mut() = set.clone();
            m.refresh();
        }
    });
    // Disposition modifiée (réglages, profil) : les raccourcis de cette barre sont relus.
    let (m, a, monitor) = (Rc::downgrade(&me), app.clone(), bar.monitor_id.clone());
    crate::host::listen(crate::events::EV_LAYOUT, Some(label), move |_| {
        let (Some(m), Some(bar)) = (m.upgrade(), a.state::<crate::state::AppState>().bar(&monitor)) else { return };
        *m.items.borrow_mut() = read_items(&bar);
        m.refresh();
    });
    // Les modèles vivent tant que la fenêtre garde ses rappels.
    drop(me);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn items_skip_unreadable_and_targetless_shortcuts() {
        let v = json!({ "items": [
            { "label": "YouTube", "target": "https://www.youtube.com", "icon": { "kind": "simple", "slug": "youtube" } },
            { "label": "Vide", "target": "  ", "icon": { "kind": "exe" } },
            { "label": "Cassé", "icon": { "kind": "exe" } },
            { "label": "Image", "target": "C:\\x.exe", "icon": { "kind": "file", "path": "C:\\x.png" }, "color": "#ff0000" },
        ] });
        let list = items(&v);
        assert_eq!(list.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(), ["YouTube", "Image"]);
        assert!(items(&Value::Null).is_empty());
        assert!(items(&json!({ "items": 3 })).is_empty());
    }

    #[test]
    fn titles() {
        assert_eq!(title("Discord", false), "Discord");
        assert_eq!(title("Discord", true), "Discord — ouvert (Maj+clic : nouvelle fenêtre)");
    }

    #[test]
    fn logos_are_tinted_like_shortcut_icon() {
        let s = |slug: &str, color: Option<&str>| Shortcut {
            label: "x".into(),
            target: "x".into(),
            icon: ShortcutIcon::Simple { slug: slug.into() },
            color: color.map(str::to_owned),
        };
        let (_, mode, c) = look(&s("youtube", None), |_| {}).unwrap();
        assert_eq!((mode, c), (TINT_COLOR, slint::Color::from_rgb_u8(0xff, 0, 0)));
        let (_, mode, _) = look(&s("github", None), |_| {}).unwrap();
        assert_eq!(mode, TINT_TEXT, "logo trop sombre : couleur du texte");
        let (_, mode, c) = look(&s("github", Some("#00ff00")), |_| {}).unwrap();
        assert_eq!((mode, c), (TINT_COLOR, slint::Color::from_rgb_u8(0, 0xff, 0)));
        assert!(look(&s("inexistant_zz", None), |_| {}).is_none(), "case vide");
    }
}
