//! Partie interface d'Essaim (`ui/widgets/essaim/widget.slint`) : état (`EssaimData`, règle
//! `statusOf` d'essaim.ts) et pause au clic.

use super::{EV_ESSAIM, Essaim, essaim_set_paused, essaim_state};
use crate::host::{self, AppHandle};
use crate::ui::{BarWindow, EssaimData};
use horizon_core::layout::BarLayout;
use slint::ComponentHandle;

/// 0 absent, 1 en pause, 2 en veille (plein écran), 3 actif : la pause de l'utilisateur
/// l'emporte sur la veille.
pub fn status(e: &Essaim) -> i32 {
    if !e.connected {
        0
    } else if e.paused {
        1
    } else if e.fullscreen || !e.active {
        2
    } else {
        3
    }
}

fn show(w: &BarWindow, e: &Essaim) {
    let d = w.global::<EssaimData>();
    d.set_status(status(e));
    d.set_paused(e.paused);
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    let a = app.clone();
    window.global::<EssaimData>().on_set_paused(move |paused| {
        if let Err(e) = essaim_set_paused(a.clone(), paused) {
            log::info!("Essaim : {e}");
        }
    });
    show(window, &essaim_state(app.clone()));
    let w = window.as_weak();
    host::listen(EV_ESSAIM, Some(label), move |e| {
        if let (Some(x), Some(w)) = (e.payload::<Essaim>(), w.upgrade()) {
            show(&w, x);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_wins_over_standby() {
        let on = Essaim { connected: true, active: true, ..Essaim::default() };
        assert_eq!(status(&Essaim::default()), 0);
        assert_eq!(status(&Essaim { paused: true, fullscreen: true, ..on.clone() }), 1);
        assert_eq!(status(&Essaim { fullscreen: true, ..on.clone() }), 2);
        assert_eq!(status(&Essaim { active: false, ..on.clone() }), 2);
        assert_eq!(status(&on), 3);
    }
}
