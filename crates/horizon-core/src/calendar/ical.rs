//! Lecture du format iCalendar : lignes dépliées, propriétés, dates et durées, événements.

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// Propriété iCalendar : nom, paramètres, valeur.
#[derive(Debug, Clone)]
pub(super) struct Prop {
    pub(super) name: String,
    pub(super) params: Vec<(String, String)>,
    pub(super) value: String,
}

impl Prop {
    pub(super) fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Ligne d'origine (pour la crate `rrule`).
    pub(super) fn line(&self) -> String {
        let mut s = self.name.clone();
        for (k, v) in &self.params {
            s.push_str(&format!(";{k}={v}"));
        }
        format!("{s}:{}", self.value)
    }
}

/// Lignes dépliées (une ligne qui commence par une espace prolonge la précédente).
pub(super) fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        match (line.strip_prefix([' ', '\t']), out.last_mut()) {
            (Some(rest), Some(prev)) => prev.push_str(rest),
            _ => out.push(line.to_owned()),
        }
    }
    out
}

pub(super) fn parse_prop(line: &str) -> Option<Prop> {
    // Le séparateur valeur est le premier « : » hors guillemets.
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?.0;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.to_ascii_uppercase();
    let params = parts
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_ascii_uppercase(), v.trim_matches('"').to_owned()))
        .collect();
    Some(Prop { name, params, value: value.to_owned() })
}

pub(super) fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(o) => out.push(o),
            None => {}
        }
    }
    out
}

/// Instant d'une valeur DTSTART / DTEND / EXDATE / RECURRENCE-ID ; `bool` = journée entière.
pub(super) fn parse_time(value: &str, tzid: Option<&str>, date_only: bool) -> Option<(DateTime<Utc>, bool)> {
    let v = value.trim();
    if date_only || v.len() == 8 {
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        let local = Local.from_local_datetime(&d.and_hms_opt(0, 0, 0)?).earliest()?;
        return Some((local.with_timezone(&Utc), true));
    }
    if let Some(utc) = v.strip_suffix('Z') {
        let n = NaiveDateTime::parse_from_str(utc, "%Y%m%dT%H%M%S").ok()?;
        return Some((Utc.from_utc_datetime(&n), false));
    }
    let n = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
    let t = match tzid.and_then(|z| z.parse::<chrono_tz::Tz>().ok()) {
        Some(tz) => tz.from_local_datetime(&n).earliest()?.with_timezone(&Utc),
        // Heure flottante ou fuseau inconnu (nom Windows) : heure locale.
        None => Local.from_local_datetime(&n).earliest()?.with_timezone(&Utc),
    };
    Some((t, false))
}

pub(super) fn time_of(p: &Prop) -> Option<(DateTime<Utc>, bool)> {
    parse_time(&p.value, p.param("TZID"), p.param("VALUE") == Some("DATE"))
}

/// Durée ISO 8601 simple (`PT1H30M`, `P1D`, `-PT15M`).
pub(super) fn parse_duration(v: &str) -> Option<Duration> {
    let (neg, v) = v.strip_prefix('-').map_or((false, v), |r| (true, r));
    let v = v.strip_prefix('+').unwrap_or(v).strip_prefix('P')?;
    let mut total = Duration::zero();
    let mut num = String::new();
    for c in v.chars() {
        match c {
            '0'..='9' => num.push(c),
            'T' => {}
            'W' | 'D' | 'H' | 'M' | 'S' => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match c {
                    'W' => Duration::weeks(n),
                    'D' => Duration::days(n),
                    'H' => Duration::hours(n),
                    'M' => Duration::minutes(n),
                    _ => Duration::seconds(n),
                };
            }
            _ => return None,
        }
    }
    Some(if neg { -total } else { total })
}

/// Premier lien de visioconférence d'un texte.
pub(super) fn video_link(text: &str) -> Option<String> {
    const HOSTS: [&str; 5] = ["meet.google.com/", "zoom.us/", "teams.microsoft.com/", "teams.live.com/", "discord.gg/"];
    text.split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '(' | ')' | ','))
        .map(|w| w.trim_end_matches(['.', ';']))
        .find(|w| w.starts_with("https://") && HOSTS.iter().any(|h| w.contains(h)))
        .map(str::to_owned)
}

pub(super) struct VEvent {
    pub(super) props: Vec<Prop>,
}

impl VEvent {
    pub(super) fn get(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name == name)
    }

    pub(super) fn all(&self, name: &str) -> impl Iterator<Item = &Prop> {
        self.props.iter().filter(move |p| p.name == name)
    }

    pub(super) fn text(&self, name: &str) -> Option<String> {
        self.get(name).map(|p| unescape(&p.value)).filter(|s| !s.trim().is_empty())
    }
}

pub(super) fn vevents(text: &str) -> Vec<VEvent> {
    let mut out = Vec::new();
    let mut current: Option<Vec<Prop>> = None;
    // Profondeur des composants imbriqués (VALARM) à ignorer.
    let mut nested = 0;
    for line in unfold(text) {
        let Some(p) = parse_prop(&line) else { continue };
        match (p.name.as_str(), p.value.trim()) {
            ("BEGIN", "VEVENT") => current = Some(Vec::new()),
            ("END", "VEVENT") => {
                if let Some(props) = current.take() {
                    out.push(VEvent { props });
                }
                nested = 0;
            }
            ("BEGIN", _) if current.is_some() => nested += 1,
            ("END", _) if current.is_some() && nested > 0 => nested -= 1,
            _ if nested == 0 => {
                if let Some(c) = current.as_mut() {
                    c.push(p);
                }
            }
            _ => {}
        }
    }
    out
}
