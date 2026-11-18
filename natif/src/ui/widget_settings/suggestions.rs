//! Réglages Suggestions (`ui/widget-settings/suggestions.slint`,
//! app/src/widgets/suggestions/Settings.svelte) : éléments exclus (`suggestions_blocked`, lus
//! hors du fil de l'interface). Retrait différé : l'élément disparaît de la liste et n'est de
//! nouveau proposé (`suggestions_unblock`) qu'à la fin du délai d'annulation.

use crate::host::AppHandle;
use crate::ui::settings_window::offer_undo;
use crate::ui::{BlockedRow, SettingsWindow, SuggestionsSettingsData};
use crate::undoable::Removal;
use crate::widgets::suggestions::{SuggestionView, suggestions_blocked, suggestions_unblock};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

#[derive(Default)]
struct List {
    blocked: Vec<SuggestionView>,
    /// Retirés, en attente de la fin du délai d'annulation.
    leaving: Vec<String>,
}

thread_local! {
    static LIST: RefCell<List> = RefCell::default();
}

/// Éléments montrés : les exclus moins ceux qui partent.
fn visible(l: &List) -> Vec<SuggestionView> {
    l.blocked.iter().filter(|b| !l.leaving.contains(&b.key)).cloned().collect()
}

fn refresh() {
    let Some(w) = super::window() else { return };
    let rows: Vec<BlockedRow> = LIST.with_borrow(visible)
        .iter()
        .map(|s| BlockedRow { label: s.label.as_str().into(), kind: if s.kind == "site" { "site" } else { "application" }.into() })
        .collect();
    w.global::<SuggestionsSettingsData>().set_rows(ModelRc::new(VecModel::from(rows)));
}

fn set_error(e: &str) {
    if let Some(w) = super::window() {
        w.global::<SuggestionsSettingsData>().set_error(e.into());
    }
}

pub fn bind(window: &SettingsWindow) {
    window.global::<SuggestionsSettingsData>().on_unblock(|i| unblock(i.max(0) as usize));
}

pub fn show(app: &AppHandle, window: &SettingsWindow) {
    LIST.set(List::default());
    let g = window.global::<SuggestionsSettingsData>();
    g.set_loaded(false);
    g.set_error("".into());
    refresh();
    let app = app.clone();
    crate::util::spawn_named("reglages-suggestions", move || {
        let blocked = suggestions_blocked(&app);
        let _ = slint::invoke_from_event_loop(move || {
            LIST.with_borrow_mut(|l| l.blocked = blocked);
            if let Some(w) = super::window() {
                w.global::<SuggestionsSettingsData>().set_loaded(true);
            }
            refresh();
        });
    });
}

/// Corbeille d'un élément : retiré de la liste, de nouveau proposé à la fin du délai.
fn unblock(i: usize) {
    let Some(s) = LIST.with_borrow(|l| visible(l).get(i).cloned()) else { return };
    let Some(app) = super::app() else { return };
    LIST.with_borrow_mut(|l| l.leaving.push(s.key.clone()));
    refresh();
    let key = s.key.clone();
    let back = move |k: &str| {
        LIST.with_borrow_mut(|l| l.leaving.retain(|x| x != k));
        refresh();
    };
    let undo_key = key.clone();
    offer_undo(Removal {
        label: format!("« {} » de nouveau proposé", s.label),
        undo: Box::new(move || back(&undo_key)),
        commit: Some(Box::new(move || {
            // Requête SQL : hors du fil de l'interface.
            crate::util::spawn_named("suggestion-reproposee", move || {
                let result = suggestions_unblock(&app, &key);
                let _ = slint::invoke_from_event_loop(move || {
                    match result {
                        Ok(()) => LIST.with_borrow_mut(|l| l.blocked.retain(|b| b.key != key)),
                        Err(e) => set_error(&e),
                    }
                    LIST.with_borrow_mut(|l| l.leaving.retain(|x| x != &key));
                    refresh();
                });
            });
        })),
    });
}
