//! Widget « Fenêtres » : nombre de fenêtres d'application ouvertes, fenêtres par écran (panneau :
//! `panel`) et « ramener ici » (clic dans le panneau, palette).
//!
//! Pas de scrutation : les crochets d'événements de Windows (fenêtre affichée, cachée, détruite,
//! masquée, réduite) marquent le compte comme périmé ; il est relu après 300 ms de calme, au
//! plus une fois toutes les 2 s, et seulement si un widget Fenêtres est sur une barre. La liste
//! complète (applis, écrans) n'est lue qu'à l'ouverture du panneau ou de la palette.
//!
//! Repris de `app/src-tauri/src/widgets/windows.rs`. Différences : l'écran d'origine est donné
//! par l'appelant (rectangle de la barre ou de la palette). Un échec de « ramener ici » est dit
//! dans l'îlot.

mod panel;

use super::Backend;
use crate::alerts::{Alert, Severity};
use horizon_core::dnd::AlertKind;
use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use crate::ui::BarWindow;
use crate::util::{Published, Waker, spawn_named, start_thread};
use horizon_core::app_windows::{ScreenGroup, group_by_screen, monitor_of};
use horizon_core::geometry::{MonitorInfo, Rect};
use horizon_core::layout::{BarLayout, WidgetKind};
use horizon_sys::app_windows::Change;
use slint::ComponentHandle;
use std::collections::HashSet;
use std::time::{Duration, Instant};

/// Nombre de fenêtres d'application ouvertes (widget « Fenêtres »).
pub const EV_WINDOWS: &str = "horizon://windows";

pub const BACKEND: Backend = Backend {
    id: "windows",
    name: "fenêtres ouvertes",
    manage: |app| {
        app.manage(Windows::default());
    },
    start: Some(start),
    bind: Some(bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

/// Écart minimal entre deux relevés du compte.
const MIN_GAP: Duration = Duration::from_secs(2);
/// Calme attendu après un événement (une fenêtre qui s'ouvre en produit plusieurs).
const SETTLE: Duration = Duration::from_millis(300);

pub struct Windows {
    count: Published<usize>,
    waker: Waker,
}

impl Default for Windows {
    fn default() -> Self {
        Windows { count: Published::new(EV_WINDOWS, 0), waker: Waker::default() }
    }
}

/// Un widget Fenêtres est sur une barre du profil actif.
fn shown(app: &AppHandle) -> bool {
    app.state::<AppState>().read(|l| l.first_of(WidgetKind::WINDOWS).is_some())
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    horizon_sys::app_windows::watch(tx)?;
    let app = app.clone();
    start_thread("fenetres", move || count_worker(&app, &rx))
}

fn count_worker(app: &AppHandle, rx: &crossbeam_channel::Receiver<Change>) {
    let own = std::process::id();
    let state = app.state::<Windows>();
    // Fenêtres comptées au dernier relevé : seule leur destruction change le compte.
    let mut known: HashSet<isize> = HashSet::new();
    let mut last: Option<Instant> = None;
    let mut dirty = true;
    loop {
        if dirty && shown(app) {
            if let Some(t) = last {
                std::thread::sleep(MIN_GAP.saturating_sub(t.elapsed()));
            }
            // Ce qui est arrivé avant le relevé est couvert par lui.
            while rx.try_recv().is_ok() {}
            known = horizon_sys::app_windows::handles(own).into_iter().collect();
            state.count.set(app, known.len());
            last = Some(Instant::now());
            dirty = false;
        }
        crossbeam_channel::select! {
            recv(rx) -> c => match c {
                Ok(Change::Shown(_)) => dirty = true,
                Ok(Change::Destroyed(h)) => dirty |= known.contains(&h),
                Err(_) => return,
            },
            recv(state.waker.receiver()) -> _ => dirty = true,
        }
        if dirty {
            std::thread::sleep(SETTLE);
        }
    }
}

/// Écran d'un rectangle, sinon l'écran principal.
fn screen_at(at: Option<Rect>, monitors: &[MonitorInfo]) -> Option<MonitorInfo> {
    at.and_then(|r| monitor_of(r, monitors))
        .or_else(|| monitors.iter().find(|m| m.primary))
        .or(monitors.first())
        .cloned()
}

/// Rectangle (pixels physiques) de la fenêtre `hwnd` (barre d'où vient la demande).
pub fn window_rect(hwnd: isize) -> Option<Rect> {
    horizon_sys::app_windows::state(hwnd).map(|(r, _, _)| r)
}

/// Fenêtres par écran, vues depuis l'écran `at` (son groupe d'abord, marqué « ici »). Lent
/// (processus, COM) : hors du fil de l'interface (panneau ; palette, phase F).
pub fn groups(app: &AppHandle, at: Option<Rect>) -> Vec<ScreenGroup> {
    let monitors = horizon_sys::monitors::list();
    let here = screen_at(at, &monitors).map(|m| m.id);
    let list = horizon_sys::app_windows::list(std::process::id(), &monitors);
    app.state::<Windows>().count.set(app, list.len());
    group_by_screen(list, &monitors, here.as_deref())
}

/// Ramène la fenêtre `hwnd` sur l'écran `at` et la met au premier plan, sur un fil à part (la
/// fenêtre peut tarder à répondre). Un échec est dit dans l'îlot.
pub fn bring(app: &AppHandle, hwnd: isize, at: Option<Rect>) {
    let app = app.clone();
    spawn_named("fenetre-ramener", move || {
        let monitors = horizon_sys::monitors::list();
        let result = match screen_at(at, &monitors) {
            Some(to) => horizon_sys::app_windows::bring_here(hwnd, &to, &monitors),
            None => Err("aucun écran".into()),
        };
        if let Err(e) = result {
            log::warn!("ramener la fenêtre {hwnd:#x} : {e}");
            let alert = Alert::icon("browsers", Severity::Attention, "Fenêtre non déplacée".into(), e);
            crate::alerts::show(&app, AlertKind::Feedback, alert);
        }
    });
}

/// « Aucune fenêtre », « 1 fenêtre », « 7 fenêtres » (`windows.ts`, `countLabel`).
pub fn count_label(n: usize) -> String {
    match n {
        0 => "Aucune fenêtre".into(),
        1 => "1 fenêtre".into(),
        n => format!("{n} fenêtres"),
    }
}

fn apply(window: &BarWindow, n: usize) {
    let g = window.global::<crate::ui::WindowsData>();
    g.set_count(n as i32);
    g.set_title(count_label(n).into());
}

/// Relie le widget d'une barre au service (fil de l'interface). Comme `windows_count` : un
/// relevé part (le widget vient de s'afficher) et suivra par l'événement.
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _bar: &BarLayout) {
    let state = app.state::<Windows>();
    state.waker.wake();
    apply(window, state.count.get());
    let w = window.as_weak();
    crate::host::listen(EV_WINDOWS, Some(label), move |e| {
        if let (Some(n), Some(w)) = (e.payload::<usize>(), w.upgrade()) {
            apply(&w, *n);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_labels() {
        assert_eq!(count_label(0), "Aucune fenêtre");
        assert_eq!(count_label(1), "1 fenêtre");
        assert_eq!(count_label(7), "7 fenêtres");
    }
}
