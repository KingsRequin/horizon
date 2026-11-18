//! Réglages Météo (`ui/widget-settings/weather.slint`, app/src/widgets/weather/Settings.svelte) :
//! ville choisie (`settings.place`), recherche par nom 350 ms après la dernière frappe
//! (`weather::weather_search`, hors du fil de l'interface ; une réponse dépassée est ignorée).

use crate::ui::{PlaceRow, SettingsWindow, WeatherSettingsData};
use crate::widgets::weather::weather_search;
use horizon_core::weather::Place;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const SEARCH_DELAY: Duration = Duration::from_millis(350);

thread_local! {
    static TIMER: Rc<slint::Timer> = Rc::default();
    /// Numéro de la dernière recherche lancée.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    static RESULTS: RefCell<Vec<Place>> = const { RefCell::new(Vec::new()) };
}

fn data(f: impl FnOnce(&WeatherSettingsData)) {
    if let Some(w) = super::window() {
        f(&w.global::<WeatherSettingsData>());
    }
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<WeatherSettingsData>();
    g.on_search(|q| search(q.to_string()));
    g.on_choose(|i| choose(i.max(0) as usize));
}

pub fn show(window: &SettingsWindow, settings: &Value) {
    let g = window.global::<WeatherSettingsData>();
    let place = settings.get("place").and_then(|p| serde_json::from_value::<Place>(p.clone()).ok());
    g.set_place_name(place.as_ref().map(|p| p.name.as_str()).unwrap_or_default().into());
    g.set_place_detail(place.as_ref().map(|p| p.detail.as_str()).unwrap_or_default().into());
    g.set_message("".into());
    set_results(&g, Vec::new());
    g.set_cleared(g.get_cleared() + 1);
}

pub fn leave() {
    TIMER.with(|t| t.stop());
    GENERATION.set(GENERATION.get() + 1);
}

fn set_results(g: &WeatherSettingsData, places: Vec<Place>) {
    let rows: Vec<PlaceRow> = places.iter().map(|p| PlaceRow { name: p.name.as_str().into(), detail: p.detail.as_str().into() }).collect();
    g.set_results(ModelRc::new(VecModel::from(rows)));
    RESULTS.set(places);
}

/// Frappe dans le champ : recherche après le délai (moins de 2 caractères : aucun résultat).
fn search(query: String) {
    let generation = GENERATION.get() + 1;
    GENERATION.set(generation);
    let q = query.trim().to_owned();
    if q.chars().count() < 2 {
        TIMER.with(|t| t.stop());
        data(|g| set_results(g, Vec::new()));
        return;
    }
    TIMER.with(|t| {
        t.start(slint::TimerMode::SingleShot, SEARCH_DELAY, move || {
            let q = q.clone();
            crate::util::spawn_named("meteo-recherche", move || {
                let result = weather_search(&q);
                let _ = slint::invoke_from_event_loop(move || {
                    if GENERATION.get() != generation {
                        return;
                    }
                    data(|g| match result {
                        Ok(places) => {
                            g.set_message(if places.is_empty() { "Aucune ville trouvée." } else { "" }.into());
                            set_results(g, places);
                        }
                        Err(e) => g.set_message(e.into()),
                    });
                });
            });
        });
    });
}

/// Choix d'une ville : enregistrée, champ et résultats vidés.
fn choose(i: usize) {
    let Some(place) = RESULTS.with_borrow(|r| r.get(i).cloned()) else { return };
    leave();
    match serde_json::to_value(&place) {
        Ok(v) => super::save(serde_json::json!({ "place": v })),
        Err(e) => log::warn!("météo : {e}"),
    }
    data(|g| {
        g.set_place_name(place.name.as_str().into());
        g.set_place_detail(place.detail.as_str().into());
        set_results(g, Vec::new());
        g.set_cleared(g.get_cleared() + 1);
    });
}
