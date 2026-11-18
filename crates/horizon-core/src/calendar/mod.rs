//! Agenda : lecture de calendriers iCalendar (adresse secrète iCal de Google Agenda, etc.) et
//! événements d'une fenêtre de temps, répétitions comprises.
//!
//! Pris en charge : VEVENT avec DTSTART / DTEND / DURATION (heure UTC, fuseau TZID IANA,
//! heure flottante = locale, journée entière), RRULE (crate `rrule`), EXDATE, occurrences
//! modifiées (RECURRENCE-ID), événements annulés (STATUS:CANCELLED) ignorés.

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

mod ical;

use ical::{Prop, VEvent, parse_duration, parse_time, time_of, video_link, vevents};

/// Occurrence d'un événement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CalEvent {
    pub title: String,
    pub location: Option<String>,
    /// Début et fin en millisecondes Unix.
    pub start_ms: i64,
    pub end_ms: i64,
    pub all_day: bool,
    /// Lien de visioconférence trouvé dans le lieu ou la description.
    pub link: Option<String>,
    /// Rang du calendrier source (couleur).
    pub calendar: usize,
}

/// Débuts d'un événement répété dans [from, to[ (au plus 500).
fn occurrences(ev: &VEvent, dtstart: &Prop, from: DateTime<Utc>, to: DateTime<Utc>, len: Duration) -> Vec<DateTime<Utc>> {
    let Some(rrule) = ev.get("RRULE") else { return vec![] };
    // Journée entière : la crate attend une date et heure, minuit local convient.
    let start_line = if dtstart.param("VALUE") == Some("DATE") || dtstart.value.trim().len() == 8 {
        format!("DTSTART:{}T000000", dtstart.value.trim())
    } else {
        Prop { params: dtstart.params.iter().filter(|(k, _)| k == "TZID").cloned().collect(), ..dtstart.clone() }.line()
    };
    let spec = format!("{start_line}\nRRULE:{}", rrule.value.trim());
    let set: rrule::RRuleSet = match spec.parse() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("répétition illisible ({e}) : {spec}");
            return vec![];
        }
    };
    // Une occurrence commencée avant `from` mais pas finie compte aussi.
    let after = (from - len - Duration::seconds(1)).with_timezone(&rrule::Tz::UTC);
    let before = to.with_timezone(&rrule::Tz::UTC);
    set.after(after).before(before).all(500).dates.into_iter().map(|d| d.with_timezone(&Utc)).collect()
}

/// Nom donné à l'agenda par son service (`X-WR-CALNAME`), s'il en a un.
pub fn calendar_name(text: &str) -> Option<String> {
    ical::unfold(text)
        .iter()
        .take_while(|l| !l.starts_with("BEGIN:VEVENT"))
        .filter_map(|l| ical::parse_prop(l))
        .find(|p| p.name == "X-WR-CALNAME")
        .map(|p| ical::unescape(p.value.trim()))
        .filter(|n| !n.is_empty())
}

/// Événements de calendriers (`(rang, texte iCalendar)`) qui touchent [from, to[, triés.
pub fn events_between(calendars: &[(usize, String)], from: DateTime<Utc>, to: DateTime<Utc>) -> Vec<CalEvent> {
    let mut out = Vec::new();
    for (rank, text) in calendars {
        let evs = vevents(text);
        // Occurrences modifiées : (UID, début d'origine) remplacés par leur version.
        let overridden: HashSet<(String, i64)> = evs
            .iter()
            .filter_map(|e| {
                let uid = e.get("UID")?.value.clone();
                let rid = time_of(e.get("RECURRENCE-ID")?)?.0;
                Some((uid, rid.timestamp()))
            })
            .collect();
        for ev in &evs {
            if ev.get("STATUS").is_some_and(|s| s.value.trim().eq_ignore_ascii_case("CANCELLED")) {
                continue;
            }
            let Some(dtstart) = ev.get("DTSTART") else { continue };
            let Some((start, all_day)) = time_of(dtstart) else { continue };
            let end = ev
                .get("DTEND")
                .and_then(time_of)
                .map(|(e, _)| e)
                .or_else(|| ev.get("DURATION").and_then(|d| parse_duration(&d.value)).map(|d| start + d))
                .unwrap_or(start + if all_day { Duration::days(1) } else { Duration::zero() });
            let len = (end - start).max(Duration::zero());
            let uid = ev.get("UID").map(|u| u.value.clone()).unwrap_or_default();
            let excluded: HashSet<i64> = ev
                .all("EXDATE")
                .flat_map(|p| {
                    p.value
                        .split(',')
                        .filter_map(|v| parse_time(v, p.param("TZID"), p.param("VALUE") == Some("DATE")))
                        .map(|(t, _)| t.timestamp())
                        .collect::<Vec<_>>()
                })
                .collect();
            let starts = if ev.get("RRULE").is_some() && ev.get("RECURRENCE-ID").is_none() {
                occurrences(ev, dtstart, from, to, len)
                    .into_iter()
                    .filter(|s| !excluded.contains(&s.timestamp()) && !overridden.contains(&(uid.clone(), s.timestamp())))
                    .collect()
            } else {
                vec![start]
            };
            let title = ev.text("SUMMARY").unwrap_or_else(|| "(Sans titre)".into());
            let location = ev.text("LOCATION");
            let link = location
                .as_deref()
                .and_then(video_link)
                .or_else(|| ev.text("DESCRIPTION").as_deref().and_then(video_link))
                .or_else(|| ev.text("X-GOOGLE-CONFERENCE").filter(|l| l.starts_with("https://")));
            for s in starts {
                let e = s + len;
                // Touche la fenêtre : commence avant sa fin et finit après son début.
                if s < to && (e > from || (len.is_zero() && s >= from)) {
                    out.push(CalEvent {
                        title: title.clone(),
                        location: location.clone(),
                        start_ms: s.timestamp_millis(),
                        end_ms: e.timestamp_millis(),
                        all_day,
                        link: link.clone(),
                        calendar: *rank,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (a.start_ms, !a.all_day, &a.title).cmp(&(b.start_ms, !b.all_day, &b.title)));
    // Même événement présent dans deux calendriers partagés : une seule fois.
    let mut seen = HashMap::new();
    out.retain(|e| seen.insert((e.title.clone(), e.start_ms, e.end_ms), ()).is_none());
    out
}

#[cfg(test)]
mod tests;
