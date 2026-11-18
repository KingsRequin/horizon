//! Panneau Météo (global `WeatherPanelData` de `ui/widgets/weather/panel.slint`), un par
//! exemplaire (« weather:<id> ») : quand une fenêtre le montre (fenêtre surgissante ou tuile), le
//! lieu est lu dans les réglages de l'exemplaire (`widget_get` de la version Tauri) et sa
//! prévision demandée (`weather_get`, une seule lecture par ville) ; les vues reçues ensuite
//! (`horizon://weather`) pour ce lieu la remplacent, une réponse plus ancienne étant ignorée.
//! Chaque fenêtre a son exemplaire et ses listes. Fil de l'interface.

use super::{EV_WEATHER, WeatherView, deg, panel_place, sky_icon, weather_get};
use crate::host::{self, AppHandle, Manager};
use crate::popup::{LABEL, instance_of};
use crate::state::AppState;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{WeatherPanelData, WeatherPanelDay, WeatherPanelHour, icons};
use crate::widgets::Panel;
use chrono::{Datelike, Local, NaiveDate};
use horizon_core::weather::{Forecast, Place};
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel { shown: Some(shown), hidden: Some(hidden), ..Panel::new(bind) };

/// Heures montrées (`forecast.hours.slice(0, 8)`).
const HOURS: usize = 8;

/// Ce que montre le panneau dans une fenêtre : le lieu de l'exemplaire et la dernière vue reçue.
#[derive(Default)]
struct Shown {
    place: Option<Place>,
    key: String,
    version: u64,
}

/// Listes d'une fenêtre (heures, jours).
struct Models {
    hours: Rc<VecModel<WeatherPanelHour>>,
    days: Rc<VecModel<WeatherPanelDay>>,
}

thread_local! {
    /// Par fenêtre (étiquette).
    static SHOWN: RefCell<HashMap<String, Shown>> = RefCell::default();
    static MODELS: RefCell<HashMap<String, Models>> = RefCell::default();
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

fn show_forecast(label: &str, g: &WeatherPanelData<'_>, place: Option<&Place>, f: &Forecast) {
    let today = Local::now().date_naive();
    g.set_place(place.map(|p| p.name.as_str()).unwrap_or_default().into());
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
    MODELS.with_borrow(|m| {
        if let Some(m) = m.get(label) {
            m.hours.set_vec(hours);
            m.days.set_vec(days);
        }
    });
}

/// Pose une vue dans chaque fenêtre qui montre son lieu (pas plus ancienne que la précédente).
fn show(v: &WeatherView) {
    let targets: Vec<(String, Option<Place>)> = SHOWN.with_borrow_mut(|all| {
        all.iter_mut()
            .filter(|(_, s)| s.key == v.key && v.version >= s.version)
            .map(|(label, s)| {
                s.version = v.version;
                (label.clone(), s.place.clone())
            })
            .collect()
    });
    for (label, place) in targets {
        let Some(h) = panel_hosts::host(&label) else { continue };
        let g = h.global::<WeatherPanelData>();
        g.set_has_forecast(v.forecast.is_some());
        g.set_error(v.error.as_deref().unwrap_or_default().into());
        if let Some(f) = &v.forecast {
            show_forecast(&label, &g, place.as_ref(), f);
        }
    }
}

fn bind(_app: &AppHandle, win: &Host) {
    // Une seule fois : chaque vue est posée dans les fenêtres qui montrent son lieu.
    if win.is_popup() {
        host::listen(EV_WEATHER, Some(LABEL), |e| {
            if let Some(v) = e.payload::<WeatherView>() {
                show(v);
            }
        });
    }
}

/// Une fenêtre montre le panneau d'un exemplaire : son lieu, et sa prévision (gardée, ou
/// demandée).
fn shown(app: &AppHandle, win: &Host, kind: &str) {
    let id = instance_of(kind).to_owned();
    let place = app.state::<AppState>().read(|l| panel_place(l, &id));
    SHOWN.with_borrow_mut(|all| all.insert(win.label.clone(), Shown { place: place.clone(), key: String::new(), version: 0 }));
    let m = Models { hours: Rc::default(), days: Rc::default() };
    let g = win.global::<WeatherPanelData>();
    g.set_hours(ModelRc::from(m.hours.clone()));
    g.set_days(ModelRc::from(m.days.clone()));
    MODELS.with_borrow_mut(|all| all.insert(win.label.clone(), m));
    g.set_has_forecast(false);
    g.set_error(Default::default());
    // Aucun widget Météo réglé : « Chargement… », comme la version Tauri.
    if let Some(p) = place {
        let v = weather_get(app, &p);
        SHOWN.with_borrow_mut(|all| {
            if let Some(s) = all.get_mut(&win.label) {
                s.key = v.key.clone();
            }
        });
        show(&v);
    }
}

fn hidden(_app: &AppHandle, win: &Host, _kind: &str) {
    SHOWN.with_borrow_mut(|all| all.remove(&win.label));
    MODELS.with_borrow_mut(|all| all.remove(&win.label));
}

#[cfg(test)]
mod tests {
    use super::*;

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
