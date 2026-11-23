//! Masquage en plein écran : la barre d'un écran se cache quand la fenêtre du dessus y est en
//! plein écran, réapparaît quand la souris s'arrête dans la bande du haut de cet écran
//! (`[autohide]` de `settings.toml` ; jamais si `reveal` est coupé ou si le curseur est caché,
//! comme dans un jeu qui capture la souris), et se recache 600 ms après que la souris l'a
//! quittée. La place réservée (appbar) est gardée : les fenêtres plein écran l'ignorent, et les
//! fenêtres maximisées ne sont pas redimensionnées.
//!
//! L'ordre des fenêtres n'est relu que sur un événement (appli passée en plein écran ou sortie,
//! avis `ABN_FULLSCREENAPP` des appbars ; changement de premier plan) et, par sécurité, toutes
//! les 2 s. Le curseur n'est scruté (100 ms) que tant qu'un plein écran est en cours. Rien ne
//! tourne pendant une absence (session verrouillée, écran éteint).
//!
//! Repris de `app/src-tauri/src/autohide.rs`.

use crate::util::{Waker, lock, start_thread};
use crate::bars::Bars;
use horizon_core::autohide::{Params, Visibility, forget_gone, next};
use horizon_core::geometry::{MonitorInfo, fullscreen_monitors};
use horizon_core::settings::AutohideSettings;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use crate::host::{AppHandle, Manager};

/// Filet de sécurité : relecture sans événement.
const SAFETY_POLL: Duration = Duration::from_secs(2);
/// Plein écran en cours : le curseur est suivi pour la réapparition.
const FULLSCREEN_POLL: Duration = Duration::from_millis(100);
const HIDE_DELAY: Duration = Duration::from_millis(600);
/// Relecture de `settings.toml` (modifiable à la main pendant que Horizon tourne).
const SETTINGS_PERIOD: Duration = Duration::from_secs(5);
/// Vérification du fond acrylique pendant un plein écran (sinon à chaque événement).
const BACKDROP_PERIOD: Duration = Duration::from_secs(2);

/// Réveil du fil de masquage (appli passée en plein écran, voir `bars::watch`).
#[derive(Default)]
pub struct Autohide {
    wake: Waker,
}

impl Autohide {
    pub fn wake(&self) {
        self.wake.wake();
    }
}

/// `height` : hauteur de la barre de cet écran (pixels logiques).
fn params(m: &MonitorInfo, s: &AutohideSettings, height: u32) -> Params {
    let px = |v: u32| (v as f32 * m.scale).round() as i32;
    Params {
        zone: px(s.reveal_px),
        bar: px(height),
        dwell: Duration::from_millis(s.reveal_ms),
        hide_delay: HIDE_DELAY,
    }
}

/// Une relecture ; vrai si un écran a une fenêtre plein écran (le curseur doit être suivi).
fn step(app: &AppHandle, states: &mut HashMap<String, Visibility>, settings: &AutohideSettings, backdrop: bool) -> bool {
    let bars = app.state::<Bars>();
    let list: Vec<(String, MonitorInfo, isize, u32)> = {
        let l = lock(&bars.list);
        l.iter().map(|b| (b.label.clone(), b.monitor.clone(), b.hwnd, b.height)).collect()
    };
    // Nos fenêtres.
    let mut own: Vec<isize> = list.iter().map(|(_, _, h, _)| *h).collect();
    // Panneau surgissant compris : ouvert au-dessus d'un plein écran, il ne doit pas passer pour
    // la fenêtre du dessus (la barre ne se masquerait plus).
    if let Some(p) = app.get_window(crate::popup::LABEL) {
        own.push(p.hwnd());
    }
    // Garde : l'acrylique revient parfois sur nos fenêtres (cause non identifiée).
    if backdrop {
        for h in &own {
            if horizon_sys::appbar::ensure_no_backdrop(*h) {
                log::info!("acrylique ou bordure DWM réapparus sur {h:#x} : retirés");
            }
        }
    }
    // Barres reconstruites (écran branché, débranché) : les anciens noms sont oubliés.
    let labels: Vec<&str> = list.iter().map(|(l, _, _, _)| l.as_str()).collect();
    forget_gone(states, &labels);
    let monitors: Vec<MonitorInfo> = list.iter().map(|(_, m, _, _)| m.clone()).collect();
    let windows = horizon_sys::fullscreen::windows_z_order(&own);
    let full = fullscreen_monitors(&windows, &monitors);
    // Mode jeu : plein écran sur l'écran principal, que sa barre se masque ou non.
    let main = app.state::<crate::state::AppState>().read(|l| {
        l.bars.iter().find(|b| b.role == horizon_core::layout::BarRole::Main).map(|b| b.monitor_id.clone())
    });
    let main_full = main.is_some_and(|m| full.contains(&m.as_str()));
    crate::game::update(app, main_full);
    crate::osd::set_main_fullscreen(main_full);
    // Barres qui restent visibles au-dessus d'un plein écran (réglage de la barre).
    let kept: Vec<String> = app
        .state::<crate::state::AppState>()
        .read(|l| l.bars.iter().filter(|b| !b.autohide).map(|b| b.monitor_id.clone()).collect());
    // Seul un curseur affiché fait revenir une barre : caché par un jeu qui capture la souris, il
    // bute en haut de l'écran dès que le joueur lève la vue.
    let cursor = settings.reveal.then(horizon_sys::fullscreen::shown_cursor).flatten();
    let mut any_full = false;
    for (label, m, hwnd, height) in &list {
        let Some(w) = app.get_window(label) else { continue };
        let now = states.get(label).copied().unwrap_or(Visibility::Shown);
        let fullscreen = full.contains(&m.id.as_str()) && !kept.contains(&m.id);
        any_full |= fullscreen;
        let next = next(now, fullscreen, cursor, &m.rect, &params(m, settings, *height), Instant::now());
        if now.visible() != next.visible() {
            let _ = if next.visible() { crate::bars::show(&w, *hwnd) } else { crate::bars::hide(&w) };
            app.state::<crate::widgets::media::audio::AudioHub>().set_hidden(label, !next.visible());
            let what = match next {
                Visibility::Shown => "affichée",
                Visibility::Revealed(_) => "affichée au-dessus du plein écran (souris en haut de l'écran)",
                Visibility::Hidden(_) => "masquée (plein écran)",
            };
            log::info!("{label} : {what}");
        }
        states.insert(label.clone(), next);
    }
    any_full
}

/// Démarre la surveillance sur un fil dédié.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let foreground = crate::foreground::subscribe(app);
    let app = app.clone();
    start_thread("autohide", move || {
        let hub = app.state::<Autohide>();
        let mut states = HashMap::new();
        let mut settings = crate::state::settings().autohide;
        let mut read_at = Instant::now();
        let mut backdrop_at: Option<Instant> = None;
        let mut woken = true;
        loop {
            horizon_sys::presence::wait_while_away();
            if read_at.elapsed() >= SETTINGS_PERIOD {
                settings = crate::state::settings().autohide;
                read_at = Instant::now();
            }
            let backdrop = woken || backdrop_at.is_none_or(|t| t.elapsed() >= BACKDROP_PERIOD);
            if backdrop {
                backdrop_at = Some(Instant::now());
            }
            let any_full = step(&app, &mut states, &settings, backdrop);
            let wait = if any_full { FULLSCREEN_POLL } else { SAFETY_POLL };
            woken = crossbeam_channel::select! {
                recv(foreground) -> msg => msg.is_ok() || {
                    // Crochet du premier plan arrêté : le filet de sécurité reste seul.
                    std::thread::sleep(wait);
                    false
                },
                recv(hub.wake.receiver()) -> _ => true,
                default(wait) => false,
            };
            // Événements arrivés en rafale : une seule relecture.
            while foreground.try_recv().is_ok() || hub.wake.receiver().try_recv().is_ok() {}
        }
    })
}
