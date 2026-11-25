//! Accent de chaque fenêtre (`Theme.accent`, un global par fenêtre en Slint) : barres, panneau
//! surgissant, fenêtres d'essai. Suit `EV_ACCENT` (`accent.rs`) ; `None` = orange d'origine.
//! Changement en fondu de `--dur-color` (700 ms, `ease-out`), comme la transition de `--accent`
//! d'app.css : la couleur est posée ~30 fois par seconde pendant le fondu seulement, et d'un coup
//! si les animations sont réduites. Tout ce qui dérive de l'accent (`accent-soft`, mélanges)
//! suit, puisque calculé depuis `Theme.accent`. Fil de l'interface.

use super::Theme;
use crate::events::EV_ACCENT;
use crate::host::{self, AppHandle};
use slint::{Color, ComponentHandle};
use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Orange d'origine (`--accent` d'app.css).
pub const DEFAULT: [u8; 3] = [0xf9, 0x73, 0x16];
/// `--dur-color`.
const FADE: Duration = Duration::from_millis(700);
const STEP: Duration = Duration::from_millis(33);

/// Pose la couleur dans une fenêtre ; faux si elle n'existe plus.
type Setter = Box<dyn Fn(Color) -> bool>;

struct Fade {
    from: [u8; 3],
    to: [u8; 3],
    start: Instant,
    /// Couleur posée en ce moment.
    shown: [u8; 3],
}

thread_local! {
    static WINDOWS: RefCell<Vec<Setter>> = RefCell::default();
    static STATE: RefCell<Fade> = RefCell::new(Fade { from: DEFAULT, to: DEFAULT, start: Instant::now(), shown: DEFAULT });
    static TIMER: slint::Timer = slint::Timer::default();
}

fn color([r, g, b]: [u8; 3]) -> Color {
    Color::from_rgb_u8(r, g, b)
}

/// Couleur du fondu au temps `t` (0–1) : mélange en sRGB, courbe `ease-out`.
pub fn blend(from: [u8; 3], to: [u8; 3], t: f32) -> [u8; 3] {
    let k = crate::motion::ease_out(t);
    std::array::from_fn(|i| (from[i] as f32 + (to[i] as f32 - from[i] as f32) * k).round() as u8)
}

/// Suit l'accent (fil de l'interface, avant l'ouverture des fenêtres).
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let initial = crate::accent::accent_state(app).unwrap_or(DEFAULT);
    STATE.with_borrow_mut(|s| *s = Fade { from: initial, to: initial, start: Instant::now(), shown: initial });
    host::listen(EV_ACCENT, None, |e| {
        if let Some(c) = e.payload::<Option<[u8; 3]>>() {
            set_target(c.unwrap_or(DEFAULT));
        }
    });
    Ok(())
}

/// Couleur posée en ce moment dans les fenêtres (pendant un fondu : celle de l'instant).
pub fn shown() -> [u8; 3] {
    STATE.with_borrow(|s| s.shown)
}

/// Une fenêtre s'ouvre : elle prend l'accent courant, puis suit ses changements. `set` pose la
/// couleur dans son global `Theme` (renvoie faux quand la fenêtre a disparu).
pub fn register(set: impl Fn(Color) -> bool + 'static) {
    let shown = STATE.with_borrow(|s| s.shown);
    if set(color(shown)) {
        WINDOWS.with_borrow_mut(|w| w.push(Box::new(set)));
    }
}

/// La fenêtre Slint `window` suit l'accent (son global `Theme`) et le réglage Windows « Effets
/// d'animation » (durées du thème nulles quand il est coupé), relu à chaque changement d'accent.
pub fn follow<T>(window: &T)
where
    T: ComponentHandle + 'static,
    for<'a> Theme<'a>: slint::Global<'a, T>,
{
    let weak = window.as_weak();
    register(move |c| {
        weak.upgrade()
            .map(|w| {
                let theme = w.global::<Theme>();
                theme.set_reduced_motion(crate::motion::reduced());
                theme.set_accent(c);
            })
            .is_some()
    });
}

fn apply(c: [u8; 3]) {
    WINDOWS.with_borrow_mut(|w| w.retain(|set| set(color(c))));
}

fn set_target(to: [u8; 3]) {
    let instant = crate::motion::reduced();
    let changed = STATE.with_borrow_mut(|s| {
        if s.to == to {
            return false;
        }
        *s = Fade { from: s.shown, to, start: Instant::now(), shown: if instant { to } else { s.shown } };
        true
    });
    if !changed {
        return;
    }
    if instant {
        TIMER.with(slint::Timer::stop);
        return apply(to);
    }
    TIMER.with(|t| t.start(slint::TimerMode::Repeated, STEP, step));
    step();
}

/// Une image du fondu ; le minuteur s'arrête à la fin.
fn step() {
    let (c, done) = STATE.with_borrow_mut(|s| {
        let t = s.start.elapsed().as_secs_f32() / FADE.as_secs_f32();
        let c = if t >= 1.0 { s.to } else { blend(s.from, s.to, t) };
        let changed = c != s.shown;
        s.shown = c;
        (changed.then_some(c), t >= 1.0)
    });
    if let Some(c) = c {
        apply(c);
    }
    if done {
        TIMER.with(slint::Timer::stop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_goes_from_one_colour_to_the_other() {
        let (a, b) = (DEFAULT, [40, 120, 220]);
        assert_eq!(blend(a, b, 0.0), a);
        assert_eq!(blend(a, b, 1.0), b);
        let mid = blend(a, b, 0.5);
        assert!(mid[2] > 150, "ease-out : déjà presque arrivé à mi-temps ({mid:?})");
        assert_eq!(blend(b, b, 0.3), b);
    }
}
