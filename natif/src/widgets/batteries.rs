//! Batteries du téléphone et de la montre (fichier de l'agent SharkApp), relues quand le
//! fichier change (et à la sortie de veille) et envoyées aux barres. Une mesure de plus de 2 h
//! est atténuée (revu chaque minute) ; sous le seuil réglé (`[alerts] devices_low`), couleur
//! d'alerte.
//!
//! Passage sous le seuil : alerte « batterie faible » dans l'îlot (`alerts::LowBattery`).
//!
//! Repris de `app/src-tauri/src/widgets/batteries.rs`. Liaison à la barre (global
//! `BatteriesData`) ici.

use crate::alerts::LowBattery;
use crate::host::{self, AppHandle, Manager};
use crate::state::AppState;
use crate::ui::{BarWindow, BatteriesData, BatteryItem, icons};
use crate::util::{MtimeCache, Published, start_thread};
use crate::widgets::Backend;
use horizon_core::batteries::{Batteries, Battery, parse};
use horizon_core::layout::{BarLayout, WidgetKind};
use horizon_core::low_battery::recent;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Batteries du téléphone et de la montre.
pub const EV_BATTERIES: &str = "horizon://batteries";

pub const BACKEND: Backend = Backend {
    id: "batteries",
    name: "batteries",
    manage: |app| {
        app.manage(BatteriesState::default());
    },
    start: Some(start),
    bind: Some(bind),
    ..Backend::NONE
};

const PERIOD: Duration = Duration::from_secs(10);
/// Au-delà, la mesure est jugée ancienne (la montre n'envoie la sienne que de temps en temps).
const STALE_MS: i64 = 2 * 3_600_000;
/// Seuil de la couleur d'alerte quand l'alerte est coupée (lib/battery.ts, `LOW_DEFAULT`).
const LOW_DEFAULT: u8 = 15;
/// Revue de l'ancienneté des mesures.
const AGE_CHECK: Duration = Duration::from_secs(60);

pub struct BatteriesState(Published<Batteries>);

impl Default for BatteriesState {
    fn default() -> Self {
        BatteriesState(Published::new(EV_BATTERIES, Batteries::default()))
    }
}

pub fn path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("KingsHub").join("batteries.json"))
}

/// Contenu du fichier ; absent ou illisible : aucune batterie.
fn read(path: &Path) -> Batteries {
    if !path.exists() {
        return Batteries::default();
    }
    // Écrit par remplacement atomique : jamais lu à moitié.
    std::fs::read_to_string(path).map_err(anyhow::Error::from).and_then(|t| parse(&t)).unwrap_or_else(|e| {
        log::warn!("batteries : {} illisible : {e:#}", path.display());
        Batteries::default()
    })
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("batteries", move || {
        let Some(path) = path() else { return };
        let file = MtimeCache::new();
        let mut phone = LowBattery::new(WidgetKind::BATTERIES, "deviceMobile");
        let mut watch = LowBattery::new(WidgetKind::BATTERIES, "watch");
        loop {
            if let Some(now) = file.fresh(&path, || Some(read(&path))) {
                let now_ms = horizon_core::time::now_ms() as i64;
                // Une mesure ancienne n'annonce rien (la montre envoie la sienne de loin en loin).
                let check = |low: &mut LowBattery, kind: &str, b: &Option<Battery>| {
                    if let Some(b) = b.as_ref().filter(|b| recent(b.at_ms, now_ms)) {
                        let name = if b.name.is_empty() { kind } else { b.name.as_str() };
                        low.update(&app, name, b.percent, b.charging);
                    }
                };
                check(&mut phone, "Téléphone", &now.phone);
                check(&mut watch, "Montre", &now.watch);
                app.state::<BatteriesState>().0.set(&app, now);
            }
            if crate::resume::sleep(PERIOD) {
                file.forget();
            }
        }
    })
}

/// Dernières batteries lues (témoin d'état).
pub fn batteries_state(app: &AppHandle) -> Batteries {
    app.state::<BatteriesState>().0.get()
}

/// Niveau affiché en couleur d'alerte (au seuil par défaut si l'alerte est coupée).
fn is_low(percent: u8, charging: bool, threshold: Option<u8>) -> bool {
    !charging && percent <= threshold.unwrap_or(LOW_DEFAULT)
}

/// Ce que montre le widget : les batteries connues, dans l'ordre téléphone, montre.
fn items(b: &Batteries, threshold: Option<u8>, now_ms: i64) -> Vec<BatteryItem> {
    let item = |icon: &str, kind: &str, b: &Battery| BatteryItem {
        icon: icons::image(icon),
        percent: format!("{} %", b.percent).into(),
        charging: b.charging,
        low: is_low(b.percent, b.charging, threshold),
        stale: now_ms - b.at_ms > STALE_MS,
        tip: format!(
            "{} — {} %{} ({})",
            if b.name.is_empty() { kind } else { &b.name },
            b.percent,
            if b.charging { ", en charge" } else { "" },
            crate::ui::time::ago(b.at_ms.max(0) as u64, now_ms.max(0) as u64)
        )
        .into(),
    };
    [("deviceMobile", "Téléphone", &b.phone), ("watch", "Montre", &b.watch)]
        .into_iter()
        .filter_map(|(icon, kind, b)| Some(item(icon, kind, b.as_ref()?)))
        .collect()
}

thread_local! {
    /// Barres qui montrent le widget, et leur seuil.
    static BARS: RefCell<Vec<(slint::Weak<BarWindow>, Option<u8>)>> = RefCell::default();
    static TIMER: slint::Timer = slint::Timer::default();
    /// Abonnement et revue communs à toutes les barres, posés une fois.
    static WATCHING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn show(window: &BarWindow, b: &Batteries, threshold: Option<u8>) {
    let next = items(b, threshold, horizon_core::time::now_ms() as i64);
    let g = window.global::<BatteriesData>();
    // Rien de changé (la plupart des relectures) : rien à redessiner.
    if g.get_items().iter().ne(next.iter().cloned()) {
        g.set_items(ModelRc::new(VecModel::from(next)));
    }
}

/// Toutes les barres : nouvelles valeurs ou mesures devenues anciennes.
fn show_all(b: &Batteries) {
    BARS.with_borrow_mut(|bars| {
        bars.retain(|(w, _)| w.upgrade().is_some());
        for (w, threshold) in bars.iter() {
            if let Some(w) = w.upgrade() {
                show(&w, b, *threshold);
            }
        }
    });
}

fn bind(app: &AppHandle, window: &BarWindow, _label: &str, _bar: &BarLayout) {
    let threshold = app.state::<AppState>().read(|l| crate::state::settings().alerts.low_threshold(l, WidgetKind::BATTERIES));
    show(window, &app.state::<BatteriesState>().0.get(), threshold);
    BARS.with_borrow_mut(|bars| bars.push((window.as_weak(), threshold)));
    if !WATCHING.replace(true) {
        // Un seul abonnement et une seule revue pour toutes les barres.
        host::listen(EV_BATTERIES, None, |e| {
            if let Some(b) = e.payload::<Batteries>() {
                show_all(b);
            }
        });
        let a = app.clone();
        TIMER.with(|t| t.start(slint::TimerMode::Repeated, AGE_CHECK, move || show_all(&a.state::<BatteriesState>().0.get())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bat(percent: u8, charging: bool, at_ms: i64) -> Battery {
        Battery { name: String::new(), percent, charging, at_ms }
    }

    #[test]
    fn low_color_follows_the_threshold() {
        assert!(is_low(15, false, None), "alerte coupée : seuil par défaut");
        assert!(!is_low(15, true, Some(20)), "en charge : jamais");
        assert!(is_low(20, false, Some(20)));
        assert!(!is_low(21, false, Some(20)));
    }

    #[test]
    fn items_in_order_and_staleness() {
        let now = 10 * STALE_MS;
        let b = Batteries { phone: Some(bat(80, true, now)), watch: Some(bat(10, false, now - STALE_MS - 1)) };
        let v = items(&b, Some(15), now);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].percent.as_str(), "80 %");
        assert!(v[0].charging && !v[0].low && !v[0].stale);
        assert!(v[1].low && v[1].stale);
        assert_eq!(v[0].tip.as_str(), "Téléphone — 80 %, en charge (à l’instant)");
        assert_eq!(v[1].tip.as_str(), "Montre — 10 % (il y a 2 h)");
        assert!(items(&Batteries::default(), None, now).is_empty());
    }
}
