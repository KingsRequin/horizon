//! Réglages Météo (`ui/widget-settings/weather.slint`, app/src/widgets/weather/Settings.svelte) :
//! villes suivies (`settings.place` pour la principale, `settings.others` pour les autres),
//! recherche par nom 350 ms après la dernière frappe (`weather::weather_search`, hors du fil de
//! l'interface ; une réponse dépassée est ignorée). Une ville trouvée s'ajoute aux autres ; son
//! retrait est annulable.

use crate::ui::settings_window::offer_undo;
use crate::ui::{PlaceRow, SettingsWindow, WeatherSettingsData};
use crate::undoable::Removal;
use crate::widgets::weather::weather_search;
use horizon_core::weather::{MAX_PLACES, Place, place_key, places_of, places_settings};
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
    /// Villes de l'exemplaire montré, la principale d'abord.
    static PLACES: RefCell<Vec<Place>> = const { RefCell::new(Vec::new()) };
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
    g.on_promote(|i| promote(i.max(0) as usize));
    g.on_remove(|i| remove(i.max(0) as usize));
}

pub fn show(window: &SettingsWindow, settings: &Value) {
    let g = window.global::<WeatherSettingsData>();
    PLACES.set(places_of(settings));
    show_places(&g);
    g.set_message("".into());
    set_results(&g, Vec::new());
    g.set_cleared(g.get_cleared() + 1);
}

pub fn leave() {
    TIMER.with(|t| t.stop());
    GENERATION.set(GENERATION.get() + 1);
}

fn rows(places: &[Place]) -> ModelRc<PlaceRow> {
    let rows: Vec<PlaceRow> = places.iter().map(|p| PlaceRow { name: p.name.as_str().into(), detail: p.detail.as_str().into() }).collect();
    ModelRc::new(VecModel::from(rows))
}

fn show_places(g: &WeatherSettingsData) {
    PLACES.with_borrow(|p| {
        g.set_places(rows(p));
        g.set_full(p.len() >= MAX_PLACES);
    });
}

fn set_results(g: &WeatherSettingsData, places: Vec<Place>) {
    g.set_results(rows(&places));
    RESULTS.set(places);
}

/// Enregistre les villes (la première est la principale) et les montre.
fn set_places(places: Vec<Place>) {
    super::save(places_settings(&places));
    PLACES.set(places);
    data(show_places);
}

/// Frappe dans le champ : recherche après le délai (moins de 2 caractères : aucun résultat).
fn search(query: String) {
    let generation = GENERATION.get() + 1;
    GENERATION.set(generation);
    let q = query.trim().to_owned();
    if q.chars().count() < 2 {
        TIMER.with(|t| t.stop());
        data(|g| {
            g.set_message("".into());
            set_results(g, Vec::new());
        });
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

/// Villes après l'ajout de `place` ; `Err` (à dire) si elle est déjà suivie ou s'il n'y a plus
/// de place.
fn with_added(mut places: Vec<Place>, place: Place) -> Result<Vec<Place>, &'static str> {
    if places.iter().any(|p| place_key(p) == place_key(&place)) {
        return Err("Cette ville est déjà suivie.");
    }
    if places.len() >= MAX_PLACES {
        return Err("Trois villes au plus : retire-en une d’abord.");
    }
    places.push(place);
    Ok(places)
}

/// Choix d'une ville trouvée : ajoutée aux autres, champ et résultats vidés.
fn choose(i: usize) {
    let Some(place) = RESULTS.with_borrow(|r| r.get(i).cloned()) else { return };
    leave();
    let added = with_added(PLACES.with_borrow(Vec::clone), place);
    data(|g| {
        g.set_message(added.as_ref().err().copied().unwrap_or_default().into());
        set_results(g, Vec::new());
        g.set_cleared(g.get_cleared() + 1);
    });
    if let Ok(places) = added {
        set_places(places);
    }
}

/// La ville de rang `i` devient la principale (celle de la barre) ; les autres gardent leur ordre.
fn promote(i: usize) {
    let mut places = PLACES.with_borrow(Vec::clone);
    if i == 0 || i >= places.len() {
        return;
    }
    let place = places.remove(i);
    places.insert(0, place);
    set_places(places);
}

/// Retrait annulable d'une ville : elle revient à sa place.
fn remove(i: usize) {
    let before = PLACES.with_borrow(Vec::clone);
    let Some(name) = before.get(i).map(|p| p.name.clone()) else { return };
    let mut next = before.clone();
    next.remove(i);
    set_places(next);
    data(|g| g.set_message("".into()));
    offer_undo(Removal { label: format!("Ville « {name} » retirée"), undo: Box::new(move || set_places(before)), commit: None });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(name: &str, lat: f64) -> Place {
        Place { name: name.into(), detail: String::new(), lat, lon: 2.0 }
    }

    #[test]
    fn a_found_place_is_added_once_up_to_the_limit() {
        let names = |p: &[Place]| p.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        let two = with_added(vec![place("Paris", 48.0)], place("Lyon", 45.0)).unwrap();
        assert_eq!(names(&two), ["Paris", "Lyon"], "la principale reste la première");
        assert_eq!(with_added(two.clone(), place("Lyon bis", 45.0)), Err("Cette ville est déjà suivie."));
        let three = with_added(two, place("Lille", 50.0)).unwrap();
        assert!(with_added(three, place("Brest", 48.4)).is_err(), "pas de quatrième");
    }
}
