//! Fil des suggestions : journal du premier plan, import de l'historique Vivaldi, calcul
//! pour l'écran au premier plan et diffusion.
//!
//! Repris tel quel de `app/src-tauri/src/widgets/suggestions/worker.rs` (hôte au lieu de Tauri).

use crate::state::AppState;
use crate::util::{lock, start_thread};
use horizon_core::layout::{BarLayout, WidgetKind};
use horizon_core::time::{local_of, now_secs as now_utc};
use horizon_core::suggest::history::read_visits;
use horizon_core::apps::app_key;
use horizon_core::suggest::score::{Context, Stabilizer, rank_launchable, scores};
use horizon_core::suggest::store::{Store, VIVALDI_APP};
use horizon_core::suggest::ItemKey;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};
use crate::host::{AppHandle, Manager};

use super::view::{fixed_keys, view};
use super::{Current, Shown, Suggestions};

const RECOMPUTE_EVERY: Duration = Duration::from_secs(5 * 60);
const IMPORT_EVERY: Duration = Duration::from_secs(15 * 60);
const DEBOUNCE: Duration = Duration::from_secs(1);
/// Au-delà, le fil a été suspendu (veille) : l'intervalle en cours s'arrête au dernier réveil.
const SLEEP_GAP_SECS: i64 = 180;
const TICK: Duration = Duration::from_secs(60);
const KEEP_SECS: i64 = 26 * 7 * 86_400;
const LOOKBACK_SECS: i64 = 57 * 86_400;
/// Recalculs successifs au plus pour remplacer des applis qu'on ne sait pas relancer.
const MAX_RERANK: usize = 4;
/// Clés d'appli des raccourcis fixes : relues au plus à cet intervalle.
const FIXED_KEYS_TTL: Duration = Duration::from_secs(60 * 60);

/// Clés d'appli des cibles de raccourcis (lire un `.lnk` passe par COM), oubliées toutes les
/// heures.
struct KeyCache {
    since: Instant,
    keys: HashMap<String, Option<String>>,
}

impl KeyCache {
    fn new() -> KeyCache {
        KeyCache { since: Instant::now(), keys: HashMap::new() }
    }

    fn get(&mut self, target: &str) -> Option<String> {
        if self.since.elapsed() >= FIXED_KEYS_TTL {
            *self = KeyCache::new();
        }
        self.keys
            .entry(target.to_owned())
            .or_insert_with(|| horizon_sys::foreground::target_app_key(target))
            .clone()
    }
}

/// Écran (identifiant de la disposition) d'un nom GDI `\\.\DISPLAYn`.
fn monitor_id(cache: &mut HashMap<String, String>, device: &str) -> Option<String> {
    if !cache.contains_key(device) {
        *cache = horizon_sys::monitors::list().into_iter().map(|m| (m.device_name, m.id)).collect();
    }
    cache.get(device).cloned()
}

/// Importe les nouvelles visites de Vivaldi. La copie de l'historique (plusieurs dizaines de
/// Mo) se fait sans tenir la base.
fn import(hub: &Suggestions, mtimes: &mut HashMap<String, SystemTime>) {
    let scratch = crate::state::app_dir().join("tmp");
    for p in horizon_sys::vivaldi::profiles() {
        let Ok(mtime) = std::fs::metadata(&p.history).and_then(|m| m.modified()) else { continue };
        if mtimes.get(&p.dir) == Some(&mtime) {
            continue;
        }
        let started = Instant::now();
        let Some(after) = hub.with_store(|s| s.cursor(&p.dir).unwrap_or(0)) else { return };
        match read_visits(&p.history, after, &scratch) {
            Ok(batch) => {
                let visits: Vec<(i64, String, i64, i64)> =
                    batch.visits.into_iter().map(|v| (v.id, v.host, v.utc, local_of(v.utc))).collect();
                let n = visits.len();
                let saved = lock(&hub.store).as_mut().map(|store| {
                    store
                        .add_visits(&p.dir, &visits)
                        .and_then(|()| batch.last_id.map_or(Ok(()), |id| store.set_cursor(&p.dir, id)))
                });
                match saved {
                    Some(Ok(())) => {
                        mtimes.insert(p.dir.clone(), mtime);
                        if n > 0 {
                            log::info!("suggestions : {n} visites importées ({}) en {:?}", p.dir, started.elapsed());
                        }
                    }
                    Some(Err(e)) => log::warn!("suggestions : enregistrement des visites : {e:#}"),
                    None => return,
                }
            }
            Err(e) => log::warn!("suggestions : lecture de l'historique {} : {e:#}", p.dir),
        }
    }
}

/// Une barre cliquable affiche le widget Suggestions (seules celles-là en reçoivent, voir
/// `compute`).
fn shown_somewhere(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .read(|l| l.bars.iter().any(|b| b.clickable() && b.widgets().any(|w| w.kind == WidgetKind::SUGGESTIONS)))
}

/// État propre au fil de calcul.
struct Worker {
    stabilizer: Stabilizer,
    fixed: KeyCache,
}

/// Suggestions de l'écran au premier plan (les autres écrans n'en ont aucune) ; `None` si la
/// base est illisible.
fn compute(app: &AppHandle, hub: &Suggestions, w: &mut Worker) -> Option<Shown> {
    let state = app.state::<AppState>();
    for (old, new) in state.take_renamed() {
        if let Some(Err(e)) = hub.with_store(|s| s.remap_monitor(&old, &new)) {
            log::warn!("suggestions : écran {old} → {new} : {e:#}");
        }
    }
    let bars: Vec<BarLayout> = state.read(|l| l.bars.clone());
    let mut next: Shown = bars.iter().map(|b| (b.monitor_id.clone(), Vec::new())).collect();
    let current = lock(&hub.current).as_ref().map(|c| (c.monitor.clone(), c.app.clone()));
    let target = current.and_then(|(monitor, fg_app)| {
        let bar = bars.iter().find(|b| b.monitor_id == monitor)?;
        let has_widget = bar.widgets().any(|w| w.kind == WidgetKind::SUGGESTIONS);
        (bar.clickable() && has_widget).then(|| (bar.clone(), fg_app))
    });
    let Some((bar, fg_app)) = target else { return Some(next) };
    let now = now_utc();
    let now_local = local_of(now);
    let read = hub.with_store(|s| s.usages(now - LOOKBACK_SECS).map(|u| (u, s.blocked().unwrap_or_default())))?;
    let (usages, mut excluded) = match read {
        Ok(r) => r,
        Err(e) => {
            log::warn!("suggestions : lecture de la base : {e:#}");
            return None;
        }
    };
    excluded.extend(fixed_keys(&bar, |t| w.fixed.get(t)));
    let fg_key = ItemKey::App(app_key(&fg_app));
    let browser = app_key(&fg_app) == VIVALDI_APP;
    let base = Context {
        now_local,
        monitor: &bar.monitor_id,
        foreground: Some(&fg_key),
        foreground_is_browser: browser,
        excluded: &excluded,
    };
    let (ranked, excluded) = rank_launchable(&usages, &base, |k| hub.launch_target(k).is_some(), MAX_RERANK);
    let ctx = Context { excluded: &excluded, ..base };
    let all = scores(&usages, &ctx);
    let shown = w.stabilizer.update(&bar.monitor_id, now_local, ranked, &all);
    let views = hub.with_store(|s| shown.into_iter().map(|x| view(s, &x.item, x.reason)).collect())?;
    next.insert(bar.monitor_id.clone(), views);
    Some(next)
}

/// Recalcule et diffuse si les suggestions ont changé.
fn recompute(app: &AppHandle, hub: &Suggestions, w: &mut Worker) {
    if let Some(next) = compute(app, hub, w) {
        hub.shown.set(app, next);
    }
}

/// Démarre le journal, l'import et le calcul sur un fil dédié.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let fg_rx = crate::foreground::subscribe(app);
    let app = app.clone();
    start_thread("suggestions", move || {
        let hub = app.state::<Suggestions>();
        match Store::open(&crate::state::app_dir().join("usage.db")) {
            Ok(s) => {
                if let Err(e) = s.purge(now_utc() - KEEP_SECS) {
                    log::warn!("suggestions : purge : {e:#}");
                }
                *lock(&hub.store) = Some(s);
            }
            Err(e) => {
                log::error!("suggestions : ouverture de usage.db : {e:#}");
                return;
            }
        }
        let mut worker = Worker { stabilizer: Stabilizer::default(), fixed: KeyCache::new() };
        let mut devices: HashMap<String, String> = HashMap::new();
        let mut mtimes: HashMap<String, SystemTime> = HashMap::new();
        let mut last_import: Option<Instant> = None;
        let mut last_compute = Instant::now();
        let mut pending: Option<Instant> = Some(Instant::now());
        let mut last_tick = now_utc();
        loop {
            let wait = pending.map_or(TICK, |p| p.saturating_duration_since(Instant::now()).min(TICK));
            crossbeam_channel::select! {
                recv(fg_rx) -> msg => {
                    let Ok(fg) = msg else { return };
                    let now = now_utc();
                    hub.close_current(now);
                    if let Some(f) = fg
                        && let Some(monitor) = monitor_id(&mut devices, &f.monitor_device)
                    {
                        *lock(&hub.current) = Some(Current { app: f.app, monitor, start_utc: now, start_local: local_of(now) });
                    }
                    pending = Some(Instant::now() + DEBOUNCE);
                }
                recv(hub.wake.receiver()) -> _ => pending = Some(Instant::now()),
                default(wait) => {}
            }
            let now = now_utc();
            if now - last_tick > SLEEP_GAP_SECS {
                // Retour de veille : l'appli au premier plan l'est encore, mais le temps
                // de la veille ne compte pas.
                if let Some((app_id, monitor)) = hub.close_current(last_tick) {
                    *lock(&hub.current) = Some(Current { app: app_id, monitor, start_utc: now, start_local: local_of(now) });
                }
            }
            last_tick = now;
            // L'historique (plusieurs dizaines de Mo copiés) n'est importé que si une barre
            // cliquable affiche des suggestions ; sinon l'import attend qu'il en faille.
            // Mode jeu : import retardé (copie et lecture d'une grosse base).
            if last_import.is_none_or(|t| t.elapsed() >= IMPORT_EVERY) && shown_somewhere(&app) && !crate::game::active() {
                import(&hub, &mut mtimes);
                last_import = Some(Instant::now());
            }
            let due = pending.is_some_and(|p| Instant::now() >= p) || last_compute.elapsed() >= RECOMPUTE_EVERY;
            if due {
                recompute(&app, &hub, &mut worker);
                last_compute = Instant::now();
                pending = None;
            }
        }
    })
}
