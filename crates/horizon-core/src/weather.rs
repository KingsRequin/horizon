//! Météo d'Open-Meteo (sans clé) : URL des requêtes et lecture des réponses.

use crate::layout::{Layout, WidgetKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Âge d'une prévision au-delà duquel elle est relue.
pub(crate) const TTL: Duration = Duration::from_secs(15 * 60);
/// Délai avant de retenter après un échec.
pub(crate) const RETRY: Duration = Duration::from_secs(2 * 60);

/// Lieu choisi dans les réglages du widget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    /// Région et pays, pour distinguer les homonymes.
    #[serde(default)]
    pub detail: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Current {
    pub temp: f64,
    pub feels: f64,
    /// Code météo OMM (WMO).
    pub code: u8,
    pub is_day: bool,
    pub wind_kmh: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hour {
    /// Heure locale du lieu, « 2026-11-11T14:00 ».
    pub time: String,
    pub temp: f64,
    pub code: u8,
    pub rain_pct: u8,
    pub is_day: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Day {
    /// Date locale du lieu, « 2026-11-11 ».
    pub date: String,
    pub min: f64,
    pub max: f64,
    pub code: u8,
    pub rain_pct: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Forecast {
    pub current: Current,
    /// Prochaines heures (à partir de l'heure en cours).
    pub hours: Vec<Hour>,
    pub days: Vec<Day>,
}

pub fn forecast_url(p: &Place) -> String {
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,apparent_temperature,weather_code,is_day,wind_speed_10m\
         &hourly=temperature_2m,weather_code,precipitation_probability,is_day\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max\
         &timezone=auto&forecast_days=4",
        p.lat, p.lon
    )
}

/// Recherche d'une ville (nom saisi, encodé pour l'URL).
pub fn geocode_url(query: &str) -> String {
    let q: String = query
        .trim()
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    format!("https://geocoding-api.open-meteo.com/v1/search?name={q}&count=6&language=fr&format=json")
}

pub fn parse_places(body: &str) -> anyhow::Result<Vec<Place>> {
    let v: Value = serde_json::from_str(body).map_err(|_| anyhow::anyhow!("réponse illisible"))?;
    Ok(v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let detail = [r["admin1"].as_str(), r["country"].as_str()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", ");
                    Some(Place {
                        name: r["name"].as_str()?.to_owned(),
                        detail,
                        lat: r["latitude"].as_f64()?,
                        lon: r["longitude"].as_f64()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default())
}

fn nums(v: &Value) -> Vec<Option<f64>> {
    v.as_array().map(|a| a.iter().map(Value::as_f64).collect()).unwrap_or_default()
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap_or_default().to_owned()).collect())
        .unwrap_or_default()
}

/// Lit une prévision ; `hours` = nombre d'heures gardées à partir de l'heure en cours.
pub fn parse_forecast(body: &str, hours: usize) -> anyhow::Result<Forecast> {
    let v: Value = serde_json::from_str(body).map_err(|_| anyhow::anyhow!("réponse illisible"))?;
    if let Some(reason) = v["reason"].as_str() {
        anyhow::bail!("Open-Meteo : {reason}");
    }
    let c = &v["current"];
    let num = |x: &Value| x.as_f64().ok_or_else(|| anyhow::anyhow!("réponse incomplète"));
    let current = Current {
        temp: num(&c["temperature_2m"])?,
        feels: c["apparent_temperature"].as_f64().unwrap_or(f64::NAN),
        code: num(&c["weather_code"])? as u8,
        is_day: c["is_day"].as_f64() != Some(0.0),
        wind_kmh: c["wind_speed_10m"].as_f64().unwrap_or(0.0),
    };
    // L'heure en cours, au format des heures horaires (« …T14:00 »).
    // Découpe sûre : un texte inattendu (non ASCII) ne doit pas faire paniquer.
    let now_hour = c["time"].as_str().map(|t| format!("{}:00", t.get(..13).unwrap_or(t))).unwrap_or_default();
    let h = &v["hourly"];
    let (times, temps, codes, rain, day) = (
        strs(&h["time"]),
        nums(&h["temperature_2m"]),
        nums(&h["weather_code"]),
        nums(&h["precipitation_probability"]),
        nums(&h["is_day"]),
    );
    let first = times.iter().position(|t| *t >= now_hour).unwrap_or(0);
    let hours = (first..times.len())
        .filter_map(|i| {
            Some(Hour {
                time: times[i].clone(),
                temp: (*temps.get(i)?)?,
                code: codes.get(i).copied().flatten()? as u8,
                rain_pct: rain.get(i).copied().flatten().unwrap_or(0.0) as u8,
                is_day: day.get(i).copied().flatten() != Some(0.0),
            })
        })
        .take(hours)
        .collect();
    let d = &v["daily"];
    let (dates, maxs, mins, dcodes, drain) = (
        strs(&d["time"]),
        nums(&d["temperature_2m_max"]),
        nums(&d["temperature_2m_min"]),
        nums(&d["weather_code"]),
        nums(&d["precipitation_probability_max"]),
    );
    let days = (0..dates.len())
        .filter_map(|i| {
            Some(Day {
                date: dates[i].clone(),
                min: (*mins.get(i)?)?,
                max: (*maxs.get(i)?)?,
                code: dcodes.get(i).copied().flatten()? as u8,
                rain_pct: drain.get(i).copied().flatten().unwrap_or(0.0) as u8,
            })
        })
        .collect();
    Ok(Forecast { current, hours, days })
}

/// Clé d'un lieu (cache et diffusion) : coordonnées arrondies au millième de degré.
pub fn place_key(p: &Place) -> String {
    format!("{:.3},{:.3}", p.lat, p.lon)
}

/// Villes qu'un widget Météo peut suivre.
pub const MAX_PLACES: usize = 3;

/// Ajoute `p` à `places` s'il n'y est pas déjà (même clé).
fn push_new(places: &mut Vec<Place>, p: Place) {
    if !places.iter().any(|q| place_key(q) == place_key(&p)) {
        places.push(p);
    }
}

/// Villes des réglages d'un widget Météo : la principale (`place`, montrée dans la barre) puis
/// les autres (`others`), sans doublon, `MAX_PLACES` au plus. Ce qui est illisible est ignoré.
pub fn places_of(settings: &Value) -> Vec<Place> {
    let others = settings.get("others").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut places = Vec::new();
    for v in settings.get("place").into_iter().chain(others) {
        if let Ok(p) = serde_json::from_value::<Place>(v.clone()) {
            push_new(&mut places, p);
        }
    }
    places.truncate(MAX_PLACES);
    places
}

/// Réglages (`place`, `others`) qui enregistrent ces villes, la première étant la principale.
pub fn places_settings(places: &[Place]) -> Value {
    let (main, others) = match places.split_first() {
        Some((main, others)) => (serde_json::json!(main), &others[..others.len().min(MAX_PLACES - 1)]),
        None => (Value::Null, &[][..]),
    };
    serde_json::json!({ "place": main, "others": others })
}

/// Lieux des widgets Météo des barres actives (une fois chacun).
pub fn wanted_places(layout: &Layout) -> Vec<Place> {
    let mut places: Vec<Place> = Vec::new();
    for w in layout.widgets_of(WidgetKind::WEATHER) {
        for p in places_of(&w.settings) {
            push_new(&mut places, p);
        }
    }
    places
}

/// Quand relire une donnée en ligne (prévision d'un lieu, agenda) : une seule requête à la
/// fois, relue après `ttl`, retentée après `retry` en cas d'échec, et tout de suite après une
/// sortie de veille (`epoch` = numéro de la dernière reprise). Par défaut : délais de la météo.
#[derive(Debug, Clone)]
pub struct Freshness {
    ttl: Duration,
    retry: Duration,
    ok_at: Option<Instant>,
    tried_at: Option<Instant>,
    loading: bool,
    /// Reprise de veille connue lors du dernier essai.
    epoch: u64,
}

impl Default for Freshness {
    fn default() -> Self {
        Freshness::with(TTL, RETRY)
    }
}

impl Freshness {
    pub fn with(ttl: Duration, retry: Duration) -> Freshness {
        Freshness { ttl, retry, ok_at: None, tried_at: None, loading: false, epoch: 0 }
    }

    pub fn due(&self, now: Instant, epoch: u64) -> bool {
        if self.loading {
            return false;
        }
        if self.tried_at.is_none() || epoch != self.epoch {
            return true;
        }
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        let failed = self.ok_at.is_none_or(|ok| self.tried_at.is_some_and(|t| t > ok));
        if failed {
            since(self.tried_at).is_some_and(|d| d >= self.retry)
        } else {
            since(self.ok_at).is_some_and(|d| d >= self.ttl)
        }
    }

    pub fn start(&mut self, now: Instant, epoch: u64) {
        self.loading = true;
        self.tried_at = Some(now);
        self.epoch = epoch;
    }

    pub fn finish(&mut self, ok: bool, now: Instant) {
        self.loading = false;
        if ok {
            self.ok_at = Some(now);
        }
    }

    pub fn loading(&self) -> bool {
        self.loading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"latitude":48.86,"longitude":2.34,
      "current":{"time":"2026-11-11T14:15","temperature_2m":9.4,"apparent_temperature":6.8,
        "weather_code":3,"is_day":1,"wind_speed_10m":14.2},
      "hourly":{"time":["2026-11-11T13:00","2026-11-11T14:00","2026-11-11T15:00","2026-11-11T16:00"],
        "temperature_2m":[9.0,9.4,9.1,null],"weather_code":[3,3,61,61],
        "precipitation_probability":[10,20,65,70],"is_day":[1,1,1,0]},
      "daily":{"time":["2026-11-11","2026-11-12"],"weather_code":[61,2],
        "temperature_2m_max":[10.2,12.0],"temperature_2m_min":[5.1,4.0],
        "precipitation_probability_max":[70,null]}}"#;

    #[test]
    fn parses_forecast_from_current_hour() {
        let f = parse_forecast(SAMPLE, 8).unwrap();
        assert_eq!(f.current.code, 3);
        assert!(f.current.is_day);
        assert!((f.current.temp - 9.4).abs() < 1e-9);
        // Commence à 14 h ; l'heure de 16 h sans température est sautée.
        assert_eq!(f.hours.iter().map(|h| h.time.as_str()).collect::<Vec<_>>(), ["2026-11-11T14:00", "2026-11-11T15:00"]);
        assert_eq!(f.hours[1].rain_pct, 65);
        assert_eq!(f.days.len(), 2);
        assert_eq!(f.days[1].rain_pct, 0);
        assert_eq!(parse_forecast(SAMPLE, 1).unwrap().hours.len(), 1);
    }

    #[test]
    fn odd_current_time_does_not_panic() {
        // Caractère de plusieurs octets à cheval sur l'octet 13, puis texte court.
        for time in ["2026-11-11T1é:00", "été", ""] {
            let body = SAMPLE.replace("2026-11-11T14:15", time);
            assert!(parse_forecast(&body, 8).is_ok(), "{time}");
        }
    }

    #[test]
    fn reports_api_errors() {
        let e = parse_forecast(r#"{"error":true,"reason":"Latitude must be in range"}"#, 8).unwrap_err();
        assert!(e.to_string().contains("Latitude"));
        assert!(parse_forecast("{}", 8).is_err());
    }

    #[test]
    fn parses_places_and_encodes_query() {
        let body = r#"{"results":[{"name":"Lyon","latitude":45.75,"longitude":4.85,"country":"France","admin1":"Auvergne-Rhône-Alpes"},
            {"name":"Lyon","latitude":1.0}]}"#;
        let p = parse_places(body).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].detail, "Auvergne-Rhône-Alpes, France");
        assert!(parse_places("{}").unwrap().is_empty());
        assert!(geocode_url("Saint-Étienne").contains("name=Saint-%C3%89tienne&"));
    }

    #[test]
    fn weather_is_fetched_once_then_after_ttl() {
        let t0 = Instant::now();
        let mut f = Freshness::default();
        assert!(f.due(t0, 0));
        f.start(t0, 0);
        // Une seule requête à la fois : les autres fenêtres attendent son résultat.
        assert!(!f.due(t0 + Duration::from_secs(1), 0));
        f.finish(true, t0 + Duration::from_secs(1));
        assert!(!f.due(t0 + Duration::from_secs(5 * 60), 0));
        assert!(f.due(t0 + Duration::from_secs(1) + TTL, 0));
    }

    #[test]
    fn failed_fetch_is_retried_sooner() {
        let t0 = Instant::now();
        let mut f = Freshness::default();
        f.start(t0, 0);
        f.finish(true, t0);
        let t1 = t0 + TTL;
        f.start(t1, 0);
        f.finish(false, t1);
        assert!(!f.due(t1 + RETRY - Duration::from_secs(1), 0));
        assert!(f.due(t1 + RETRY, 0));
    }

    #[test]
    fn resume_makes_weather_due_at_once() {
        let t0 = Instant::now();
        let mut f = Freshness::default();
        f.start(t0, 0);
        f.finish(true, t0);
        assert!(!f.due(t0 + Duration::from_secs(10), 0));
        assert!(f.due(t0 + Duration::from_secs(10), 1));
        f.start(t0 + Duration::from_secs(10), 1);
        assert!(!f.due(t0 + Duration::from_secs(11), 1));
    }

    #[test]
    fn wanted_places_are_deduplicated() {
        use crate::layout::tests::real_monitors;
        let mut l = Layout::default_for(&real_monitors());
        let paris = serde_json::json!({ "place": { "name": "Paris", "lat": 48.8566, "lon": 2.3522 } });
        let ids: Vec<String> =
            ["main", "top"].iter().map(|bar| l.add_widget(bar, crate::layout::Zone::Right, WidgetKind::WEATHER).unwrap()).collect();
        assert!(wanted_places(&l).is_empty());
        for id in &ids {
            l.set_widget_settings(id, paris.clone()).unwrap();
        }
        let places = wanted_places(&l);
        assert_eq!(places.len(), 1);
        assert_eq!(place_key(&places[0]), "48.857,2.352");
        // Les autres villes d'un widget sont suivies aussi.
        let two = serde_json::json!({ "place": paris["place"], "others": [{ "name": "Lyon", "lat": 45.76, "lon": 4.84 }] });
        l.set_widget_settings(&ids[0], two).unwrap();
        let names: Vec<String> = wanted_places(&l).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Paris", "Lyon"]);
    }

    #[test]
    fn a_widget_follows_up_to_three_places() {
        let place = |name: &str, lat: f64| Place { name: name.into(), detail: String::new(), lat, lon: 2.0 };
        let all = [place("Paris", 48.0), place("Lyon", 45.0), place("Lille", 50.0), place("Brest", 48.4)];
        // Réglages d'avant (une seule ville), ou vides.
        let old = serde_json::json!({ "place": all[0] });
        assert_eq!(places_of(&old), all[..1]);
        assert!(places_of(&Value::Null).is_empty());
        // Aller-retour : la première est la principale ; pas plus de trois, pas de doublon.
        let s = places_settings(&all);
        assert_eq!(s["place"]["name"], "Paris");
        assert_eq!(places_of(&s), all[..3]);
        let twice = serde_json::json!({ "place": all[0], "others": [all[0], { "name": "illisible" }, all[1]] });
        assert_eq!(places_of(&twice), all[..2]);
        // Plus aucune ville : la principale est effacée (fusionnée dans les réglages).
        let none = places_settings(&[]);
        assert!(none["place"].is_null() && places_of(&none).is_empty());
    }
}
