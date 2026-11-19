//! Batterie du casque, relue toutes les 30 s (5 s quand la bascule audio est active), et tout
//! de suite quand le récepteur annonce l'arrivée ou le départ du casque (`Reader::on_notice`) ;
//! envoyée aux barres quand elle change. `None` : pas de récepteur, ou casque éteint.
//!
//! Bascule audio (`[features] headset_switch` + `fallback_output`) : casque éteint, la sortie
//! par défaut passe sur la sortie de repli ; rallumé, elle revient (`horizon_core::headset_switch`,
//! qui dit aussi quand relire pour confirmer sans attendre).
//!
//! Repris de `app/src-tauri/src/widgets/headset.rs`. Différences : les sorties sont lues et la
//! sortie par défaut changée directement (`horizon_sys::mixer::outputs_once`,
//! `set_default_once`) en attendant le mixeur du cœur. Îlot dynamique : « batterie faible »
//! (`alerts::LowBattery`) et « Casque éteint — Son sur … ».

use super::Backend;
use crate::alerts::{self, LowBattery, Severity};
use horizon_core::dnd::AlertKind;
use crate::host::{AppHandle, Manager};
use crate::ui::BarWindow;
use crate::util::{Published, Waker, start_thread};
use horizon_core::headset_switch::{HeadsetSwitch, Switch};
use horizon_core::layout::BarLayout;
use horizon_sys::headset::{Headset, Reader};
use slint::ComponentHandle;
use std::time::Duration;

/// Batterie du casque.
pub const EV_HEADSET: &str = "horizon://headset";

pub const BACKEND: Backend = Backend {
    id: "headset",
    name: "casque",
    manage: |app| {
        app.manage(HeadsetState::default());
    },
    start: Some(start),
    bind: Some(bind),
    ..Backend::NONE
};

const PERIOD: Duration = Duration::from_secs(30);
/// Bascule audio active : relecture de secours pour les casques qui n'annoncent rien.
const PERIOD_SWITCH: Duration = Duration::from_secs(5);

pub struct HeadsetState {
    headset: Published<Option<Headset>>,
    /// Avis du récepteur : relire tout de suite.
    wake: Waker,
}

impl Default for HeadsetState {
    fn default() -> Self {
        HeadsetState { headset: Published::new(EV_HEADSET, None), wake: Waker::default() }
    }
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("casque", move || {
        let mut reader = match Reader::new() {
            Ok(r) => r,
            Err(e) => return log::error!("casque : accès HID impossible : {e:#}"),
        };
        let notified = app.clone();
        reader.on_notice(move || notified.state::<HeadsetState>().wake.wake());
        let mut last_error = String::new();
        let mut low = LowBattery::new("headset");
        let mut switch = HeadsetSwitch::default();
        loop {
            let fallback = crate::state::settings().features.headset_fallback().map(str::to_owned);
            let now = match reader.poll() {
                Ok(h) => {
                    last_error.clear();
                    match &fallback {
                        Some(fb) => follow_headset(&app, &mut switch, h.is_some(), reader.present(), fb),
                        None => switch.reset(),
                    }
                    h
                }
                Err(e) => {
                    // Une erreur isolée (réponse perdue) est fréquente : journalisée une fois.
                    let msg = format!("{e:#}");
                    if msg != last_error {
                        log::warn!("casque : {msg}");
                        last_error = msg;
                    }
                    None
                }
            };
            if let Some(h) = &now {
                low.update(&app, "Casque", h.level, h.charging);
            }
            let state = app.state::<HeadsetState>();
            state.headset.set(&app, now);
            let wait = match switch.retry_in(horizon_core::time::now_ms()) {
                Some(ms) if fallback.is_some() => Duration::from_millis(ms),
                _ if fallback.is_some() => PERIOD_SWITCH,
                _ => PERIOD,
            };
            state.wake.wait(wait);
        }
    })
}

/// Bascule la sortie audio si le casque vient de s'éteindre ou de se rallumer ; le dit dans
/// l'îlot.
/// `present` : récepteurs HID (fabricant, produit USB), pour reconnaître la sortie du casque.
fn follow_headset(app: &AppHandle, switch: &mut HeadsetSwitch, on: bool, present: &[(u16, u16)], fallback: &str) {
    let (outputs, current) = match horizon_sys::mixer::outputs_once() {
        Ok(o) => o,
        Err(e) => return log::warn!("casque : lecture des sorties audio : {e:#}"),
    };
    let headset_output = present.iter().find_map(|&(vid, pid)| horizon_sys::mixer::outputs_of_usb(vid, pid).into_iter().next());
    let now = horizon_core::time::now_ms();
    let Some(s) = switch.observe(on, current.as_deref(), fallback, headset_output.as_deref(), now) else { return };
    let (id, title) = match &s {
        Switch::ToFallback(id) => (id, "Casque éteint"),
        Switch::Back(id) => (id, "Casque rallumé"),
    };
    let name = outputs.iter().find(|d| &d.id == id).map_or("sortie inconnue", |d| d.name.as_str());
    match horizon_sys::mixer::set_default_once(id) {
        Ok(()) => {
            log::info!("{title} : sortie audio → {name}");
            let alert = alerts::Alert::icon("headset", Severity::Info, title.into(), format!("Son sur {name}"));
            alerts::show(app, AlertKind::Feedback, alert);
        }
        Err(e) => log::warn!("{title} : bascule vers {name} impossible : {e:#}"),
    }
}

/// Casque affiché : `None` s'il est éteint.
pub fn current(app: &AppHandle) -> Option<Headset> {
    app.state::<HeadsetState>().headset.get()
}

/// Niveau en couleur d'alerte : sous le seuil de « batterie faible » (Réglages d'Horizon ›
/// Alertes, 15 % s'il est coupé), hors charge (`lib/battery.ts`, `isLow`).
pub fn is_low(level: u8, charging: bool, threshold: Option<u8>) -> bool {
    !charging && level <= threshold.unwrap_or(LOW_DEFAULT)
}

/// Seuil par défaut (`LOW_DEFAULT` de `lib/battery.ts`, `DEFAULT_THRESHOLD` de
/// `horizon_core::low_battery`).
const LOW_DEFAULT: u8 = 15;

/// Infobulle : « PRO X 2 — 92 %, en charge ».
pub fn title(h: &Headset) -> String {
    format!("{} — {} %{}", h.name, h.level, if h.charging { ", en charge" } else { "" })
}

/// Pose l'état du casque dans une barre (fil de l'interface).
pub fn apply(app: &AppHandle, window: &BarWindow, h: Option<&Headset>) {
    let g = window.global::<crate::ui::HeadsetData>();
    let Some(h) = h else {
        g.set_on(false);
        return;
    };
    let threshold = app.state::<crate::state::AppState>().read(|l| crate::state::settings().alerts.headset_threshold(l));
    g.set_level(i32::from(h.level));
    g.set_charging(h.charging);
    g.set_low(is_low(h.level, h.charging, threshold));
    g.set_title(title(h).into());
    g.set_on(true);
}

/// Relie le widget d'une barre au service (fil de l'interface).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _bar: &BarLayout) {
    apply(app, window, current(app).as_ref());
    let (a, w) = (app.clone(), window.as_weak());
    crate::host::listen(EV_HEADSET, Some(label), move |e| {
        if let (Some(h), Some(w)) = (e.payload::<Option<Headset>>(), w.upgrade()) {
            apply(&a, &w, h.as_ref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_follows_the_threshold_and_charging() {
        assert!(is_low(15, false, Some(15)));
        assert!(!is_low(16, false, Some(15)));
        assert!(!is_low(5, true, Some(15)), "en charge : jamais en alerte");
        assert!(is_low(15, false, None), "alerte coupée : couleur au seuil par défaut");
        assert!(is_low(30, false, Some(30)));
    }

    #[test]
    fn title_reads_like_the_tauri_one() {
        let h = Headset { name: "PRO X 2".into(), level: 92, charging: false };
        assert_eq!(title(&h), "PRO X 2 — 92 %");
        assert_eq!(title(&Headset { charging: true, ..h }), "PRO X 2 — 92 %, en charge");
    }
}
