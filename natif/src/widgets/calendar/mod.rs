//! Widget Agenda : adresses iCal secrètes lues hors du fil de l'interface, gardées en cache
//! (une barre par écran montre les mêmes) et relues toutes les 10 min ou après une sortie de
//! veille. Dans la barre : l'événement en cours ou le prochain (dans les 12 h), le temps restant
//! revu toutes les 30 s. Panneau (`panel.rs`) : les prochains jours, propres à l'exemplaire.
//!
//! Repris de `app/src-tauri/src/widgets/calendar.rs`. Différence : la page de chaque barre
//! demandait ses événements (`calendar_events`) toutes les 10 min ; ici un seul fil les lit pour
//! tous les exemplaires (une fois par liste d'adresses) et les envoie aux barres
//! (`horizon://calendar`), qui les gardent ; une liste d'adresses pas encore lue (agenda ajouté
//! dans les réglages) l'est aussitôt (`request`). Bouton « Tester » des réglages :
//! `calendar_test`.

mod panel;

use crate::host::{self, AppHandle, Emitter, Manager};
use crate::state::AppState;
use crate::ui::{BarWindow, CalendarData, CalendarItem, panels};
use crate::util::{lock, spawn_named, start_thread};
use crate::web::get;
use crate::widgets::Backend;
use chrono::{Duration as ChronoDuration, Utc};
use horizon_core::calendar::{CalEvent, calendar_name, events_between};
use horizon_core::layout::{BarLayout, WidgetKind};
use horizon_core::weather::Freshness;
use serde::Serialize;
use serde_json::Value;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Événements des agendas d'une liste d'adresses (`CalendarUpdate`).
pub const EV_CALENDAR: &str = "horizon://calendar";

const CALENDAR_TTL: Duration = Duration::from_secs(10 * 60);
/// Après un échec, l'adresse est retentée au plus à ce rythme (l'erreur reste affichée).
const CALENDAR_RETRY: Duration = Duration::from_secs(60);
/// Calendrier plus gros que ça : refusé (adresse qui ne mène pas à un agenda).
const MAX_ICS: u64 = 20 * 1024 * 1024;
/// Nombre de couleurs d'agenda (`Theme.calendars`).
const CAL_COLORS: usize = 6;
/// Événement mis en avant s'il commence dans moins de 12 h.
const HORIZON_MS: i64 = 12 * 3_600_000;
/// « Bientôt » : moins de 15 min avant le début (clic = rejoindre la visio).
const SOON_MS: i64 = 15 * 60_000;
/// Revue du temps restant.
const REFRESH: Duration = Duration::from_secs(30);

pub const BACKEND: Backend = Backend {
    id: "calendar",
    name: "agenda",
    manage: |app| {
        app.manage(Calendars::default());
    },
    start: Some(start),
    bind: Some(bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};

/// Agenda par adresse, et dernière vue de chaque liste d'adresses.
#[derive(Default)]
pub struct Calendars {
    cals: Mutex<HashMap<String, Calendar>>,
    views: Mutex<HashMap<Vec<String>, CalendarView>>,
    /// Listes d'adresses en cours de lecture hors du rythme des 10 min (`request`).
    asked: Mutex<std::collections::HashSet<Vec<String>>>,
}

/// Dernière lecture réussie d'un agenda (texte iCalendar), dernière erreur, et quand relire.
struct Calendar {
    fresh: Freshness,
    text: Option<String>,
    error: Option<String>,
}

impl Calendar {
    fn new() -> Calendar {
        Calendar { fresh: Freshness::with(CALENDAR_TTL, CALENDAR_RETRY), text: None, error: None }
    }
}

#[derive(Clone, Serialize)]
pub struct CalendarView {
    pub events: Vec<CalEvent>,
    /// Une erreur par adresse illisible (texte court, sans l'adresse secrète).
    pub errors: Vec<String>,
}

/// Vue d'une liste d'adresses, envoyée aux barres.
#[derive(Clone, Serialize)]
pub struct CalendarUpdate {
    pub urls: Vec<String>,
    pub view: CalendarView,
}

/// Adresse d'agenda utilisable : `webcal://` devient `https://`.
fn normalize(url: &str) -> Option<String> {
    let u = url.trim();
    let u = u.strip_prefix("webcal://").map_or_else(|| u.to_owned(), |r| format!("https://{r}"));
    (u.starts_with("https://") || u.starts_with("http://")).then_some(u)
}

/// Événements d'hier à dans 8 jours (bloquant : lit les agendas dus).
pub fn calendar_events(app: &AppHandle, urls: &[String]) -> CalendarView {
    let mut texts = Vec::new();
    let mut errors = Vec::new();
    let cals = &app.state::<Calendars>().cals;
    for (rank, raw) in urls.iter().enumerate() {
        let Some(url) = normalize(raw) else {
            errors.push(format!("Agenda {} : adresse invalide", rank + 1));
            continue;
        };
        // Une lecture à la fois par adresse, refaite après `CALENDAR_TTL` ou une veille.
        let due = {
            let mut cals = lock(cals);
            let e = cals.entry(url.clone()).or_insert_with(Calendar::new);
            let (now, epoch) = (Instant::now(), crate::resume::epoch());
            e.fresh.due(now, epoch).then(|| e.fresh.start(now, epoch)).is_some()
        };
        if due {
            let result = match get(&url, MAX_ICS) {
                Ok(text) if text.contains("BEGIN:VCALENDAR") => Ok(text),
                Ok(_) => Err("ce n’est pas un agenda iCal".to_owned()),
                Err(e) => Err(e),
            };
            let mut cals = lock(cals);
            let e = cals.entry(url.clone()).or_insert_with(Calendar::new);
            e.fresh.finish(result.is_ok(), Instant::now());
            match result {
                Ok(text) => (e.text, e.error) = (Some(text), None),
                Err(err) => e.error = Some(err),
            }
        }
        let cals = lock(cals);
        if let Some(e) = cals.get(&url) {
            if let Some(err) = &e.error {
                errors.push(format!("Agenda {} : {err}", rank + 1));
            }
            // Une adresse en échec garde sa dernière lecture réussie.
            if let Some(text) = &e.text {
                texts.push((rank, text.clone()));
            }
        }
    }
    let now = Utc::now();
    let events = events_between(&texts, now - ChronoDuration::days(1), now + ChronoDuration::days(8));
    CalendarView { events, errors }
}

/// Lit tout de suite une liste d'adresses dont la vue n'est pas encore connue (agenda ajouté ou
/// retiré dans les réglages), hors du fil de l'interface ; le résultat part aux barres.
fn request(app: &AppHandle, urls: &[String]) {
    let calendars = app.state::<Calendars>();
    if urls.is_empty() || lock(&calendars.views).contains_key(urls) || !lock(&calendars.asked).insert(urls.to_vec()) {
        return;
    }
    let (app, urls) = (app.clone(), urls.to_vec());
    spawn_named("agenda-lecture", move || {
        let view = calendar_events(&app, &urls);
        let calendars = app.state::<Calendars>();
        lock(&calendars.views).insert(urls.clone(), view.clone());
        lock(&calendars.asked).remove(&urls);
        let _ = app.emit(EV_CALENDAR, CalendarUpdate { urls, view });
    });
}

/// Résultat du bouton « Tester » d'un agenda.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalendarTest {
    /// Nom donné par le service (`X-WR-CALNAME`).
    pub name: Option<String>,
    /// Événements d'hier à dans 8 jours.
    pub events: usize,
}

/// Lit un agenda tout de suite, sans cache (réglages) : son nom et ses événements proches.
/// Bloquant : hors du fil de l'interface.
pub fn calendar_test(url: &str) -> Result<CalendarTest, String> {
    let url = normalize(url).ok_or("adresse invalide : elle doit commencer par https://, http:// ou webcal://")?;
    let text = get(&url, MAX_ICS)?;
    if !text.contains("BEGIN:VCALENDAR") {
        return Err("ce n’est pas un agenda iCal".into());
    }
    let now = Utc::now();
    let events = events_between(&[(0, text.clone())], now - ChronoDuration::days(1), now + ChronoDuration::days(8)).len();
    Ok(CalendarTest { name: calendar_name(&text), events })
}

/// Lit les agendas de tous les exemplaires (une fois par liste d'adresses), toutes les 10 min
/// et tout de suite à la sortie de veille.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("calendar", move || {
        loop {
            let lists: Vec<Vec<String>> = app.state::<AppState>().read(|l| {
                let mut v: Vec<Vec<String>> = Vec::new();
                for w in l.widgets_of(WidgetKind::CALENDAR) {
                    let urls = urls_of(&w.settings);
                    if !urls.is_empty() && !v.contains(&urls) {
                        v.push(urls);
                    }
                }
                v
            });
            for urls in lists {
                let view = calendar_events(&app, &urls);
                lock(&app.state::<Calendars>().views).insert(urls.clone(), view.clone());
                let _ = app.emit(EV_CALENDAR, CalendarUpdate { urls, view });
            }
            // Un peu plus que la durée du cache : la lecture suivante est due.
            crate::resume::sleep(CALENDAR_TTL + Duration::from_secs(5));
        }
    })
}

/// Adresses enregistrées dans les réglages du widget (`settings.urls`).
pub(crate) fn urls_of(settings: &Value) -> Vec<String> {
    settings["urls"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).filter(|u| !u.trim().is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Couleur (rang dans la palette) d'un événement de l'agenda de rang `rank` : celle gardée pour
/// son adresse (`settings.colors`), sinon celle de son rang.
fn event_color(settings: &Value, rank: usize) -> usize {
    let stored = urls_of(settings)
        .get(rank)
        .and_then(|u| settings["colors"].get(u))
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    stored.unwrap_or(rank) % CAL_COLORS
}

/// Événement à afficher : celui en cours, sinon le prochain qui commence dans moins de
/// `horizon_ms` (journées entières exclues).
fn featured(events: &[CalEvent], now: i64, horizon_ms: i64) -> Option<&CalEvent> {
    let mut timed = events.iter().filter(|e| !e.all_day && e.end_ms > now);
    let running = timed.clone().find(|e| e.start_ms <= now);
    running.or_else(|| timed.find(|e| e.start_ms - now <= horizon_ms))
}

/// « dans 25 min », « dans 2 h 10 », « en cours ».
fn relative(e: &CalEvent, now: i64) -> String {
    if e.start_ms <= now {
        return "en cours".into();
    }
    let min = ((e.start_ms - now) as f64 / 60_000.0).round().max(1.0) as i64;
    if min < 60 {
        return format!("dans {min} min");
    }
    let (h, m) = (min / 60, min % 60);
    if m > 0 { format!("dans {h} h {m:02}") } else { format!("dans {h} h") }
}

/// Un exemplaire : ses réglages et la dernière vue de ses agendas.
struct Row {
    settings: Value,
    urls: Vec<String>,
    view: Option<CalendarView>,
}

impl Row {
    fn item(&self, now: i64) -> CalendarItem {
        let mut it = CalendarItem { configured: !self.urls.is_empty(), ..Default::default() };
        let Some(v) = &self.view else { return it };
        it.error = !v.errors.is_empty();
        if let Some(e) = featured(&v.events, now, HORIZON_MS) {
            it.has_next = true;
            it.color = event_color(&self.settings, e.calendar) as i32;
            it.title = e.title.as_str().into();
            it.when = relative(e, now).into();
            it.soon = e.start_ms > now && e.start_ms - now <= SOON_MS;
            it.location = e.location.as_deref().unwrap_or_default().into();
            it.link = e.link.is_some();
        }
        it
    }
}

/// Exemplaires d'une barre.
struct BarRows {
    window: slint::Weak<BarWindow>,
    model: Rc<VecModel<CalendarItem>>,
    rows: Rc<RefCell<Vec<Row>>>,
}

thread_local! {
    static BARS: RefCell<Vec<BarRows>> = RefCell::default();
    static TIMER: slint::Timer = slint::Timer::default();
}

fn now_ms() -> i64 {
    horizon_core::time::now_ms() as i64
}

/// Revoit ce que montrent les exemplaires (temps restant, événement suivant).
fn refresh_all() {
    let now = now_ms();
    BARS.with_borrow_mut(|bars| {
        bars.retain(|b| b.window.upgrade().is_some());
        for b in bars.iter() {
            for (i, row) in b.rows.borrow().iter().enumerate() {
                let next = row.item(now);
                if b.model.row_data(i).as_ref() != Some(&next) {
                    b.model.set_row_data(i, next);
                }
            }
        }
    });
}

fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    let views = lock(&app.state::<Calendars>().views).clone();
    let rows: Vec<Row> = super::instances(bar, "calendar")
        .map(|w| {
            let urls = urls_of(&w.settings);
            if !views.contains_key(&urls) {
                request(app, &urls);
            }
            Row { view: views.get(&urls).cloned(), settings: w.settings.clone(), urls }
        })
        .collect();
    let now = now_ms();
    let model = Rc::new(VecModel::from(rows.iter().map(|r| r.item(now)).collect::<Vec<_>>()));
    let g = window.global::<CalendarData>();
    g.set_items(ModelRc::from(model.clone()));
    let rows_of = Rc::new(RefCell::new(rows));
    // Liaison refaite (disposition modifiée) : les rangées d'avant de cette barre partent.
    BARS.with_borrow_mut(|b| {
        b.retain(|x| x.window.upgrade().is_some_and(|w| !std::ptr::eq(w.window(), window.window())));
        b.push(BarRows { window: window.as_weak(), model, rows: rows_of.clone() });
    });
    TIMER.with(|t| {
        if !t.running() {
            t.start(slint::TimerMode::Repeated, REFRESH, refresh_all);
        }
    });

    let (l, rows) = (label.to_owned(), rows_of.clone());
    let a = app.clone();
    g.on_clicked(move |data, id| on_click(&a, &l, &rows.borrow(), data as usize, &id));

    host::listen(EV_CALENDAR, Some(label), move |e| {
        let Some(u) = e.payload::<CalendarUpdate>() else { return };
        for row in rows_of.borrow_mut().iter_mut().filter(|r| r.urls == u.urls) {
            row.view = Some(u.view.clone());
        }
        refresh_all();
    });
}

/// Clic : pas encore d'agenda → réglages ; visio qui commence (ou en cours) → la rejoindre ;
/// sinon le panneau épinglé.
fn on_click(app: &AppHandle, label: &str, rows: &[Row], data: usize, id: &str) {
    let now = now_ms();
    let Some(row) = rows.get(data) else { return };
    let link = row.view.as_ref().and_then(|v| featured(&v.events, now, HORIZON_MS)).and_then(|e| {
        let soon = e.start_ms > now && e.start_ms - now <= SOON_MS;
        e.link.clone().filter(|_| soon || e.start_ms <= now)
    });
    if row.urls.is_empty() {
        panels::settings(app, id);
    } else if let Some(url) = link {
        if let Err(e) = crate::web::url_open(&url) {
            log::warn!("agenda : {e}");
        }
    } else {
        panels::pin(label, &format!("calendar:{id}"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;
    const NOW: i64 = 1_794_000_000_000;

    fn ev(title: &str, start: i64, end: i64, all_day: bool) -> CalEvent {
        CalEvent { title: title.into(), location: None, start_ms: start, end_ms: end, all_day, link: None, calendar: 0 }
    }

    #[test]
    fn normalizes_calendar_addresses() {
        assert_eq!(normalize(" webcal://a.b/c.ics ").as_deref(), Some("https://a.b/c.ics"));
        assert_eq!(normalize("C:\\agenda.ics"), None);
    }

    #[test]
    fn features_the_running_event_else_the_next_close_one() {
        let list = [
            ev("Jour", NOW - 14 * H, NOW + 10 * H, true),
            ev("Réunion", NOW - H / 2, NOW + H / 2, false),
            ev("Sport", NOW + 5 * H, NOW + 6 * H, false),
        ];
        assert_eq!(featured(&list, NOW, HORIZON_MS).map(|e| e.title.as_str()), Some("Réunion"));
        assert_eq!(featured(&list[2..], NOW, HORIZON_MS).map(|e| e.title.as_str()), Some("Sport"));
        assert!(featured(&list[2..], NOW, 2 * H).is_none());
        assert!(featured(&list[..1], NOW, HORIZON_MS).is_none());
    }

    #[test]
    fn remaining_time() {
        assert_eq!(relative(&ev("a", NOW + 25 * 60_000, NOW + H, false), NOW), "dans 25 min");
        assert_eq!(relative(&ev("a", NOW + 2 * H + 5 * 60_000, NOW + 3 * H, false), NOW), "dans 2 h 05");
        assert_eq!(relative(&ev("a", NOW + 3 * H, NOW + 4 * H, false), NOW), "dans 3 h");
        assert_eq!(relative(&ev("a", NOW - 1, NOW + H, false), NOW), "en cours");
    }

    #[test]
    fn reads_addresses_and_colors() {
        let s = serde_json::json!({ "urls": ["https://a", " ", 3], "colors": { "https://a": 3 } });
        assert_eq!(urls_of(&s), ["https://a"]);
        assert!(urls_of(&Value::Null).is_empty());
        assert_eq!(event_color(&s, 0), 3);
        let old = serde_json::json!({ "urls": ["https://a", "https://b"] });
        assert_eq!(event_color(&old, 1), 1, "réglage d'avant : couleur du rang");
    }

    #[test]
    fn item_of_an_instance() {
        let mut e = ev("Point", NOW + 10 * 60_000, NOW + H, false);
        e.link = Some("https://meet".into());
        e.location = Some("Salle".into());
        let row = Row {
            settings: serde_json::json!({ "urls": ["https://a"] }),
            urls: vec!["https://a".into()],
            view: Some(CalendarView { events: vec![e], errors: vec!["Agenda 2 : x".into()] }),
        };
        let it = row.item(NOW);
        assert!(it.configured && it.has_next && it.soon && it.link && it.error);
        assert_eq!((it.title.as_str(), it.when.as_str(), it.location.as_str()), ("Point", "dans 10 min", "Salle"));
        let empty = Row { settings: Value::Null, urls: Vec::new(), view: None };
        assert!(!empty.item(NOW).configured);
    }
}
