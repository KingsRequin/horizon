//! Panneau du quota Claude (global `ClaudeQuotaPanelData` de
//! `ui/widgets/claude_quota/panel.slint`) : textes de Panel.svelte et calculs de `quota.ts`
//! (rythme, date d'épuisement, remise à zéro). L'ouverture redemande le quota (au plus une fois
//! par minute, `quota_refresh`) ; les textes relatifs (« mis à jour il y a… », rythme) avancent
//! toutes les 30 s panneau ouvert. Fil de l'interface.

use super::ui::{level, parse_ms};
use super::{EV_QUOTA, QuotaPayload, quota_refresh, quota_state};
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{ClaudeQuotaPanelData, ClaudeQuotaPanelLimit, ClaudeQuotaPanelShare};
use crate::widgets::Panel;
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use horizon_core::quota::QuotaWindow;
use horizon_core::time::now_ms;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: Some(closed), ..Panel::new(bind) };

/// Cadence des textes relatifs (`every(30_000)`).
const TICK: Duration = Duration::from_secs(30);
const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

thread_local! {
    static LAST: RefCell<QuotaPayload> = RefCell::default();
    static LIMITS: Rc<VecModel<ClaudeQuotaPanelLimit>> = Rc::new(VecModel::default());
    static SHARES: Rc<VecModel<ClaudeQuotaPanelShare>> = Rc::new(VecModel::default());
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Début de la fenêtre d'une limite : 5 h avant la remise à zéro, ou le début de la semaine.
fn window_start(w: &QuotaWindow, week_started_at: Option<&str>) -> Option<u64> {
    let reset = parse_ms(w.resets_at.as_deref()?)?;
    if w.kind == "five_hour" {
        return reset.checked_sub(5 * HOUR);
    }
    match week_started_at.and_then(parse_ms) {
        Some(s) => Some(s),
        None => reset.checked_sub(7 * DAY),
    }
}

/// Rythme d'une limite (`pace`) : part du temps écoulée (0–100) et instant où la limite serait
/// atteinte au rythme moyen (`None` : elle tient jusqu'à la remise à zéro). `None` sans dates
/// exploitables.
#[derive(Debug, PartialEq)]
pub struct Pace {
    pub elapsed_pct: f64,
    pub exhausted_at: Option<u64>,
}

pub fn pace(w: &QuotaWindow, week_started_at: Option<&str>, now: u64) -> Option<Pace> {
    let start = window_start(w, week_started_at)?;
    let reset = parse_ms(w.resets_at.as_deref()?)?;
    if reset <= start || now <= start || now >= reset {
        return None;
    }
    let (span, elapsed) = ((reset - start) as f64, (now - start) as f64);
    let elapsed_pct = elapsed / span * 100.0;
    let used = w.used_pct as f64;
    if used >= 100.0 {
        return Some(Pace { elapsed_pct, exhausted_at: Some(now) });
    }
    if used <= 0.0 {
        return Some(Pace { elapsed_pct, exhausted_at: None });
    }
    let at = start as f64 + elapsed * 100.0 / used;
    Some(Pace { elapsed_pct, exhausted_at: (at < reset as f64).then_some(at as u64) })
}

const WEEKDAYS: [&str; 7] = ["lun.", "mar.", "mer.", "jeu.", "ven.", "sam.", "dim."];
const MONTHS: [&str; 12] =
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];

/// « lun. 16 nov. à 16:00 », « aujourd’hui à 16:00 », « demain à 16:00 » (heure locale).
fn when_local(at: NaiveDateTime, today: NaiveDate) -> String {
    let d = at.date();
    let prefix = if d == today {
        "aujourd’hui".to_owned()
    } else if today.succ_opt() == Some(d) {
        "demain".to_owned()
    } else {
        format!("{} {} {}", WEEKDAYS[d.weekday().num_days_from_monday() as usize], d.day(), MONTHS[d.month0() as usize])
    };
    format!("{prefix} à {:02}:{:02}", at.hour(), at.minute())
}

fn local(ms: u64) -> Option<NaiveDateTime> {
    Local.timestamp_millis_opt(ms as i64).single().map(|t| t.naive_local())
}

/// `when` de quota.ts.
fn when(ms: u64, now: u64) -> String {
    match (local(ms), local(now)) {
        (Some(at), Some(n)) => when_local(at, n.date()),
        _ => String::new(),
    }
}

fn round(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// Ligne du rythme (`paceLine`).
fn pace_line(w: &QuotaWindow, week_started_at: Option<&str>, now: u64) -> String {
    let Some(p) = pace(w, week_started_at, now) else { return String::new() };
    let head = format!("{} % utilisés pour {} % du temps écoulé", round(w.used_pct as f64), round(p.elapsed_pct));
    match p.exhausted_at {
        None => format!("{head} · tient jusqu’à la remise à zéro"),
        Some(t) if t <= now => format!("{head} · plafond atteint"),
        Some(t) => format!("{head} · plafond vers {}", when(t, now)),
    }
}

fn title(w: &QuotaWindow) -> String {
    match w.kind {
        "seven_day" => "Semaine · tous modèles".into(),
        "five_hour" => "Session de 5 h".into(),
        _ => w.label.clone(),
    }
}

fn limit(w: &QuotaWindow, week_started_at: Option<&str>, now: u64) -> ClaudeQuotaPanelLimit {
    let reset = w
        .resets_at
        .as_deref()
        .and_then(parse_ms)
        .map(|r| format!("Remise à zéro {} · {}", when(r, now), crate::ui::time::until(r, now)))
        .unwrap_or_default();
    let share = w
        .share
        .map(|s| {
            format!(
                "{} % de la semaine générale consommés sur {} % alloués",
                round(s.used_pct as f64),
                round(s.allowance_pct as f64)
            )
        })
        .unwrap_or_default();
    ClaudeQuotaPanelLimit {
        title: title(w).into(),
        model: w.kind == "model",
        pct: format!("{} %", round(w.used_pct as f64)).into(),
        used: w.used_pct / 100.0,
        level: level(w),
        reset: reset.into(),
        share: share.into(),
        pace: pace_line(w, week_started_at, now).into(),
    }
}

fn show() {
    let now = now_ms();
    LAST.with_borrow(|p| {
        let (limits, shares) = match &p.quota {
            Some(q) => (
                q.windows.iter().map(|w| limit(w, q.week_started_at.as_deref(), now)).collect(),
                q.breakdown
                    .iter()
                    .map(|r| ClaudeQuotaPanelShare {
                        label: if r.label == "Other" { "Autre".into() } else { r.label.as_str().into() },
                        pct: format!("{} %", round(r.pct as f64)).into(),
                        value: r.pct / 100.0,
                        zero: r.pct <= 0.0,
                    })
                    .collect(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        let updated: slint::SharedString =
            p.fetched_at_ms.map(|t| format!("mis à jour {}", crate::ui::time::ago(t, now))).unwrap_or_default().into();
        let extra = p.quota.as_ref().map(|q| {
            let text = format!(
                "Crédits supplémentaires {}",
                if q.extra_enabled { "activés : l’usage continue au-delà des limites" } else { "désactivés" }
            );
            (q.breakdown.iter().any(|r| r.pct > 0.0), slint::SharedString::from(text))
        });
        panel_hosts::each("claude_quota", |h| {
            let g = h.global::<ClaudeQuotaPanelData>();
            g.set_updated(updated.clone());
            g.set_error(p.error.as_deref().unwrap_or_default().into());
            g.set_has_quota(p.quota.is_some());
            if let Some((breakdown, extra)) = &extra {
                g.set_has_breakdown(*breakdown);
                g.set_extra(extra.clone());
            }
        });
        LIMITS.with(|m| crate::ui::model::sync(m, limits));
        SHARES.with(|m| crate::ui::model::sync(m, shares));
    });
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<ClaudeQuotaPanelData>();
    g.set_limits(ModelRc::from(LIMITS.with(Rc::clone)));
    g.set_breakdown(ModelRc::from(SHARES.with(Rc::clone)));
    LAST.set(quota_state(app.clone()));
    show();
    if win.is_popup() {
        host::listen(EV_QUOTA, Some(LABEL), |e| {
            if let Some(p) = e.payload::<QuotaPayload>() {
                LAST.set(p.clone());
                show();
            }
        });
    }
}

/// Ouverture : quota redemandé (au plus une fois par minute), textes relatifs à jour.
fn opened(app: &AppHandle, _kind: &str) {
    quota_refresh(app.clone());
    show();
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, TICK, show));
}

fn closed(_app: &AppHandle, _kind: &str) {
    TIMER.with(slint::Timer::stop);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: &'static str, used: f32, resets: &str) -> QuotaWindow {
        QuotaWindow { label: "x".into(), kind, used_pct: used, resets_at: Some(resets.into()), severity: None, share: None }
    }

    #[test]
    fn pace_follows_quota_ts() {
        let reset = "2026-11-17T15:00:00Z";
        let r = parse_ms(reset).unwrap();
        // Session de 5 h : commencée à 10:00 ; à 12:30, moitié du temps.
        let now = r - 2 * HOUR - HOUR / 2;
        let p = pace(&window("five_hour", 25.0, reset), None, now).unwrap();
        assert!((p.elapsed_pct - 50.0).abs() < 1e-9);
        assert_eq!(p.exhausted_at, None, "25 % pour 50 % du temps : tient");
        let p = pace(&window("five_hour", 100.0, reset), None, now).unwrap();
        assert_eq!(p.exhausted_at, Some(now));
        // 80 % en 2 h 30 : plafond 3 h 07 30 après le début.
        let p = pace(&window("five_hour", 80.0, reset), None, now).unwrap();
        assert_eq!(p.exhausted_at, Some(r - 5 * HOUR + 3 * HOUR + 7 * 60_000 + 30_000));
        // Hors de la fenêtre, ou sans date : rien.
        assert!(pace(&window("five_hour", 10.0, reset), None, r + 1).is_none());
        // Semaine : début donné.
        let p = pace(&window("seven_day", 10.0, reset), Some("2026-11-10T15:00:00Z"), r - DAY * 7 / 2).unwrap();
        assert!((p.elapsed_pct - 50.0).abs() < 1e-9);
    }

    #[test]
    fn when_follows_quota_ts() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 17).unwrap();
        let at = |d: u32, h: u32| NaiveDate::from_ymd_opt(2026, 11, d).unwrap().and_hms_opt(h, 0, 0).unwrap();
        assert_eq!(when_local(at(17, 16), today), "aujourd’hui à 16:00");
        assert_eq!(when_local(at(18, 3), today), "demain à 03:00");
        assert_eq!(when_local(at(20, 10), today), "ven. 20 nov. à 10:00");
    }

    #[test]
    fn titles() {
        assert_eq!(title(&window("seven_day", 0.0, "")), "Semaine · tous modèles");
        assert_eq!(title(&window("five_hour", 0.0, "")), "Session de 5 h");
        assert_eq!(title(&window("model", 0.0, "")), "x");
    }
}
