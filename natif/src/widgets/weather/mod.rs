//! Widget Météo (Open-Meteo) : requêtes HTTP hors du fil de l'interface, résultats gardés en
//! cache (une barre par écran demande les mêmes données).
//!
//! Une seule lecture par ville, quel que soit le nombre de widgets et de barres ; le résultat
//! est diffusé à toutes les barres (`horizon://weather`). Les villes des widgets de la
//! disposition sont relues toutes les 15 min, et tout de suite à la sortie de veille.
//!
//! Repris de `app/src-tauri/src/widgets/weather.rs`, recherche de villes des réglages comprise
//! (`weather_search`, bloquante : hors du fil de l'interface). Liaison à la barre (global `WeatherData`) ici,
//! au panneau dans `panel`.

mod panel;

use crate::host::{self, AppHandle, Emitter, Manager};
use crate::ui::{BarWindow, WeatherData, WeatherItem, WeatherState, icons};
use crate::util::{lock, spawn_named, start_thread};
use crate::web::get;
use crate::widgets::Backend;
use horizon_core::layout::BarLayout;
use horizon_core::weather::{
    Forecast, Freshness, Place, forecast_url, geocode_url, parse_forecast, parse_places, place_key, places_of, wanted_places,
};
use serde::Serialize;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Pas du contrôle des villes à relire.
const WEATHER_CHECK: Duration = Duration::from_secs(60);
/// Heures de prévision gardées pour le panneau.
const HOURS: usize = 12;

/// Météo d'une ville (`WeatherView`), envoyée à toutes les barres.
pub const EV_WEATHER: &str = "horizon://weather";

pub const BACKEND: Backend = Backend {
    id: "weather",
    name: "météo",
    manage: |app| {
        app.manage(Weather::default());
    },
    start: Some(start),
    bind: Some(bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

/// Prévision par ville (`place_key`).
#[derive(Default)]
pub struct Weather(Mutex<HashMap<String, WeatherEntry>>);

struct WeatherEntry {
    place: Place,
    forecast: Option<Forecast>,
    error: Option<String>,
    fresh: Freshness,
    version: u64,
}

/// État de la météo d'une ville, tel qu'envoyé aux barres.
#[derive(Clone, Serialize)]
pub struct WeatherView {
    pub key: String,
    pub forecast: Option<Forecast>,
    /// Dernière erreur ; la prévision précédente est gardée.
    pub error: Option<String>,
    pub loading: bool,
    /// Croît à chaque changement (une réponse plus ancienne qu'un événement est ignorée).
    pub version: u64,
}

fn next_version() -> u64 {
    static VERSION: AtomicU64 = AtomicU64::new(0);
    VERSION.fetch_add(1, Ordering::Relaxed) + 1
}

impl WeatherEntry {
    fn view(&self, key: &str) -> WeatherView {
        WeatherView {
            key: key.to_owned(),
            forecast: self.forecast.clone(),
            error: self.error.clone(),
            loading: self.fresh.loading(),
            version: self.version,
        }
    }
}

fn fetch_weather(place: &Place) -> Result<Forecast, String> {
    let body = get(&forecast_url(place), 2 * 1024 * 1024)?;
    parse_forecast(&body, HOURS).map_err(|e| e.to_string())
}

/// Villes qui répondent à `query` (géocodage Open-Meteo, réglages) ; moins de 2 caractères :
/// aucune. Bloquant : hors du fil de l'interface.
pub fn weather_search(query: &str) -> Result<Vec<Place>, String> {
    if query.trim().chars().count() < 2 {
        return Ok(Vec::new());
    }
    parse_places(&get(&geocode_url(query), 512 * 1024)?).map_err(|e| e.to_string())
}

/// État d'une ville ; lance sa lecture si elle est due (une seule à la fois), le résultat
/// partant ensuite à toutes les barres.
pub fn weather_get(app: &AppHandle, place: &Place) -> WeatherView {
    let key = place_key(place);
    let weather = app.state::<Weather>();
    let mut map = lock(&weather.0);
    let e = map.entry(key.clone()).or_insert_with(|| WeatherEntry {
        place: place.clone(),
        forecast: None,
        error: None,
        fresh: Freshness::default(),
        version: 0,
    });
    let (now, epoch) = (Instant::now(), crate::resume::epoch());
    if e.fresh.due(now, epoch) {
        e.fresh.start(now, epoch);
        e.version = next_version();
        let (app, place, key) = (app.clone(), e.place.clone(), key.clone());
        spawn_named("meteo-lecture", move || {
            let result = fetch_weather(&place);
            let view = {
                let weather = app.state::<Weather>();
                let mut map = lock(&weather.0);
                // Ville retirée entre-temps de la disposition : résultat oublié.
                let Some(e) = map.get_mut(&key) else { return };
                e.fresh.finish(result.is_ok(), Instant::now());
                match result {
                    Ok(f) => (e.forecast, e.error) = (Some(f), None),
                    Err(err) => {
                        log::info!("météo de {} : {err}", place.name);
                        e.error = Some(err);
                    }
                }
                e.version = next_version();
                e.view(&key)
            };
            let _ = app.emit(EV_WEATHER, view);
        });
    }
    e.view(&key)
}

/// Relit les villes des widgets Météo quand c'est dû (et à la sortie de veille) ; les villes
/// qui ne sont plus dans la disposition sont oubliées.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("weather", move || {
        loop {
            let places = app.state::<crate::state::AppState>().read(wanted_places);
            let keys: Vec<String> = places.iter().map(place_key).collect();
            lock(&app.state::<Weather>().inner().0).retain(|k, _| keys.contains(k));
            for p in &places {
                weather_get(&app, p);
            }
            crate::resume::sleep(WEATHER_CHECK);
        }
    })
}

/// Icône du ciel pour un code OMM (`sky` de weather.ts).
pub fn sky_icon(code: u8, day: bool) -> &'static str {
    match code {
        0 | 1 => {
            if day {
                "sun"
            } else {
                "moon"
            }
        }
        2 => {
            if day {
                "cloudSun"
            } else {
                "cloudMoon"
            }
        }
        45 | 48 => "cloudFog",
        51..=57 | 61 | 63 | 65..=67 | 80..=82 => "cloudRain",
        71..=77 | 85 | 86 => "cloudSnow",
        95 | 96 | 99 => "cloudLightning",
        _ => "cloud",
    }
}

/// « 9° » (arrondi comme `Math.round` : ,5 vers le haut).
fn deg(t: f64) -> String {
    format!("{}°", (t + 0.5).floor() as i64)
}

/// Ce que montre un exemplaire : son lieu (s'il en a un) et la dernière vue reçue.
fn item(place: bool, view: Option<&WeatherView>) -> WeatherItem {
    let mut it = WeatherItem { state: WeatherState::Unset, icon: Default::default(), temp: Default::default(), stale: false };
    if !place {
        return it;
    }
    let Some(v) = view else {
        it.state = WeatherState::Loading;
        return it;
    };
    match (&v.forecast, &v.error) {
        (Some(f), err) => {
            it.state = WeatherState::Forecast;
            it.icon = icons::image(sky_icon(f.current.code, f.current.is_day));
            it.temp = deg(f.current.temp).into();
            it.stale = err.is_some();
        }
        (None, Some(_)) => it.state = WeatherState::Error,
        (None, None) => it.state = WeatherState::Loading,
    }
    it
}

/// Ville principale d'un exemplaire, celle que montre la barre (`settings.place`).
fn place_of(settings: &serde_json::Value) -> Option<Place> {
    places_of(settings).into_iter().next()
}

/// Villes montrées par le panneau de l'exemplaire `id` : les siennes, sinon celles du premier
/// widget Météo réglé (une tuile ajoutée sans ville resterait sinon sur « Chargement… »).
fn panel_places(l: &horizon_core::layout::Layout, id: &str) -> Vec<Place> {
    let own = l.widget(id).map(|w| places_of(&w.settings)).unwrap_or_default();
    if !own.is_empty() {
        return own;
    }
    l.widgets_of(horizon_core::layout::WidgetKind::WEATHER).map(|w| places_of(&w.settings)).find(|p| !p.is_empty()).unwrap_or_default()
}

/// Un exemplaire : la clé de son lieu et la version de la vue montrée.
struct Row {
    key: Option<String>,
    version: u64,
}

fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let mut rows = Vec::new();
    let mut items = Vec::new();
    for w in super::instances(bar, "weather") {
        let place = place_of(&w.settings);
        let view = place.as_ref().map(|p| weather_get(app, p));
        items.push(item(place.is_some(), view.as_ref()));
        rows.push(Row { key: view.as_ref().map(|v| v.key.clone()), version: view.map_or(0, |v| v.version) });
    }
    let model = Rc::new(VecModel::from(items));
    window.global::<WeatherData>().set_items(ModelRc::from(model.clone()));
    let rows = std::cell::RefCell::new(rows);
    host::listen(EV_WEATHER, Some(label), move |e| {
        let Some(v) = e.payload::<WeatherView>() else { return };
        for (i, row) in rows.borrow_mut().iter_mut().enumerate() {
            // Une réponse plus ancienne que la vue montrée est ignorée.
            if row.key.as_deref() == Some(v.key.as_str()) && v.version >= row.version {
                row.version = v.version;
                let next = item(true, Some(v));
                if model.row_data(i).as_ref() != Some(&next) {
                    model.set_row_data(i, next);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sky_icons_follow_the_wmo_codes() {
        assert_eq!(sky_icon(0, true), "sun");
        assert_eq!(sky_icon(1, false), "moon");
        assert_eq!(sky_icon(2, false), "cloudMoon");
        assert_eq!(sky_icon(3, true), "cloud");
        assert_eq!(sky_icon(48, true), "cloudFog");
        assert_eq!(sky_icon(56, true), "cloudRain");
        assert_eq!(sky_icon(62, true), "cloud", "code inconnu : nuageux");
        assert_eq!(sky_icon(81, true), "cloudRain");
        assert_eq!(sky_icon(86, true), "cloudSnow");
        assert_eq!(sky_icon(99, true), "cloudLightning");
    }

    #[test]
    fn item_states() {
        assert_eq!(item(false, None).state, WeatherState::Unset);
        assert_eq!(item(true, None).state, WeatherState::Loading);
        let v = WeatherView { key: "k".into(), forecast: None, error: Some("x".into()), loading: false, version: 1 };
        assert_eq!(item(true, Some(&v)).state, WeatherState::Error);
        assert_eq!(deg(9.4), "9°");
        assert_eq!(deg(-0.4), "0°");
    }

    #[test]
    fn short_searches_ask_nothing() {
        assert_eq!(weather_search(" a "), Ok(Vec::new()));
    }

    #[test]
    fn place_from_settings() {
        let s = serde_json::json!({ "place": { "name": "Paris", "detail": "France", "lat": 48.85, "lon": 2.35 } });
        assert_eq!(place_of(&s).map(|p| p.name), Some("Paris".into()));
        assert!(place_of(&serde_json::json!({ "place": { "name": "x" } })).is_none());
        assert!(place_of(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn unset_panel_takes_the_first_set_place() {
        use horizon_core::geometry::{MonitorInfo, Rect};
        use horizon_core::layout::{Layout, WidgetKind, Zone};
        let m = MonitorInfo {
            id: "a".into(),
            device_name: "a".into(),
            rect: Rect { x: 0, y: 0, w: 1920, h: 1080 },
            work: Rect { x: 0, y: 40, w: 1920, h: 1040 },
            scale: 1.0,
            primary: true,
            portrait: false,
        };
        let mut l = Layout::default_for(&[m]);
        let names = |l: &Layout, id: &str| panel_places(l, id).into_iter().map(|p| p.name).collect::<Vec<_>>();
        let tile = l.add_widget("a", Zone::Tiles, WidgetKind::WEATHER).unwrap();
        assert!(panel_places(&l, &tile).is_empty(), "aucun widget Météo réglé");
        let set = l.add_widget("a", Zone::Right, WidgetKind::WEATHER).unwrap();
        let paris = serde_json::json!({
            "place": { "name": "Paris", "detail": "France", "lat": 48.85, "lon": 2.35 },
            "others": [{ "name": "Brest", "detail": "France", "lat": 48.39, "lon": -4.49 }],
        });
        l.set_widget_settings(&set, paris).unwrap();
        assert_eq!(names(&l, &tile), ["Paris", "Brest"], "villes reprises");
        let lyon = serde_json::json!({ "place": { "name": "Lyon", "detail": "France", "lat": 45.76, "lon": 4.84 } });
        l.set_widget_settings(&tile, lyon).unwrap();
        assert_eq!(names(&l, &tile), ["Lyon"], "les siennes d'abord");
    }
}
