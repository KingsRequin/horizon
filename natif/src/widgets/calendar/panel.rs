//! Panneau Agenda (global `CalendarPanelData` de `ui/widgets/calendar/panel.slint`), propre à
//! l'exemplaire : à l'affichage, ses réglages relus (`widget_get`) et ses agendas lus hors du
//! fil de l'interface (`calendar_events`, cache de 10 min) ; regroupement par jour revu toutes
//! les 30 s panneau montré (`every(30_000)` de la version Tauri), et à chaque relecture des
//! agendas par le fil du widget. Chaque fenêtre (surgissante, tuile) a son exemplaire. Fil de
//! l'interface.

use super::{CalEvent, CalendarUpdate, CalendarView, EV_CALENDAR, event_color, urls_of};
use crate::host::{self, AppHandle, Manager};
use crate::popup::LABEL;
use crate::state::AppState;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{CalendarPanelData, CalendarPanelRow, model};
use crate::widgets::Panel;
use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

pub(super) const PANEL: Panel =
    Panel { opened: Some(opened), closed: Some(closed), shown: Some(shown), hidden: Some(hidden), ..Panel::new(bind) };

/// Revue de l'agenda, panneau montré.
const REFRESH: Duration = Duration::from_secs(30);
/// Événements montrés au plus.
const MAX_EVENTS: usize = 8;

/// Exemplaire montré par une fenêtre : identifiant, réglages, adresses, dernière vue de ses
/// agendas, et ses listes.
struct Shown {
    id: String,
    settings: Value,
    urls: Vec<String>,
    view: Option<CalendarView>,
    rows: Rc<VecModel<CalendarPanelRow>>,
    errors: Rc<VecModel<slint::SharedString>>,
}

thread_local! {
    /// Par fenêtre (étiquette).
    static SHOWN: RefCell<HashMap<String, Shown>> = RefCell::default();
    static TICK: slint::Timer = slint::Timer::default();
}

fn bind(_app: &AppHandle, win: &Host) {
    win.global::<CalendarPanelData>().on_join(|url| {
        if let Err(e) = crate::web::url_open(&url) {
            log::warn!("agenda : {e}");
        }
    });
    // Agendas relus par le fil du widget pendant que le panneau est montré : une seule fois,
    // pour chaque fenêtre qui montre ces adresses.
    if win.is_popup() {
        host::listen(EV_CALENDAR, Some(LABEL), |e| {
            let Some(u) = e.payload::<CalendarUpdate>() else { return };
            let mine: Vec<String> = SHOWN.with_borrow_mut(|all| {
                all.iter_mut()
                    .filter(|(_, s)| s.urls == u.urls)
                    .map(|(l, s)| {
                        s.view = Some(u.view.clone());
                        l.clone()
                    })
                    .collect()
            });
            for l in mine {
                show(&l);
            }
        });
    }
}

/// Une fenêtre montre `calendar:<id>` : réglages de l'exemplaire relus, agendas lus en
/// arrière-plan.
fn shown(app: &AppHandle, win: &Host, kind: &str) {
    let id = crate::popup::instance_of(kind).to_owned();
    let settings = app.state::<AppState>().read(|l| l.widget(&id).map(|w| w.settings.clone())).unwrap_or(Value::Null);
    let urls = urls_of(&settings);
    // Vue gardée par le fil du widget pour ces adresses : affichée tout de suite.
    let cached = crate::util::lock(&app.state::<super::Calendars>().views).get(&urls).cloned();
    let (rows, errors): (Rc<VecModel<CalendarPanelRow>>, Rc<VecModel<slint::SharedString>>) = Default::default();
    let g = win.global::<CalendarPanelData>();
    g.set_rows(ModelRc::from(rows.clone()));
    g.set_errors(ModelRc::from(errors.clone()));
    g.set_instance(id.as_str().into());
    g.set_loaded(false);
    let label = win.label.clone();
    SHOWN.with_borrow_mut(|all| {
        all.insert(label.clone(), Shown { id: id.clone(), settings, urls: urls.clone(), view: cached, rows, errors })
    });
    show(&label);
    let a = app.clone();
    crate::util::spawn_named("agenda-panneau", move || {
        let view = super::calendar_events(&a, &urls);
        let _ = slint::invoke_from_event_loop(move || {
            let mine = SHOWN.with_borrow_mut(|all| match all.get_mut(&label) {
                Some(s) if s.id == id => {
                    s.view = Some(view);
                    true
                }
                _ => false,
            });
            if mine {
                if let Some(h) = panel_hosts::host(&label) {
                    h.global::<CalendarPanelData>().set_loaded(true);
                }
                show(&label);
            }
        });
    });
}

fn hidden(_app: &AppHandle, win: &Host, _kind: &str) {
    SHOWN.with_borrow_mut(|all| all.remove(&win.label));
}

/// Panneau montré quelque part : regroupement par jour revu régulièrement.
fn opened(_app: &AppHandle, _kind: &str) {
    TICK.with(|t| t.start(slint::TimerMode::Repeated, REFRESH, show_all));
}

fn closed(_app: &AppHandle, _kind: &str) {
    TICK.with(slint::Timer::stop);
}

fn show_all() {
    let labels: Vec<String> = SHOWN.with_borrow(|all| all.keys().cloned().collect());
    for l in labels {
        show(&l);
    }
}

/// Pose les lignes de l'exemplaire montré par la fenêtre `label`, à l'instant.
fn show(label: &str) {
    let now = horizon_core::time::now_ms() as i64;
    SHOWN.with_borrow(|all| {
        let Some(s) = all.get(label) else { return };
        let (rows, errors) = match &s.view {
            Some(v) => (rows(&v.events, &s.settings, now, &Local), v.errors.iter().map(|e| e.as_str().into()).collect()),
            None => (Vec::new(), Vec::new()),
        };
        model::sync(&s.rows, rows);
        model::sync(&s.errors, errors);
    });
}

/// « 09:30 » (`toLocaleTimeString('fr-FR', { hour: '2-digit', minute: '2-digit' })`).
fn hhmm<Tz: TimeZone>(ms: i64, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    tz.timestamp_millis_opt(ms).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
}

/// Libellé d'un jour : « Aujourd’hui », « Demain », sinon « jeudi 13 novembre ».
fn day_title<Tz: TimeZone>(ms: i64, now: i64, tz: &Tz) -> String {
    let day = |t: i64| tz.timestamp_millis_opt(t).single().map(|d: DateTime<Tz>| d.date_naive());
    let (Some(d), Some(today)) = (day(ms), day(now)) else { return String::new() };
    match (d - today).num_days() {
        0 => "Aujourd’hui".to_owned(),
        1 => "Demain".to_owned(),
        _ => crate::widgets::clock::date_fr(d),
    }
}

/// Événements à venir (ou en cours), au plus `MAX_EVENTS`, précédés du titre de leur jour.
fn rows<Tz: TimeZone>(events: &[CalEvent], settings: &Value, now: i64, tz: &Tz) -> Vec<CalendarPanelRow>
where
    Tz::Offset: std::fmt::Display,
{
    let mut out: Vec<CalendarPanelRow> = Vec::new();
    let mut last_day = None;
    for e in events.iter().filter(|e| e.end_ms > now).take(MAX_EVENTS) {
        // Une journée entière commencée hier compte pour aujourd'hui.
        let day = day_title(e.start_ms.max(now), now, tz);
        if last_day.as_ref() != Some(&day) {
            out.push(CalendarPanelRow { day: true, title: day.as_str().into(), ..Default::default() });
            last_day = Some(day);
        }
        let location = e.location.as_deref().filter(|l| !l.starts_with("http")).unwrap_or_default();
        out.push(CalendarPanelRow {
            day: false,
            title: e.title.as_str().into(),
            time: if e.all_day { "Journée".into() } else { format!("{} – {}", hhmm(e.start_ms, tz), hhmm(e.end_ms, tz)).into() },
            location: location.into(),
            color: event_color(settings, e.calendar) as i32,
            now: e.start_ms <= now && !e.all_day,
            link: e.link.as_deref().unwrap_or_default().into(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, NaiveDate};

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(3600).unwrap()
    }

    /// Le `day` novembre 2026 à `h:m`, heure de Paris (UTC+1).
    fn at(day: u32, h: u32, m: u32) -> i64 {
        tz().from_local_datetime(&NaiveDate::from_ymd_opt(2026, 11, day).unwrap().and_hms_opt(h, m, 0).unwrap())
            .unwrap()
            .timestamp_millis()
    }

    fn ev(title: &str, start: i64, end: i64, all_day: bool, calendar: usize) -> CalEvent {
        CalEvent { title: title.into(), location: None, start_ms: start, end_ms: end, all_day, link: None, calendar }
    }

    #[test]
    fn days_are_named() {
        let now = at(17, 15, 0);
        assert_eq!(day_title(at(17, 23, 0), now, &tz()), "Aujourd’hui");
        assert_eq!(day_title(at(18, 0, 30), now, &tz()), "Demain");
        assert_eq!(day_title(at(19, 9, 0), now, &tz()), "jeudi 19 novembre");
        assert_eq!(hhmm(at(17, 9, 5), &tz()), "09:05");
    }

    #[test]
    fn agenda_grouped_by_day() {
        let now = at(17, 15, 0);
        let mut meet = ev("Point", at(17, 15, 30), at(17, 16, 0), false, 1);
        meet.link = Some("https://meet.google.com/x".into());
        meet.location = Some("https://meet.google.com/x".into());
        let mut dentist = ev("Dentiste", at(18, 10, 30), at(18, 11, 0), false, 0);
        dentist.location = Some("12 rue des Lilas".into());
        let events = [
            ev("Passé", at(17, 9, 0), at(17, 10, 0), false, 0),
            ev("Anniversaire", at(17, 0, 0), at(18, 0, 0), true, 0),
            ev("Réunion", at(17, 14, 30), at(17, 15, 30), false, 0),
            meet,
            dentist,
        ];
        let settings = serde_json::json!({ "urls": ["https://a", "https://b"], "colors": { "https://b": 4 } });
        let r = rows(&events, &settings, now, &tz());
        let titles: Vec<&str> = r.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Aujourd’hui", "Anniversaire", "Réunion", "Point", "Demain", "Dentiste"]);
        assert!(r[0].day && !r[1].day);
        assert_eq!((r[1].time.as_str(), r[1].now), ("Journée", false), "journée entière jamais « en cours »");
        assert_eq!((r[2].time.as_str(), r[2].now), ("14:30 – 15:30", true));
        assert_eq!((r[3].color, r[3].location.as_str(), r[3].link.as_str()), (4, "", "https://meet.google.com/x"));
        assert_eq!(r[5].location.as_str(), "12 rue des Lilas");
    }

    #[test]
    fn at_most_eight_events() {
        let now = at(17, 8, 0);
        let events: Vec<CalEvent> = (0..12).map(|i| ev("e", at(17, 9, i), at(17, 9, i + 1), false, 0)).collect();
        assert_eq!(rows(&events, &Value::Null, now, &tz()).len(), 1 + MAX_EVENTS);
    }
}
