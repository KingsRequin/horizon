//! Bulle de molette d'une barre (`ui/wheel-hint.slint`, lib/wheel.svelte.ts) : volume général,
//! avance dans la lecture. Elle disparaît peu après le dernier cran. Fil de l'interface.

use super::{BarWindow, WheelHintState, icons};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

const SHOWN: Duration = Duration::from_millis(1100);

thread_local! {
    /// Minuteur de disparition de la bulle de chaque barre.
    static TIMERS: RefCell<HashMap<String, slint::Timer>> = RefCell::default();
}

/// Montre la bulle de la barre `label` à l'abscisse `x` (px logiques) ; `value` : jauge de 0 à 1.
pub fn show(window: &BarWindow, label: &str, x: f32, icon: &str, text: &str, value: Option<f32>) {
    let g = window.global::<WheelHintState>();
    g.set_x(x);
    g.set_icon(icons::image(icon));
    g.set_text(text.into());
    g.set_value(value.unwrap_or(-1.0));
    g.set_shown(true);
    let weak = window.as_weak();
    TIMERS.with_borrow_mut(|t| {
        t.entry(label.to_owned()).or_default().start(slint::TimerMode::SingleShot, SHOWN, move || {
            if let Some(w) = weak.upgrade() {
                w.global::<WheelHintState>().set_shown(false);
            }
        })
    });
}

/// La bulle est encore affichée (crans successifs : cumuler).
pub fn shown(window: &BarWindow) -> bool {
    window.global::<WheelHintState>().get_shown()
}

/// Barre fermée.
pub fn forget(label: &str) {
    let _ = TIMERS.try_with(|t| t.borrow_mut().remove(label));
}
