//! Score d'un élément pour un écran et un instant : usages pondérés par l'heure, le jour de la
//! semaine, l'ancienneté et l'écran (conception §5.3).

use super::ItemKey;
use std::collections::{HashMap, HashSet};

/// Un usage : appli au premier plan au moins 10 s, ou visite d'un site.
#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub item: ItemKey,
    /// Heure locale « naïve » en secondes depuis 1970 (l'heure affichée par l'horloge).
    pub local_ts: i64,
    /// Écran où l'usage a eu lieu ; `None` = inconnu (historique ancien, onglet en arrière-plan).
    pub monitor: Option<String>,
}

pub struct Context<'a> {
    pub now_local: i64,
    pub monitor: &'a str,
    /// Élément au premier plan : jamais suggéré.
    pub foreground: Option<&'a ItemKey>,
    /// Le navigateur est au premier plan : les sites passent devant.
    pub foreground_is_browser: bool,
    pub excluded: &'a HashSet<ItemKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub item: ItemKey,
    pub score: f64,
    pub reason: String,
}

const MINUTES_PER_DAY: i64 = 1440;
const SIGMA_MIN: f64 = 45.0;
const MAX_GAP_MIN: i64 = 120;
const ELIGIBLE_GAP_MIN: i64 = 60;
const OTHER_WEEKDAY: f64 = 0.3;
const HALF_LIFE_DAYS: f64 = 28.0;
const UNATTRIBUTED: f64 = 0.3;
const BROWSER_BOOST: f64 = 1.5;
const MERGE_SECS: i64 = 30 * 60;
const ELIGIBLE_DAYS: usize = 3;
const WINDOW_DAYS: i64 = 56;
const MAX_SHOWN: usize = 3;
const HOLD_SECS: i64 = 600;

/// Score de 3 usages le même jour de la semaine, à la même heure, sur le même écran, âgés de
/// 1, 2 et 3 semaines (0,5^¼ + 0,5^½ + 0,5^¾), × 0,9 : le minimum d'une habitude hebdomadaire.
pub(crate) const THRESHOLD: f64 = 0.9 * (0.840_896_415_253_714_6 + std::f64::consts::FRAC_1_SQRT_2 + 0.594_603_557_501_360_5);

const WEEKDAYS: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];

/// Jour de la semaine d'une heure locale naïve : 0 = lundi (le 1er janvier 1970 était un jeudi).
pub(crate) fn weekday(local_ts: i64) -> i64 {
    (local_ts.div_euclid(86_400) + 3).rem_euclid(7)
}

fn minute_of_day(local_ts: i64) -> i64 {
    local_ts.div_euclid(60).rem_euclid(MINUTES_PER_DAY)
}

/// Écart signé (minutes, dans [-720, 720)) de l'heure de l'usage à l'heure courante.
fn clock_gap(usage: i64, now: i64) -> i64 {
    (minute_of_day(now) - minute_of_day(usage) + 720).rem_euclid(MINUTES_PER_DAY) - 720
}

/// Usages de site du même hôte à moins de 30 min : un seul (le plus ancien).
fn merged(usages: &[Usage]) -> Vec<&Usage> {
    let mut sorted: Vec<&Usage> = usages.iter().collect();
    sorted.sort_by(|a, b| a.item.as_str().cmp(&b.item.as_str()).then(a.local_ts.cmp(&b.local_ts)));
    let mut out: Vec<&Usage> = Vec::with_capacity(sorted.len());
    let mut last_kept: Option<&Usage> = None;
    for u in sorted {
        let dup = matches!(u.item, ItemKey::Site(_))
            && last_kept.is_some_and(|k| k.item == u.item && u.local_ts - k.local_ts < MERGE_SECS);
        if !dup {
            out.push(u);
            last_kept = Some(u);
        }
    }
    out
}

struct Acc {
    score: f64,
    same_weekday: f64,
    days: HashSet<i64>,
}

fn accumulate(usages: &[Usage], ctx: &Context) -> HashMap<ItemKey, Acc> {
    let list = merged(usages);
    let attributed: HashSet<&ItemKey> =
        list.iter().filter(|u| u.monitor.is_some()).map(|u| &u.item).collect();
    let now_wd = weekday(ctx.now_local);
    let mut acc: HashMap<ItemKey, Acc> = HashMap::new();
    for u in list {
        if ctx.excluded.contains(&u.item) || ctx.foreground == Some(&u.item) {
            continue;
        }
        let age = ctx.now_local - u.local_ts;
        if age < 0 {
            continue;
        }
        // Un élément jamais vu sur un écran précis compte pleinement partout : l'historique
        // ancien suffit dès le premier jour. Dès qu'il a été vu sur un écran, l'historique
        // sans écran ne pèse plus que 0,3.
        let screen = match &u.monitor {
            Some(m) if m == ctx.monitor => 1.0,
            Some(_) => continue,
            None if attributed.contains(&u.item) => UNATTRIBUTED,
            None => 1.0,
        };
        let gap = clock_gap(u.local_ts, ctx.now_local);
        if gap.abs() > MAX_GAP_MIN {
            continue;
        }
        // Instant de la journée de l'usage qui a l'heure courante : 23 h 50 vendredi, vu à
        // 0 h 10, compte pour samedi.
        let aligned = u.local_ts + gap * 60;
        let same_day = weekday(aligned) == now_wd;
        let w_hour = (-((gap * gap) as f64) / (2.0 * SIGMA_MIN * SIGMA_MIN)).exp();
        let w_day = if same_day { 1.0 } else { OTHER_WEEKDAY };
        let w_age = 0.5f64.powf(age as f64 / 86_400.0 / HALF_LIFE_DAYS);
        let boost = if ctx.foreground_is_browser && matches!(u.item, ItemKey::Site(_)) {
            BROWSER_BOOST
        } else {
            1.0
        };
        let w = w_hour * w_day * w_age * screen * boost;
        let a = acc
            .entry(u.item.clone())
            .or_insert(Acc { score: 0.0, same_weekday: 0.0, days: HashSet::new() });
        a.score += w;
        if same_day {
            a.same_weekday += w;
        }
        if gap.abs() <= ELIGIBLE_GAP_MIN && age <= WINDOW_DAYS * 86_400 {
            a.days.insert(aligned.div_euclid(86_400));
        }
    }
    acc
}

/// Score de chaque élément pour cet écran et cet instant (exclus et premier plan omis).
pub fn scores(usages: &[Usage], ctx: &Context) -> HashMap<ItemKey, f64> {
    accumulate(usages, ctx).into_iter().map(|(k, a)| (k, a.score)).collect()
}

/// Suggestions assez sûres, de la meilleure à la moins bonne, trois au plus.
pub fn rank(usages: &[Usage], ctx: &Context) -> Vec<Suggestion> {
    let hour = ((minute_of_day(ctx.now_local) + 30) / 60) % 24;
    let day = WEEKDAYS[weekday(ctx.now_local) as usize];
    let mut out: Vec<Suggestion> = accumulate(usages, ctx)
        .into_iter()
        .filter(|(_, a)| a.days.len() >= ELIGIBLE_DAYS && a.score >= THRESHOLD)
        .map(|(item, a)| {
            let reason = if a.same_weekday >= 0.6 * a.score {
                format!("souvent le {day} vers {hour:02}:00")
            } else {
                format!("souvent vers {hour:02}:00")
            };
            Suggestion { item, score: a.score, reason }
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.item.as_str().cmp(&b.item.as_str())));
    out.truncate(MAX_SHOWN);
    out
}

/// Classement sans les applis qu'on ne sait pas relancer : chaque appli classée pour laquelle
/// `launchable` est faux est écartée et le classement refait, `rounds` fois au plus. Renvoie le
/// classement et les exclusions finales (pour `scores`).
pub fn rank_launchable(
    usages: &[Usage],
    ctx: &Context,
    mut launchable: impl FnMut(&str) -> bool,
    rounds: usize,
) -> (Vec<Suggestion>, HashSet<ItemKey>) {
    let mut excluded = ctx.excluded.clone();
    let mut ranked = Vec::new();
    for _ in 0..rounds {
        ranked = rank(usages, &Context { excluded: &excluded, ..*ctx });
        let stuck: Vec<ItemKey> = ranked
            .iter()
            .filter(|s| matches!(&s.item, ItemKey::App(k) if !launchable(k)))
            .map(|s| s.item.clone())
            .collect();
        if stuck.is_empty() {
            break;
        }
        excluded.extend(stuck);
    }
    (ranked, excluded)
}

/// Évite le clignotement : une suggestion affichée reste 10 min tant que son score ne tombe pas
/// sous la moitié du seuil. Mémoire propre à chaque écran.
#[derive(Default)]
pub struct Stabilizer {
    shown: HashMap<String, Vec<(Suggestion, i64)>>,
}

impl Stabilizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(
        &mut self,
        monitor: &str,
        now_local: i64,
        ranked: Vec<Suggestion>,
        all_scores: &HashMap<ItemKey, f64>,
    ) -> Vec<Suggestion> {
        let previous = self.shown.remove(monitor).unwrap_or_default();
        let mut next: Vec<(Suggestion, i64)> = Vec::new();
        for (s, since) in previous {
            let held = now_local - since < HOLD_SECS
                && all_scores.get(&s.item).is_some_and(|&v| v >= THRESHOLD / 2.0);
            match ranked.iter().find(|r| r.item == s.item) {
                Some(r) => next.push((r.clone(), since)),
                None if held => next.push((s, since)),
                None => {}
            }
        }
        for r in ranked {
            if next.len() >= MAX_SHOWN {
                break;
            }
            if !next.iter().any(|(s, _)| s.item == r.item) {
                next.push((r, now_local));
            }
        }
        let out = next.iter().map(|(s, _)| s.clone()).collect();
        self.shown.insert(monitor.to_owned(), next);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;
    /// Samedi 7 novembre 2026, 00:00 (heure locale naïve).
    const SAT: i64 = 1_794_009_600;

    /// `wd` : 0 = lundi … 5 = samedi ; semaine `weeks_before` avant celle du samedi 7 novembre.
    fn at(wd: i64, h: i64, m: i64, weeks_before: i64) -> i64 {
        SAT + (wd - 5) * DAY + h * 3600 + m * 60 - weeks_before * 7 * DAY
    }

    fn yt() -> ItemKey {
        ItemKey::Site("youtube.com".into())
    }

    fn u(item: ItemKey, ts: i64, mon: Option<&str>) -> Usage {
        Usage { item, local_ts: ts, monitor: mon.map(Into::into) }
    }

    fn ctx<'a>(now: i64, mon: &'a str, ex: &'a HashSet<ItemKey>) -> Context<'a> {
        Context { now_local: now, monitor: mon, foreground: None, foreground_is_browser: false, excluded: ex }
    }

    fn saturdays(n: i64, mon: Option<&str>) -> Vec<Usage> {
        (1..=n).map(|w| u(yt(), at(5, 16, 0, w) + w * 60, mon)).collect()
    }

    #[test]
    fn apps_that_cannot_be_launched_are_replaced() {
        let app = |k: &str| ItemKey::App(k.into());
        let mut usages = saturdays(6, Some("M"));
        for k in ["perdue", "ok"] {
            usages.extend((1..=6).map(|w| u(app(k), at(5, 16, 0, w) + w * 60, Some("M"))));
        }
        let ex = HashSet::new();
        let now = at(5, 16, 0, 0);
        let (ranked, excluded) = rank_launchable(&usages, &ctx(now, "M", &ex), |k| k != "perdue", 4);
        assert!(ranked.iter().all(|s| s.item != app("perdue")));
        assert!(ranked.iter().any(|s| s.item == app("ok")));
        assert_eq!(excluded, HashSet::from([app("perdue")]));
        // Aucun recalcul permis : le classement brut reste.
        let (raw, _) = rank_launchable(&usages, &ctx(now, "M", &ex), |_| false, 0);
        assert!(raw.is_empty());
    }

    #[test]
    fn weekday_formula_matches_known_saturday() {
        assert_eq!(weekday(SAT), 5);
        assert_eq!(weekday(at(0, 10, 0, 0)), 0);
    }

    #[test]
    fn three_saturdays_youtube_is_suggested_saturday_4pm() {
        let ex = HashSet::new();
        let r = rank(&saturdays(3, Some("M")), &ctx(at(5, 16, 5, 0), "M", &ex));
        assert_eq!(r.iter().map(|s| &s.item).collect::<Vec<_>>(), vec![&yt()]);
        assert_eq!(r[0].reason, "souvent le samedi vers 16:00");
    }

    #[test]
    fn two_occurrences_are_not_enough() {
        let ex = HashSet::new();
        assert!(rank(&saturdays(2, Some("M")), &ctx(at(5, 16, 5, 0), "M", &ex)).is_empty());
    }

    #[test]
    fn other_screen_usages_do_not_count() {
        let ex = HashSet::new();
        assert!(rank(&saturdays(3, Some("L")), &ctx(at(5, 16, 5, 0), "M", &ex)).is_empty());
    }

    #[test]
    fn unattributed_history_only_item_is_suggested_everywhere() {
        let ex = HashSet::new();
        let us = saturdays(3, None);
        for m in ["M", "L"] {
            assert_eq!(rank(&us, &ctx(at(5, 16, 5, 0), m, &ex)).len(), 1, "{m}");
        }
    }

    #[test]
    fn attributed_usage_weakens_history_on_other_screens() {
        let ex = HashSet::new();
        let now = at(5, 16, 5, 0);
        let before = scores(&saturdays(4, None), &ctx(now, "L", &ex))[&yt()];
        let mut us = saturdays(4, None);
        us.push(u(yt(), at(5, 16, 0, 0) - 3 * DAY, Some("M"))); // vu une fois sur M
        let on_m = scores(&us, &ctx(now, "M", &ex))[&yt()];
        let on_l = scores(&us, &ctx(now, "L", &ex))[&yt()];
        assert!(on_l < before * 0.5, "{on_l} vs {before}");
        assert!(on_m > on_l);
    }

    #[test]
    fn wrong_time_is_not_suggested() {
        let ex = HashSet::new();
        assert!(rank(&saturdays(4, Some("M")), &ctx(at(5, 10, 0, 0), "M", &ex)).is_empty());
    }

    #[test]
    fn daily_habit_counts_on_other_weekdays() {
        let ex = HashSet::new();
        let tw = ItemKey::Site("twitch.tv".into());
        let us: Vec<Usage> = (1..=21).map(|d| u(tw.clone(), at(1, 21, 0, 0) - d * DAY, Some("M"))).collect();
        let r = rank(&us, &ctx(at(1, 21, 0, 0), "M", &ex));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].reason, "souvent vers 21:00");
    }

    #[test]
    fn excluded_and_foreground_are_skipped() {
        let now = at(5, 16, 5, 0);
        let us = saturdays(3, Some("M"));
        let ex: HashSet<ItemKey> = [yt()].into();
        assert!(rank(&us, &ctx(now, "M", &ex)).is_empty());
        let none = HashSet::new();
        let fg = yt();
        let c = Context { foreground: Some(&fg), ..ctx(now, "M", &none) };
        assert!(rank(&us, &c).is_empty());
    }

    #[test]
    fn at_most_three() {
        let ex = HashSet::new();
        let mut us = Vec::new();
        for i in 0..5 {
            let k = ItemKey::App(format!("a{i}.exe"));
            us.extend((1..=3).map(|w| u(k.clone(), at(5, 16, 0, w), Some("M"))));
        }
        assert_eq!(rank(&us, &ctx(at(5, 16, 0, 0), "M", &ex)).len(), 3);
    }

    #[test]
    fn visits_within_30_min_merge() {
        let ex = HashSet::new();
        let t = at(5, 16, 0, 1);
        let many: Vec<Usage> = (0..10).map(|i| u(yt(), t + i * 120, Some("M"))).collect();
        let one = vec![u(yt(), t, Some("M"))];
        let now = at(5, 16, 0, 0);
        let a = scores(&many, &ctx(now, "M", &ex))[&yt()];
        let b = scores(&one, &ctx(now, "M", &ex))[&yt()];
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn browser_in_front_boosts_sites() {
        let ex = HashSet::new();
        let now = at(5, 16, 0, 0);
        let us = saturdays(3, Some("M"));
        let plain = scores(&us, &ctx(now, "M", &ex))[&yt()];
        let boosted = scores(&us, &Context { foreground_is_browser: true, ..ctx(now, "M", &ex) })[&yt()];
        assert!((boosted - plain * 1.5).abs() < 1e-9);
    }

    #[test]
    fn score_handles_future_and_old_events() {
        let ex = HashSet::new();
        let now = at(5, 16, 0, 0);
        let us = vec![u(yt(), now + 7 * DAY, Some("M")), u(yt(), now - 200 * DAY, Some("M"))];
        let s = scores(&us, &ctx(now, "M", &ex)).get(&yt()).copied().unwrap_or(0.0);
        assert!(s.is_finite() && s < THRESHOLD);
        assert!(rank(&us, &ctx(now, "M", &ex)).is_empty());
    }

    #[test]
    fn midnight_is_close_to_23h() {
        let ex = HashSet::new();
        let k = ItemKey::App("late.exe".into());
        let us: Vec<Usage> = (1..=3).map(|w| u(k.clone(), at(4, 23, 50, w), Some("M"))).collect();
        assert_eq!(rank(&us, &ctx(at(5, 0, 10, 0), "M", &ex)).len(), 1);
    }

    fn sug(item: ItemKey) -> Suggestion {
        Suggestion { item, score: THRESHOLD * 2.0, reason: String::new() }
    }

    #[test]
    fn stabilizer_keeps_item_10_minutes() {
        let mut st = Stabilizer::new();
        let a = ItemKey::App("a.exe".into());
        let b = ItemKey::App("b.exe".into());
        let t = 1_000_000;
        assert_eq!(st.update("M", t, vec![sug(a.clone())], &[(a.clone(), THRESHOLD)].into()).len(), 1);
        // a n'est plus classé mais garde un score > seuil/2 : il reste, b s'ajoute.
        let sc: HashMap<ItemKey, f64> = [(a.clone(), THRESHOLD * 0.6), (b.clone(), THRESHOLD)].into();
        let shown = st.update("M", t + 300, vec![sug(b.clone())], &sc);
        assert_eq!(shown.iter().map(|s| &s.item).collect::<Vec<_>>(), vec![&a, &b]);
        // Après 10 min, a disparaît.
        let shown = st.update("M", t + 601, vec![sug(b.clone())], &sc);
        assert_eq!(shown.iter().map(|s| &s.item).collect::<Vec<_>>(), vec![&b]);
    }

    #[test]
    fn stabilizer_drops_item_under_half_threshold() {
        let mut st = Stabilizer::new();
        let a = ItemKey::App("a.exe".into());
        st.update("M", 0, vec![sug(a.clone())], &[(a.clone(), THRESHOLD)].into());
        assert!(st.update("M", 60, vec![], &[(a, THRESHOLD * 0.4)].into()).is_empty());
    }

    #[test]
    fn stabilizer_is_per_screen() {
        let mut st = Stabilizer::new();
        let a = ItemKey::App("a.exe".into());
        st.update("M", 0, vec![sug(a.clone())], &[(a.clone(), THRESHOLD)].into());
        assert!(st.update("L", 60, vec![], &[(a, THRESHOLD)].into()).is_empty());
    }
}
