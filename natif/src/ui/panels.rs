//! Accroche des panneaux surgissants et des réglages des widgets, appelée par les
//! widgets (global `Panels` et `PanelArea` de `ui/panels.slint`) ou par leur liaison Rust (`pin`,
//! `settings`). Règle de lib/panels.ts : aperçu au survol après `HOVER_DELAY` (aussitôt si un
//! panneau voisin est ouvert), fermeture peu après la sortie, épinglé au clic ; panneau propre à
//! un exemplaire : « genre:identifiant ». La fenêtre est menée par `ui::popup`.

use super::{BarWindow, Panels, popup};
use crate::host::AppHandle;
use crate::settings_window::{self, SettingsPage};
use crate::popup::HOVER_DELAY;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

/// Ancre d'un panneau : abscisse et largeur du widget dans sa barre (px logiques) ; le panneau
/// s'ouvre sous la bande.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Anchor {
    pub x: f32,
    pub w: f32,
}

#[derive(Default)]
struct BarPanels {
    /// Aperçu qui attend la fin du délai.
    pending: slint::Timer,
    /// Dernière ancre connue de chaque genre de panneau (survol ou clic) : un épinglage décidé
    /// par Rust (Agenda) s'ouvre sous le widget.
    anchors: HashMap<String, Anchor>,
}

thread_local! {
    static BARS: RefCell<HashMap<String, BarPanels>> = RefCell::default();
}

fn with<R>(label: &str, f: impl FnOnce(&mut BarPanels) -> R) -> R {
    BARS.with_borrow_mut(|b| f(b.entry(label.to_owned()).or_default()))
}

/// Survol du widget du panneau `kind` : entrée (aperçu après le délai, ou aussitôt si un panneau
/// est déjà ouvert) ou sortie (aperçu annulé, panneau fermé peu après).
pub fn hover(label: &str, kind: &str, inside: bool, anchor: Anchor) {
    let delay = if inside { Some(if popup::keep(kind) { Duration::ZERO } else { HOVER_DELAY }) } else { None };
    let (l, k) = (label.to_owned(), kind.to_owned());
    with(label, |b| {
        b.anchors.insert(kind.to_owned(), anchor);
        match delay {
            Some(d) => b.pending.start(slint::TimerMode::SingleShot, d, move || popup::show(&l, &k, anchor.x, anchor.w, false)),
            None => b.pending.stop(),
        }
    });
    if !inside {
        popup::hide_soon();
    }
}

/// Clic : le panneau `kind` s'ouvre épinglé, sous son widget (dernière ancre connue, ou
/// `anchor`).
pub fn pin(label: &str, kind: &str, anchor: Option<Anchor>) {
    let anchor = with(label, |b| {
        b.pending.stop();
        let a = anchor.or_else(|| b.anchors.get(kind).copied()).unwrap_or_default();
        b.anchors.insert(kind.to_owned(), a);
        a
    });
    popup::show(label, kind, anchor.x, anchor.w, true);
}

/// Ouvre les réglages de l'exemplaire `id` (widget vide ou pas encore réglé, cliqué).
pub fn settings(app: &AppHandle, id: &str) {
    settings_window::open(app, SettingsPage::Widget(id.to_owned()));
}

/// Relie le global `Panels` de la barre `label`.
pub fn bind(app: &AppHandle, label: &str, window: &BarWindow) {
    let g = window.global::<Panels>();
    let l = label.to_owned();
    g.on_hover(move |kind, inside, x, w| hover(&l, &kind, inside, Anchor { x, w }));
    let l = label.to_owned();
    g.on_pin(move |kind, x, w| pin(&l, &kind, Some(Anchor { x, w })));
    let a = app.clone();
    g.on_settings(move |id| settings(&a, &id));
}

/// Barre fermée.
pub fn forget(label: &str) {
    let _ = BARS.try_with(|b| b.borrow_mut().remove(label));
}
