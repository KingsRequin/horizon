//! Fenêtres qui montrent les panneaux des widgets : la fenêtre surgissante (un panneau à la fois)
//! et les barres hautes (une tuile par widget de leur zone `tiles`, comme `Tile.svelte`). Chaque
//! fenêtre a son propre exemplaire des globaux Slint des panneaux (`<Id>PanelData`) : un panneau
//! écrit dans toutes celles qui le montrent ([`each`]), ou dans une seule ([`host`]) quand ce
//! qu'il montre dépend d'elle (exemplaire d'un widget, écran de la barre).
//!
//! Crochets des widgets (`widgets::Panel`), sur le fil de l'interface :
//! - `bind(app, host)` : une fois par fenêtre (la surgissante à sa création, une barre au premier
//!   affichage d'une tuile de ce genre) ;
//! - `opened(app, genre)` / `closed(app, genre)` : le panneau du genre commence à être montré
//!   quelque part / ne l'est plus nulle part (suivi du mixeur, minuteurs) ; une barre cachée
//!   (plein écran) ne compte pas ;
//! - `shown(app, host, genre)` / `hidden(app, host, genre)` : une fenêtre commence / cesse de
//!   montrer le panneau `genre` (« weather:w1 ») ; une tuile a toujours son exemplaire.

use super::{BarWindow, PanelWindow};
use crate::host::AppHandle;
use crate::popup::base_of;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;

enum Win {
    Popup(PanelWindow),
    Bar(BarWindow),
}

/// Une fenêtre qui montre des panneaux.
pub struct Host {
    pub label: String,
    win: Win,
}

impl Host {
    /// Global Slint d'un panneau dans cette fenêtre.
    pub fn global<'a, T>(&'a self) -> T
    where
        T: slint::Global<'a, PanelWindow> + slint::Global<'a, BarWindow>,
    {
        match &self.win {
            Win::Popup(w) => w.global::<T>(),
            Win::Bar(w) => w.global::<T>(),
        }
    }

    /// Style graphique de la fenêtre (panneaux des plugins : thème recopié).
    pub fn graphic(&self) -> bool {
        match &self.win {
            Win::Popup(w) => w.get_graphic(),
            Win::Bar(w) => w.get_graphic(),
        }
    }

    /// Échelle de la fenêtre.
    pub fn scale(&self) -> f32 {
        match &self.win {
            Win::Popup(w) => w.window().scale_factor(),
            Win::Bar(w) => w.window().scale_factor(),
        }
    }

    /// C'est la fenêtre surgissante (ses abonnements valent pour toutes les fenêtres).
    pub fn is_popup(&self) -> bool {
        matches!(self.win, Win::Popup(_))
    }

    /// Le panneau est montré en tuile, dans une barre haute : pas de fenêtre surgissante à
    /// fermer après une action (`inline` de la version Tauri).
    pub fn inline(&self) -> bool {
        !self.is_popup()
    }
}

enum WeakWin {
    Popup(slint::Weak<PanelWindow>),
    Bar(slint::Weak<BarWindow>),
}

struct Entry {
    label: String,
    win: WeakWin,
    /// Panneaux montrés (« media:w1 » pour une tuile, le genre demandé pour la surgissante).
    kinds: Vec<String>,
    /// Fenêtre affichée (barre visible, panneau ouvert).
    active: bool,
    /// Genres dont les rappels sont reliés dans cette fenêtre.
    bound: Vec<String>,
    /// Écran de la fenêtre (pixels physiques) : celui de la barre, ou du dernier panneau.
    screen: Option<horizon_core::geometry::Rect>,
}

impl Entry {
    fn upgrade(&self) -> Option<Host> {
        let win = match &self.win {
            WeakWin::Popup(w) => Win::Popup(w.upgrade()?),
            WeakWin::Bar(w) => Win::Bar(w.upgrade()?),
        };
        Some(Host { label: self.label.clone(), win })
    }

    fn pairs(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.kinds.iter().filter(|_| self.active).map(|k| (self.label.clone(), k.clone()))
    }
}

thread_local! {
    static HOSTS: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// Ce qui change quand l'ensemble des (fenêtre, panneau) montrés passe de `old` à `new` : les
/// panneaux cachés, les genres qui ne sont plus montrés nulle part, ceux qui le deviennent, les
/// panneaux montrés (dans l'ordre des appels des crochets).
#[derive(Debug, Default, PartialEq)]
struct Changes {
    hidden: Vec<(String, String)>,
    closed: Vec<String>,
    opened: Vec<String>,
    shown: Vec<(String, String)>,
}

fn bases(pairs: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<String> = pairs.iter().map(|(_, k)| base_of(k).to_owned()).collect();
    v.sort();
    v.dedup();
    v
}

fn changes(old: &[(String, String)], new: &[(String, String)]) -> Changes {
    let (before, after) = (bases(old), bases(new));
    Changes {
        hidden: old.iter().filter(|p| !new.contains(p)).cloned().collect(),
        closed: before.iter().filter(|b| !after.contains(b)).cloned().collect(),
        opened: after.iter().filter(|b| !before.contains(b)).cloned().collect(),
        shown: new.iter().filter(|p| !old.contains(p)).cloned().collect(),
    }
}

fn all_pairs() -> Vec<(String, String)> {
    HOSTS.with_borrow(|h| h.iter().flat_map(Entry::pairs).collect())
}

/// Applique `f` à la liste des fenêtres, puis appelle les crochets des panneaux pour ce qui a
/// changé (registre relâché : les crochets écrivent dans les fenêtres).
fn change(app: &AppHandle, f: impl FnOnce(&mut Vec<Entry>)) {
    let old = all_pairs();
    HOSTS.with_borrow_mut(f);
    let new = all_pairs();
    let c = changes(&old, &new);
    if !c.opened.is_empty() || !c.closed.is_empty() {
        use crate::host::Manager;
        app.state::<crate::popup::PopupState>().set_shown(bases(&new));
    }
    for (label, kind) in &c.hidden {
        if let Some(h) = host(label) {
            crate::widgets::panel_hook(kind, |p| p.hidden.map(|f| f(app, &h, kind)));
        }
    }
    for base in &c.closed {
        crate::widgets::panel_hook(base, |p| p.closed.map(|f| f(app, base)));
    }
    for base in &c.opened {
        crate::widgets::panel_hook(base, |p| p.opened.map(|f| f(app, base)));
    }
    for (label, kind) in &c.shown {
        if let Some(h) = host(label) {
            crate::widgets::panel_hook(kind, |p| p.shown.map(|f| f(app, &h, kind)));
        }
    }
}

/// Relie dans la fenêtre `label` les panneaux de `kinds` qui ne le sont pas encore.
fn bind_missing(app: &AppHandle, label: &str, kinds: &[String]) {
    let todo: Vec<String> = HOSTS.with_borrow_mut(|h| {
        let Some(e) = h.iter_mut().find(|e| e.label == label) else { return Vec::new() };
        let mut todo = Vec::new();
        for k in kinds {
            let b = base_of(k).to_owned();
            if !e.bound.contains(&b) && crate::widgets::has_panel(&b) {
                e.bound.push(b.clone());
                todo.push(b);
            }
        }
        todo
    });
    if todo.is_empty() {
        return;
    }
    let Some(h) = host(label) else { return };
    for b in todo {
        crate::widgets::panel_hook(&b, |p| {
            (p.bind)(app, &h);
            Some(())
        });
    }
}

/// La fenêtre surgissante vient d'être créée : tous les panneaux y sont reliés.
pub fn add_popup(app: &AppHandle, label: &str, window: &PanelWindow) {
    HOSTS.with_borrow_mut(|h| {
        h.push(Entry {
            label: label.to_owned(),
            win: WeakWin::Popup(window.as_weak()),
            kinds: Vec::new(),
            active: false,
            bound: Vec::new(),
            screen: None,
        })
    });
    let mut all: Vec<String> = crate::widgets::PANELS.iter().map(|s| (*s).to_owned()).collect();
    all.push(crate::plugins::PANEL_KIND.to_owned());
    bind_missing(app, label, &all);
}

/// Panneau ouvert dans la fenêtre surgissante (`None` : fermée), et son écran.
pub fn set_popup(app: &AppHandle, label: &str, kind: Option<&str>, screen: Option<horizon_core::geometry::Rect>) {
    change(app, |h| {
        if let Some(e) = h.iter_mut().find(|e| e.label == label) {
            e.kinds = kind.map(str::to_owned).into_iter().collect();
            e.active = kind.is_some();
            if screen.is_some() {
                e.screen = screen;
            }
        }
    });
}

/// Une barre haute (`label`) montre ces tuiles (« genre:identifiant », dans l'ordre) ; elle est
/// ajoutée à la liste à son premier appel, cachée tant que [`set_active`] ne l'a pas montrée.
pub fn set_tiles(app: &AppHandle, label: &str, window: &BarWindow, kinds: Vec<String>, screen: horizon_core::geometry::Rect) {
    HOSTS.with_borrow_mut(|h| {
        if !h.iter().any(|e| e.label == label) {
            h.push(Entry {
                label: label.to_owned(),
                win: WeakWin::Bar(window.as_weak()),
                kinds: Vec::new(),
                active: false,
                bound: Vec::new(),
                screen: Some(screen),
            });
        }
    });
    let todo = kinds.clone();
    change(app, |h| {
        if let Some(e) = h.iter_mut().find(|e| e.label == label) {
            e.kinds = kinds;
        }
    });
    // Rappels et valeurs de départ (écrites dans les fenêtres qui montrent le panneau : la
    // barre en fait déjà partie).
    bind_missing(app, label, &todo);
}

/// La barre `label` est affichée ou cachée (plein écran) : ses tuiles comptent ou non.
pub fn set_active(app: &AppHandle, label: &str, active: bool) {
    if HOSTS.with_borrow(|h| h.iter().any(|e| e.label == label && e.active != active)) {
        change(app, |h| {
            if let Some(e) = h.iter_mut().find(|e| e.label == label) {
                e.active = active;
            }
        });
    }
}

/// Fenêtre fermée : ses panneaux sont cachés, elle sort de la liste.
pub fn forget(app: &AppHandle, label: &str) {
    if !HOSTS.with_borrow(|h| h.iter().any(|e| e.label == label)) {
        return;
    }
    change(app, |h| {
        if let Some(e) = h.iter_mut().find(|e| e.label == label) {
            e.active = false;
        }
    });
    HOSTS.with_borrow_mut(|h| h.retain(|e| e.label != label));
}

/// La fenêtre `label`, si elle vit encore.
pub fn host(label: &str) -> Option<Host> {
    HOSTS.with_borrow(|h| h.iter().find(|e| e.label == label).and_then(Entry::upgrade))
}

/// Écran de la fenêtre `label` (pixels physiques) : celui de la barre, ou du dernier panneau
/// ouvert dans la fenêtre surgissante.
pub fn screen(label: &str) -> Option<horizon_core::geometry::Rect> {
    HOSTS.with_borrow(|h| h.iter().find(|e| e.label == label).and_then(|e| e.screen))
}

/// Chaque fenêtre qui peut montrer le panneau `base` : la surgissante (toujours tenue à jour,
/// pour s'ouvrir aussitôt) et les barres qui en ont une tuile, même cachées.
pub fn each(base: &str, mut f: impl FnMut(&Host)) {
    let hosts: Vec<Host> = HOSTS.with_borrow(|h| {
        h.iter()
            .filter(|e| matches!(e.win, WeakWin::Popup(_)) || e.kinds.iter().any(|k| base_of(k) == base))
            .filter_map(Entry::upgrade)
            .collect()
    });
    for h in &hosts {
        f(h);
    }
}

/// Fenêtres qui montrent en ce moment le panneau `kind` exactement (« weather:w1 »).
pub fn showing(kind: &str) -> Vec<Host> {
    HOSTS.with_borrow(|h| h.iter().filter(|e| e.active && e.kinds.iter().any(|k| k == kind)).filter_map(Entry::upgrade).collect())
}

/// Panneaux montrés, par fenêtre (journal, essais).
pub fn summary() -> HashMap<String, Vec<String>> {
    HOSTS.with_borrow(|h| h.iter().filter(|e| e.active).map(|e| (e.label.clone(), e.kinds.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(l: &str, k: &str) -> (String, String) {
        (l.into(), k.into())
    }

    #[test]
    fn a_kind_opens_with_its_first_window_and_closes_with_its_last() {
        // Une tuile Lecture en cours, puis le panneau surgissant du même genre : ouvert une fois.
        let tile = vec![p("bar-1-0", "media:w1")];
        let c = changes(&[], &tile);
        assert_eq!(c.opened, ["media"]);
        assert_eq!(c.shown, tile);
        let both = vec![p("bar-1-0", "media:w1"), p("popup", "media")];
        let c = changes(&tile, &both);
        assert!(c.opened.is_empty() && c.closed.is_empty());
        assert_eq!(c.shown, [p("popup", "media")]);
        // Panneau fermé : la tuile le garde ouvert.
        let c = changes(&both, &tile);
        assert!(c.closed.is_empty());
        assert_eq!(c.hidden, [p("popup", "media")]);
        // Barre cachée : plus rien.
        let c = changes(&tile, &[]);
        assert_eq!((c.hidden, c.closed), (tile, vec!["media".to_owned()]));
    }

    #[test]
    fn instances_of_a_kind_share_its_opening() {
        let old = vec![p("bar-1-0", "weather:a")];
        let new = vec![p("bar-1-0", "weather:a"), p("popup", "weather:b")];
        let c = changes(&old, &new);
        assert!(c.opened.is_empty());
        assert_eq!(c.shown, [p("popup", "weather:b")]);
        let c = changes(&new, &[p("popup", "weather:b")]);
        assert_eq!(c.hidden, [p("bar-1-0", "weather:a")]);
        assert!(c.closed.is_empty());
    }
}
