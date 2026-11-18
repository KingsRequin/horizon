//! Tests de l'agenda.

use super::*;
use chrono::{NaiveDateTime, TimeZone};

fn utc(s: &str) -> DateTime<Utc> {
    Utc.from_utc_datetime(&NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap())
}

fn cal(body: &str) -> Vec<(usize, String)> {
    vec![(0, format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{body}END:VCALENDAR\r\n"))]
}

#[test]
fn simple_event_with_tzid_and_folded_lines() {
    let c = cal(
        "BEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Europe/Paris:20261112T100000\r\n\
         DTEND;TZID=Europe/Paris:20261112T113000\r\nSUMMARY:Réunion d'équipe\\, point\r\n  hebdo\r\n\
         LOCATION:https://meet.google.com/abc-defg-hij\r\nBEGIN:VALARM\r\nSUMMARY:rappel\r\nEND:VALARM\r\nEND:VEVENT\r\n",
    );
    let e = events_between(&c, utc("2026-11-12 00:00"), utc("2026-11-13 00:00"));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].title, "Réunion d'équipe, point hebdo");
    // 10:00 à Paris = 09:00 UTC en hiver.
    assert_eq!(e[0].start_ms, utc("2026-11-12 09:00").timestamp_millis());
    assert_eq!(e[0].end_ms, utc("2026-11-12 10:30").timestamp_millis());
    assert_eq!(e[0].link.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
    assert!(!e[0].all_day);
}

#[test]
fn weekly_rule_with_exdate_and_moved_occurrence() {
    let c = cal(
        "BEGIN:VEVENT\r\nUID:r\r\nDTSTART:20261102T180000Z\r\nDTEND:20261102T190000Z\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\nEXDATE:20261109T180000Z\r\nSUMMARY:Sport\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:r\r\nRECURRENCE-ID:20261116T180000Z\r\nDTSTART:20261117T180000Z\r\n\
         DTEND:20261117T190000Z\r\nSUMMARY:Sport (décalé)\r\nEND:VEVENT\r\n",
    );
    let e = events_between(&c, utc("2026-11-01 00:00"), utc("2026-11-30 00:00"));
    let got: Vec<(String, i64)> = e.iter().map(|e| (e.title.clone(), e.start_ms)).collect();
    assert_eq!(
        got,
        vec![
            ("Sport".into(), utc("2026-11-02 18:00").timestamp_millis()),
            ("Sport (décalé)".into(), utc("2026-11-17 18:00").timestamp_millis()),
            ("Sport".into(), utc("2026-11-23 18:00").timestamp_millis()),
        ]
    );
}

#[test]
fn running_occurrence_and_window_bounds() {
    let c = cal(
        "BEGIN:VEVENT\r\nUID:d\r\nDTSTART:20261111T080000Z\r\nDTEND:20261111T120000Z\r\n\
         RRULE:FREQ=DAILY;COUNT=3\r\nSUMMARY:Atelier\r\nEND:VEVENT\r\n",
    );
    // Fenêtre qui commence au milieu de l'occurrence du 12 : elle compte.
    let e = events_between(&c, utc("2026-11-12 10:00"), utc("2026-11-13 09:00"));
    assert_eq!(e.len(), 2);
    assert_eq!(e[0].start_ms, utc("2026-11-12 08:00").timestamp_millis());
    assert_eq!(e[1].start_ms, utc("2026-11-13 08:00").timestamp_millis());
}

#[test]
fn all_day_cancelled_and_duration() {
    let c = cal(
        "BEGIN:VEVENT\r\nUID:j\r\nDTSTART;VALUE=DATE:20261112\r\nDTEND;VALUE=DATE:20261113\r\nSUMMARY:Anniversaire\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:x\r\nDTSTART:20261112T100000Z\r\nDURATION:PT45M\r\nSUMMARY:Appel\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:c\r\nSTATUS:CANCELLED\r\nDTSTART:20261112T110000Z\r\nSUMMARY:Annulé\r\nEND:VEVENT\r\n",
    );
    let e = events_between(&c, utc("2026-11-11 00:00"), utc("2026-11-14 00:00"));
    assert_eq!(e.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Anniversaire", "Appel"]);
    assert!(e[0].all_day);
    assert_eq!(e[0].end_ms - e[0].start_ms, 86_400_000);
    assert_eq!(e[1].end_ms - e[1].start_ms, 45 * 60_000);
}

#[test]
fn duplicates_across_calendars_are_merged() {
    let ev = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s\r\nDTSTART:20261112T100000Z\r\nDTEND:20261112T110000Z\r\nSUMMARY:Cours\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let e = events_between(&[(0, ev.into()), (1, ev.into())], utc("2026-11-12 00:00"), utc("2026-11-13 00:00"));
    assert_eq!(e.len(), 1);
}

#[test]
fn durations_and_links() {
    assert_eq!(parse_duration("PT1H30M"), Some(Duration::minutes(90)));
    assert_eq!(parse_duration("P1DT2H"), Some(Duration::hours(26)));
    assert_eq!(parse_duration("-PT15M"), Some(Duration::minutes(-15)));
    assert_eq!(parse_duration("X"), None);
    assert_eq!(
        video_link("Rejoindre : <https://zoom.us/j/123>. Merci").as_deref(),
        Some("https://zoom.us/j/123")
    );
    assert_eq!(video_link("https://exemple.fr"), None);
}

#[test]
fn calendar_name_is_read_before_the_events() {
    let text = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Perso\\, famille\r\nBEGIN:VEVENT\r\nX-WR-CALNAME:non\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert_eq!(calendar_name(text).as_deref(), Some("Perso, famille"));
    assert_eq!(calendar_name("BEGIN:VCALENDAR\nEND:VCALENDAR\n"), None);
    assert_eq!(calendar_name("BEGIN:VCALENDAR\nX-WR-CALNAME: \nEND:VCALENDAR\n"), None);
}
