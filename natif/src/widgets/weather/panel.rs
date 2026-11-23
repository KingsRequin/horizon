//! Panneau Météo (global `WeatherPanelData` de `ui/widgets/weather/panel.slint`), un par
//! exemplaire (« weather:<id> ») : quand une fenêtre le montre (fenêtre surgissante ou tuile), les
//! villes sont lues dans les réglages de l'exemplaire et leur prévision demandée (`weather_get`,
//! une seule lecture par ville) ; les vues reçues ensuite (`horizon://weather`) les remplacent,
//! une réponse plus ancienne étant ignorée. Avec plusieurs villes, une rangée les résume (ciel,
//! température) et un clic choisit celle dont le détail est montré (la principale à l'ouverture).
//! Chaque fenêtre a son exemplaire et ses listes. Fil de l'interface.

use super::{EV_WEATHER, WeatherView, deg, panel_places, sky_icon, weather_get};
use crate::host::{self, AppHandle, Manager};
use crate::popup::{LABEL, instance_of};
use crate::state::AppState;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{WeatherPanelCity, WeatherPanelData, WeatherPanelDay, WeatherPanelHour, icons};
use crate::widgets::Panel;
use chrono::{Datelike, Local, NaiveDate};
use horizon_core::weather::{Forecast, Place, place_key};
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel { shown: Some(shown), hidden: Some(hidden), ..Panel::new(bind) };

/// Heures montrées (`forecast.hours.slice(0, 8)`).
const HOURS: usize = 8;

/// Une ville du panneau et sa dernière vue reçue.
struct City {
    place: Place,
    key: String,
    view: Option<WeatherView>,
}

/// Ce que montre le panneau dans une fenêtre : les villes de l'exemplaire, celle dont le détail
/// est montré, et ses listes.
struct Shown {
    cities: Vec<City>,
    selected: usize,
    rows: Rc<VecModel<WeatherPanelCity>>,
    hours: Rc<VecModel<WeatherPanelHour>>,
    days: Rc<VecModel<WeatherPanelDay>>,
}

thread_local! {
    /// Par fenêtre (étiquette).
    static SHOWN: RefCell<HashMap<String, Shown>> = RefCell::default();
}

/// Libellé du ciel pour un code OMM (`sky` de weather.ts).
pub fn sky_label(code: u8) -> &'static str {
    match code {
        0 => "Ciel dégagé",
        1 => "Plutôt dégagé",
        2 => "Partiellement nuageux",
        3 => "Couvert",
        45 | 48 => "Brouillard",
        51..=55 => "Bruine",
        56 | 57 => "Bruine verglaçante",
        61 => "Pluie faible",
        63 => "Pluie",
        65 => "Forte pluie",
        66 | 67 => "Pluie verglaçante",
        71..=77 => "Neige",
        80..=82 => "Averses",
        85 | 86 => "Averses de neige",
        95 => "Orage",
        96 | 99 => "Orage et grêle",
        _ => "Nuageux",
    }
}

const WEEKDAYS: [&str; 7] = ["Lun.", "Mar.", "Mer.", "Jeu.", "Ven.", "Sam.", "Dim."];

/// « 2026-11-11 » → « Auj. », « Dem. », sinon le jour abrégé (« Jeu. », `dayLabel` mis en
/// capitale par le style).
pub fn day_label(date: &str, today: NaiveDate) -> String {
    let Ok(d) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else { return date.to_owned() };
    match (d - today).num_days() {
        0 => "Auj.".into(),
        1 => "Dem.".into(),
        _ => WEEKDAYS[d.weekday().num_days_from_monday() as usize].into(),
    }
}

/// Heure d'une prévision horaire : « 21:00 ».
fn hour(time: &str) -> String {
    format!("{}:00", time.get(11..13).unwrap_or("--"))
}

/// Pluie montrée (à partir de 20 %) et « mouillée » (à partir de 40 %).
fn rain(pct: u8) -> (String, bool) {
    (if pct >= 20 { format!("{pct} %") } else { String::new() }, pct >= 40)
}

/// Résumé d'une ville dans la rangée : ciel et température, « … » en attendant la prévision,
/// « – » si elle n'a pas pu être lue.
fn city_row(c: &City) -> WeatherPanelCity {
    let forecast = c.view.as_ref().and_then(|v| v.forecast.as_ref());
    let failed = c.view.as_ref().is_some_and(|v| v.error.is_some());
    let detail = if c.place.detail.is_empty() { String::new() } else { format!(", {}", c.place.detail) };
    WeatherPanelCity {
        name: c.place.name.as_str().into(),
        tip: format!("{}{detail}", c.place.name).into(),
        has_icon: forecast.is_some(),
        icon: forecast.map(|f| icons::image(sky_icon(f.current.code, f.current.is_day))).unwrap_or_default(),
        temp: match forecast {
            Some(f) => deg(f.current.temp),
            None if failed => "–".into(),
            None => "…".into(),
        }
        .into(),
    }
}

/// Détail d'une prévision : conditions actuelles, heures et jours.
fn show_forecast(g: &WeatherPanelData<'_>, s: &Shown, place: &Place, f: &Forecast) {
    let today = Local::now().date_naive();
    g.set_place(place.name.as_str().into());
    g.set_icon(icons::image(sky_icon(f.current.code, f.current.is_day)));
    g.set_temp(deg(f.current.temp).into());
    g.set_label(sky_label(f.current.code).into());
    g.set_feels(format!("Ressenti {} ·", deg(f.current.feels)).into());
    g.set_wind(format!("{} km/h", (f.current.wind_kmh + 0.5).floor() as i64).into());
    let hours: Vec<_> = f
        .hours
        .iter()
        .take(HOURS)
        .map(|h| {
            let (rain, wet) = rain(h.rain_pct);
            WeatherPanelHour {
                time: hour(&h.time).into(),
                icon: icons::image(sky_icon(h.code, h.is_day)),
                temp: deg(h.temp).into(),
                rain: rain.into(),
                wet,
                label: sky_label(h.code).into(),
            }
        })
        .collect();
    let days: Vec<_> = f
        .days
        .iter()
        .map(|d| {
            let (rain, wet) = rain(d.rain_pct);
            WeatherPanelDay {
                name: day_label(&d.date, today).into(),
                icon: icons::image(sky_icon(d.code, true)),
                rain: rain.into(),
                wet,
                min: deg(d.min).into(),
                max: deg(d.max).into(),
                label: sky_label(d.code).into(),
            }
        })
        .collect();
    s.hours.set_vec(hours);
    s.days.set_vec(days);
}

/// Redessine le panneau de la fenêtre `label` : la rangée des villes et le détail de la choisie.
fn render(label: &str) {
    let Some(h) = panel_hosts::host(label) else { return };
    let g = h.global::<WeatherPanelData>();
    SHOWN.with_borrow(|all| {
        let Some(s) = all.get(label) else { return };
        crate::ui::model::sync(&s.rows, s.cities.iter().map(city_row).collect());
        g.set_selected(s.selected as i32);
        let city = s.cities.get(s.selected);
        let view = city.and_then(|c| c.view.as_ref());
        g.set_has_forecast(view.is_some_and(|v| v.forecast.is_some()));
        g.set_error(view.and_then(|v| v.error.as_deref()).unwrap_or_default().into());
        if let (Some(c), Some(f)) = (city, view.and_then(|v| v.forecast.as_ref())) {
            show_forecast(&g, s, &c.place, f);
        }
    });
}

/// Garde une vue dans chaque fenêtre qui montre sa ville (pas plus ancienne que la précédente)
/// et redessine ces fenêtres.
fn receive(v: &WeatherView) {
    let labels: Vec<String> = SHOWN.with_borrow_mut(|all| {
        let mut labels = Vec::new();
        for (label, s) in all.iter_mut() {
            let newer = s.cities.iter_mut().filter(|c| c.key == v.key && c.view.as_ref().is_none_or(|old| v.version >= old.version));
            let mut changed = false;
            for c in newer {
                c.view = Some(v.clone());
                changed = true;
            }
            if changed {
                labels.push(label.clone());
            }
        }
        labels
    });
    for label in labels {
        render(&label);
    }
}

/// Clic sur une ville de la rangée : son détail est montré.
fn select(label: &str, i: usize) {
    let known = SHOWN.with_borrow_mut(|all| {
        all.get_mut(label).filter(|s| i < s.cities.len()).map(|s| s.selected = i).is_some()
    });
    if known {
        render(label);
    }
}

fn bind(_app: &AppHandle, win: &Host) {
    let label = win.label.clone();
    win.global::<WeatherPanelData>().on_select(move |i| select(&label, i.max(0) as usize));
    // Une seule fois : chaque vue est posée dans les fenêtres qui montrent sa ville.
    if win.is_popup() {
        host::listen(EV_WEATHER, Some(LABEL), |e| {
            if let Some(v) = e.payload::<WeatherView>() {
                receive(v);
            }
        });
    }
}

/// Une fenêtre montre le panneau d'un exemplaire : ses villes, et leur prévision (gardée, ou
/// demandée). Aucun widget Météo réglé : « Chargement… », comme la version Tauri.
fn shown(app: &AppHandle, win: &Host, kind: &str) {
    let id = instance_of(kind).to_owned();
    let places = app.state::<AppState>().read(|l| panel_places(l, &id));
    let cities = places
        .into_iter()
        .map(|place| {
            let view = weather_get(app, &place);
            City { key: place_key(&place), place, view: Some(view) }
        })
        .collect();
    let s = Shown { cities, selected: 0, rows: Rc::default(), hours: Rc::default(), days: Rc::default() };
    let g = win.global::<WeatherPanelData>();
    g.set_cities(ModelRc::from(s.rows.clone()));
    g.set_hours(ModelRc::from(s.hours.clone()));
    g.set_days(ModelRc::from(s.days.clone()));
    SHOWN.with_borrow_mut(|all| all.insert(win.label.clone(), s));
    render(&win.label);
}

fn hidden(_app: &AppHandle, win: &Host, _kind: &str) {
    SHOWN.with_borrow_mut(|all| all.remove(&win.label));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_city_row_waits_then_says_it_failed() {
        let place = Place { name: "Lyon".into(), detail: "Rhône, France".into(), lat: 45.76, lon: 4.84 };
        let mut c = City { key: place_key(&place), place, view: None };
        let row = city_row(&c);
        assert_eq!((row.name.as_str(), row.tip.as_str(), row.temp.as_str(), row.has_icon), ("Lyon", "Lyon, Rhône, France", "…", false));
        c.view = Some(WeatherView { key: c.key.clone(), forecast: None, error: Some("hors ligne".into()), loading: false, version: 1 });
        assert_eq!(city_row(&c).temp.as_str(), "–");
    }

    #[test]
    fn labels_follow_weather_ts() {
        assert_eq!(sky_label(0), "Ciel dégagé");
        assert_eq!(sky_label(57), "Bruine verglaçante");
        assert_eq!(sky_label(62), "Nuageux");
        assert_eq!(sky_label(99), "Orage et grêle");
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        assert_eq!(day_label("2026-11-17", today), "Auj.");
        assert_eq!(day_label("2026-11-18", today), "Dem.");
        assert_eq!(day_label("2026-11-19", today), "Jeu.");
        assert_eq!(hour("2026-11-11T21:00"), "21:00");
        assert_eq!(rain(10), (String::new(), false));
        assert_eq!(rain(30), ("30 %".into(), false));
        assert_eq!(rain(65), ("65 %".into(), true));
    }
}
