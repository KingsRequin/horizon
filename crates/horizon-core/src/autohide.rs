//! Masquage en plein écran : quand la barre d'un écran se cache, et quand la souris la fait
//! réapparaître.
//!
//! La zone de réapparition est une bande en haut de l'écran : un simple contact avec le bord
//! ne suffit pas quand un autre écran est au-dessus (la souris passe dessus au lieu de s'arrêter).
//! La souris doit y rester un court instant, pour que la traverser ne fasse rien apparaître.

use crate::geometry::Rect;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility {
    Shown,
    /// Cachée ; `Some` = souris dans la zone de réapparition depuis cet instant.
    Hidden(Option<Instant>),
    /// Affichée par-dessus le plein écran ; `Some` = souris sortie depuis cet instant.
    Revealed(Option<Instant>),
}

impl Visibility {
    pub fn visible(self) -> bool {
        !matches!(self, Visibility::Hidden(_))
    }
}

/// Mesures en pixels physiques de l'écran concerné.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Hauteur de la zone de réapparition.
    pub zone: i32,
    /// Hauteur de la barre (la souris dessus la garde affichée).
    pub bar: i32,
    /// Présence nécessaire dans la zone avant la réapparition.
    pub dwell: Duration,
    /// Délai avant de recacher la barre que la souris a quittée.
    pub hide_delay: Duration,
}

fn within_x(m: &Rect, x: i32) -> bool {
    x >= m.x && x < m.x + m.w
}

fn in_zone(m: &Rect, (x, y): (i32, i32), height: i32) -> bool {
    within_x(m, x) && y >= m.y - 1 && y < m.y + height.max(2)
}

/// État suivant de la barre d'un écran `m`.
pub fn next(
    now_state: Visibility,
    fullscreen: bool,
    cursor: Option<(i32, i32)>,
    m: &Rect,
    p: &Params,
    now: Instant,
) -> Visibility {
    use Visibility::*;
    if !fullscreen {
        return Shown;
    }
    let in_reveal = cursor.is_some_and(|c| in_zone(m, c, p.zone));
    let over_bar = cursor.is_some_and(|c| in_zone(m, c, p.bar + 4));
    match now_state {
        Shown => Hidden(None),
        Hidden(None) if in_reveal => {
            if p.dwell.is_zero() { Revealed(None) } else { Hidden(Some(now)) }
        }
        Hidden(Some(t)) if in_reveal => {
            if now.duration_since(t) >= p.dwell { Revealed(None) } else { Hidden(Some(t)) }
        }
        Hidden(_) => Hidden(None),
        Revealed(_) if over_bar => Revealed(None),
        Revealed(None) => Revealed(Some(now)),
        Revealed(Some(t)) if now.duration_since(t) >= p.hide_delay => Hidden(None),
        other => other,
    }
}

/// Oublie l'état des fenêtres qui n'existent plus (barres reconstruites sous un autre nom).
pub fn forget_gone<V>(states: &mut HashMap<String, V>, alive: &[&str]) {
    states.retain(|label, _| alive.contains(&label.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilt_bars_forget_old_labels() {
        let mut states: HashMap<String, Visibility> = HashMap::new();
        states.insert("bar-1-0".into(), Visibility::Hidden(None));
        states.insert("bar-1-1".into(), Visibility::Shown);
        states.insert("bar-2-0".into(), Visibility::Shown);
        forget_gone(&mut states, &["bar-2-0", "bar-2-1"]);
        let mut left: Vec<&String> = states.keys().collect();
        left.sort();
        assert_eq!(left, vec!["bar-2-0"]);
    }
    use Visibility::*;

    const M: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
    const P: Params = Params {
        zone: 12,
        bar: 40,
        dwell: Duration::from_millis(150),
        hide_delay: Duration::from_millis(600),
    };

    fn ms(t0: Instant, n: u64) -> Instant {
        t0 + Duration::from_millis(n)
    }

    #[test]
    fn hides_on_fullscreen_and_shows_after() {
        let t = Instant::now();
        assert_eq!(next(Shown, true, None, &M, &P, t), Hidden(None));
        assert_eq!(next(Hidden(None), false, None, &M, &P, t), Shown);
        assert_eq!(next(Revealed(None), false, None, &M, &P, t), Shown);
    }

    #[test]
    fn reveals_after_dwell_in_zone_below_top_edge() {
        let t = Instant::now();
        // 10 px sous le bord : dans la zone (12 px), plus seulement au contact du bord.
        let s = next(Hidden(None), true, Some((500, 10)), &M, &P, t);
        assert_eq!(s, Hidden(Some(t)));
        assert_eq!(next(s, true, Some((500, 8)), &M, &P, ms(t, 100)), Hidden(Some(t)));
        assert_eq!(next(s, true, Some((500, 8)), &M, &P, ms(t, 160)), Revealed(None));
    }

    #[test]
    fn crossing_to_screen_above_does_not_reveal() {
        let t = Instant::now();
        let s = next(Hidden(None), true, Some((500, 5)), &M, &P, t);
        // La souris est passée sur l'écran du dessus avant la fin du délai.
        let s = next(s, true, Some((500, -300)), &M, &P, ms(t, 100));
        assert_eq!(s, Hidden(None));
        assert_eq!(next(s, true, Some((500, -300)), &M, &P, ms(t, 400)), Hidden(None));
    }

    #[test]
    fn outside_zone_or_other_screen_does_nothing() {
        let t = Instant::now();
        assert_eq!(next(Hidden(None), true, Some((500, 30)), &M, &P, t), Hidden(None));
        assert_eq!(next(Hidden(None), true, Some((2500, 5)), &M, &P, t), Hidden(None));
    }

    #[test]
    fn revealed_stays_over_bar_then_hides_after_delay() {
        let t = Instant::now();
        assert_eq!(next(Revealed(Some(t)), true, Some((500, 30)), &M, &P, t), Revealed(None));
        let s = next(Revealed(None), true, Some((500, 300)), &M, &P, t);
        assert_eq!(s, Revealed(Some(t)));
        assert_eq!(next(s, true, Some((500, 300)), &M, &P, ms(t, 300)), s);
        assert_eq!(next(s, true, Some((500, 300)), &M, &P, ms(t, 650)), Hidden(None));
    }

    #[test]
    fn without_a_cursor_nothing_reveals_and_the_bar_hides() {
        // Curseur caché par un jeu, ou réapparition coupée : pas de position.
        let t = Instant::now();
        assert_eq!(next(Hidden(Some(t)), true, None, &M, &P, ms(t, 200)), Hidden(None));
        let s = next(Revealed(None), true, None, &M, &P, t);
        assert_eq!(s, Revealed(Some(t)));
        assert_eq!(next(s, true, None, &M, &P, ms(t, 650)), Hidden(None));
    }

    #[test]
    fn zero_dwell_reveals_at_once() {
        let p = Params { dwell: Duration::ZERO, ..P };
        assert_eq!(next(Hidden(None), true, Some((1, 0)), &M, &p, Instant::now()), Revealed(None));
    }
}
