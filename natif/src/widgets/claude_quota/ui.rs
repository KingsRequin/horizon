//! Partie interface du Quota Claude (`ui/widgets/claude_quota/widget.slint`) : jauges et
//! prochaine remise à zéro (`ClaudeQuotaData`), règles de `quota.ts` (couleur, limites
//! montrées). L'heure de remise à zéro avance chaque minute.

use super::{EV_QUOTA, QuotaPayload, quota_state};
use crate::host::{self, AppHandle};
use crate::ui::model;
use crate::ui::{BarWindow, ClaudeQuotaData, QuotaGaugeRow};
use horizon_core::layout::BarLayout;
use horizon_core::quota::QuotaWindow;
use horizon_core::time::now_ms;
use slint::{ComponentHandle, VecModel};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// Cadence de l'heure de remise à zéro (`every(60_000)`).
const TICK: Duration = Duration::from_secs(60);

struct Bound {
    window: slint::Weak<BarWindow>,
    gauges: Rc<VecModel<QuotaGaugeRow>>,
}

thread_local! {
    static BOUND: RefCell<Vec<Bound>> = RefCell::default();
    static LAST: RefCell<Option<QuotaPayload>> = const { RefCell::new(None) };
    static TIMER: slint::Timer = slint::Timer::default();
}

/// Niveau d'une limite (`quotaColor`) : celui de l'API d'abord, sinon le pourcentage ;
/// 0 accent, 1 attention, 2 alerte.
pub fn level(w: &QuotaWindow) -> i32 {
    match w.severity.as_deref() {
        Some("critical" | "exceeded") => 2,
        Some("warning") => 1,
        _ if w.used_pct >= 90.0 => 2,
        _ if w.used_pct >= 70.0 => 1,
        _ => 0,
    }
}

/// Limites montrées dans la barre (`barWindows`) : générales toujours, celles d'un modèle
/// seulement quand elles approchent de leur plafond.
pub fn bar_windows(windows: &[QuotaWindow]) -> impl Iterator<Item = &QuotaWindow> {
    windows.iter().filter(|w| w.kind != "model" || w.used_pct >= 80.0)
}

/// Date ISO 8601 en ms Unix.
pub(super) fn parse_ms(iso: &str) -> Option<u64> {
    let t = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    u64::try_from(t.timestamp_millis()).ok()
}

/// Prochaine remise à zéro des limites montrées : « remise à zéro dans 4 j 15 h » (vide sinon).
pub fn next_reset(windows: &[QuotaWindow], now: u64) -> String {
    bar_windows(windows)
        .filter_map(|w| w.resets_at.as_deref().and_then(parse_ms))
        .filter(|&t| t > now)
        .min()
        .map(|t| format!("remise à zéro {}", crate::ui::time::until(t, now)))
        .unwrap_or_default()
}

fn show_all(p: &QuotaPayload) {
    let now = now_ms();
    let windows: &[QuotaWindow] = p.quota.as_ref().map_or(&[], |q| &q.windows);
    let rows: Vec<QuotaGaugeRow> = bar_windows(windows)
        .map(|w| QuotaGaugeRow { label: w.label.as_str().into(), value: w.used_pct, level: level(w) })
        .collect();
    let reset = next_reset(windows, now);
    BOUND.with_borrow_mut(|b| {
        b.retain(|x| x.window.upgrade().is_some());
        for x in b.iter() {
            let Some(w) = x.window.upgrade() else { continue };
            let d = w.global::<ClaudeQuotaData>();
            d.set_known(p.quota.is_some() || p.error.is_some());
            d.set_failed(p.quota.is_none());
            d.set_stale(p.quota.is_some() && p.error.is_some());
            d.set_reset(reset.as_str().into());
            d.set_error(p.error.as_deref().unwrap_or_default().into());
            model::sync(&x.gauges, rows.clone());
        }
    });
    TIMER.with(|t| {
        if p.quota.is_none() {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, TICK, || LAST.with_borrow(|l| l.iter().for_each(show_all)));
        }
    });
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    let (gauges, rc) = model::new();
    window.global::<ClaudeQuotaData>().set_gauges(rc);
    BOUND.with_borrow_mut(|b| b.push(Bound { window: window.as_weak(), gauges }));
    if LAST.with_borrow(Option::is_none) {
        LAST.set(Some(quota_state(app.clone())));
    }
    LAST.with_borrow(|l| l.iter().for_each(show_all));
    host::listen(EV_QUOTA, Some(label), move |e| {
        if let Some(p) = e.payload::<QuotaPayload>() {
            let changed = LAST.with_borrow(|l| l.as_ref() != Some(p));
            if changed {
                LAST.set(Some(p.clone()));
                show_all(p);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: &'static str, used: f32, severity: Option<&str>, resets: Option<&str>) -> QuotaWindow {
        QuotaWindow {
            label: kind.into(),
            kind,
            used_pct: used,
            resets_at: resets.map(str::to_owned),
            severity: severity.map(str::to_owned),
            share: None,
        }
    }

    #[test]
    fn levels_follow_quota_color() {
        assert_eq!(level(&window("seven_day", 10.0, Some("critical"), None)), 2);
        assert_eq!(level(&window("seven_day", 10.0, Some("warning"), None)), 1);
        assert_eq!(level(&window("seven_day", 95.0, Some("normal"), None)), 2);
        assert_eq!(level(&window("seven_day", 75.0, None, None)), 1);
        assert_eq!(level(&window("seven_day", 30.0, None, None)), 0);
    }

    #[test]
    fn model_limits_only_near_their_cap() {
        let w = [window("five_hour", 10.0, None, None), window("model", 79.0, None, None), window("model", 85.0, None, None)];
        assert_eq!(bar_windows(&w).count(), 2);
    }

    #[test]
    fn next_reset_is_the_closest_future_one() {
        let now = parse_ms("2026-11-17T10:00:00Z").unwrap();
        let w = [
            window("five_hour", 10.0, None, Some("2026-11-17T13:05:00Z")),
            window("seven_day", 10.0, None, Some("2026-11-20T10:00:00+00:00")),
            window("model", 10.0, None, Some("2026-11-17T10:30:00Z")),
        ];
        assert_eq!(next_reset(&w, now), "remise à zéro dans 3 h 05", "la limite du modèle cachée ne compte pas");
        assert_eq!(next_reset(&[], now), "");
    }
}
