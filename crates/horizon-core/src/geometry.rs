//! Géométrie des écrans et des barres, en pixels physiques (coordonnées du bureau virtuel).

use serde::{Deserialize, Serialize};

/// Rectangle en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Un écran tel que Windows le décrit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Identifiant stable (chemin matériel de l'écran), à défaut le nom GDI.
    pub id: String,
    /// Nom GDI (`\\.\DISPLAY1`), qui peut changer d'un démarrage à l'autre.
    pub device_name: String,
    pub rect: Rect,
    /// Zone de travail (écran moins les barres réservées).
    pub work: Rect,
    /// Échelle d'affichage (1.0 = 100 %).
    pub scale: f32,
    pub primary: bool,
    pub portrait: bool,
}

/// Rectangle à réserver en haut de l'écran pour une barre de `height_logical` pixels logiques.
pub fn appbar_rect_for_monitor(m: &MonitorInfo, height_logical: u32) -> Rect {
    Rect {
        x: m.rect.x,
        y: m.rect.y,
        w: m.rect.w,
        h: (height_logical as f32 * m.scale).round() as i32,
    }
}

/// Une fenêtre de premier niveau, telle que vue pour la détection du plein écran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinInfo {
    pub rect: Rect,
    /// Fenêtre à ignorer : bureau, barre des tâches, surcouches transparentes, nos barres.
    pub ignorable: bool,
}

/// Vrai si la fenêtre couvre tout l'écran. Une fenêtre simplement maximisée s'arrête sous la
/// place réservée à la barre ; une vidéo plein écran d'un navigateur maximisé reste dite
/// « maximisée » par Windows mais couvre tout : seule la surface compte.
pub(crate) fn is_fullscreen_on(w: &WinInfo, m: &MonitorInfo) -> bool {
    let (r, s) = (w.rect, m.rect);
    !w.ignorable
        && r.x <= s.x
        && r.y <= s.y
        && r.x + r.w >= s.x + s.w
        && r.y + r.h >= s.y + s.h
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

/// Écrans dont la fenêtre visible la plus haute (ordre Z, première = dessus) est en plein écran.
pub fn fullscreen_monitors<'a>(windows: &[WinInfo], monitors: &'a [MonitorInfo]) -> Vec<&'a str> {
    monitors
        .iter()
        .filter(|m| {
            windows
                .iter()
                .find(|w| !w.ignorable && intersects(w.rect, m.rect))
                .is_some_and(|w| is_fullscreen_on(w, m))
        })
        .map(|m| m.id.as_str())
        .collect()
}

/// Les mêmes écrans, aux mêmes places et à la même échelle ? La zone de travail est ignorée :
/// nos propres barres la modifient en réservant leur place. L'ordre de la liste aussi.
pub fn same_screens(a: &[MonitorInfo], b: &[MonitorInfo]) -> bool {
    let key = |m: &MonitorInfo| (m.id.clone(), m.rect, m.scale.to_bits(), m.primary);
    let mut ka: Vec<_> = a.iter().map(key).collect();
    let mut kb: Vec<_> = b.iter().map(key).collect();
    ka.sort_by(|x, y| x.0.cmp(&y.0));
    kb.sort_by(|x, y| x.0.cmp(&y.0));
    ka == kb
}

/// Nom de chaque écran pour l'utilisateur, d'après sa place par rapport à l'écran principal :
/// « Écran principal », « Écran de droite », « Écran de gauche », « Écran du haut », « Écran
/// du bas » (numérotés s'il y en a deux du même côté, le plus proche d'abord).
pub fn screen_names(monitors: &[MonitorInfo]) -> Vec<(String, String)> {
    let center = |m: &MonitorInfo| (i64::from(m.rect.x) + i64::from(m.rect.w) / 2, i64::from(m.rect.y) + i64::from(m.rect.h) / 2);
    let Some(main) = monitors.iter().find(|m| m.primary).or(monitors.first()) else { return Vec::new() };
    let (mx, my) = center(main);
    let mut named: Vec<(String, &'static str, i64)> = monitors
        .iter()
        .map(|m| {
            let (dx, dy) = (center(m).0 - mx, center(m).1 - my);
            let side = if m.id == main.id {
                "Écran principal"
            } else if dy.abs() > dx.abs() {
                if dy < 0 { "Écran du haut" } else { "Écran du bas" }
            } else if dx > 0 {
                "Écran de droite"
            } else {
                "Écran de gauche"
            };
            (m.id.clone(), side, dx * dx + dy * dy)
        })
        .collect();
    named.sort_by_key(|(_, side, dist)| (*side, *dist));
    let mut out = Vec::new();
    for (i, (id, side, _)) in named.iter().enumerate() {
        let same: Vec<_> = named.iter().filter(|(_, s, _)| s == side).collect();
        let name = if same.len() > 1 {
            let rank = named[..=i].iter().filter(|(_, s, _)| s == side).count();
            format!("{side} {rank}")
        } else {
            (*side).to_owned()
        };
        out.push((id.clone(), name));
    }
    // Ordre d'origine des écrans.
    out.sort_by_key(|(id, _)| monitors.iter().position(|m| &m.id == id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screens_are_named_by_their_place() {
        let names = screen_names(&crate::layout::tests::real_monitors());
        let name = |id: &str| names.iter().find(|(i, _)| i == id).unwrap().1.clone();
        assert_eq!(name("main"), "Écran principal");
        assert_eq!(name("right"), "Écran de droite");
        assert_eq!(name("top"), "Écran du haut");
        assert_eq!(name("left"), "Écran de gauche");
        let mut two = crate::layout::tests::real_monitors();
        two.push(crate::layout::tests::mon("far", 3000, 0, 1920, 1080, false));
        let names = screen_names(&two);
        assert_eq!(names.iter().find(|(i, _)| i == "right").unwrap().1, "Écran de droite 1");
        assert_eq!(names.iter().find(|(i, _)| i == "far").unwrap().1, "Écran de droite 2");
        assert!(screen_names(&[]).is_empty());
    }

    fn win(x: i32, y: i32, w: i32, h: i32) -> WinInfo {
        WinInfo { rect: Rect { x, y, w, h }, ignorable: false }
    }

    #[test]
    fn fullscreen_covers_whole_monitor() {
        let m = monitor(0, 0, 1920, 1080, 1.0);
        assert!(is_fullscreen_on(&win(0, 0, 1920, 1080), &m));
        // Un peu plus grand (bordures invisibles) : toujours plein écran.
        assert!(is_fullscreen_on(&win(-8, -8, 1936, 1096), &m));
    }

    #[test]
    fn maximized_window_is_fullscreen_only_when_it_covers_the_bar() {
        let m = monitor(0, 0, 1920, 1080, 1.0);
        // Maximisée : elle s'arrête sous la place réservée à la barre.
        let mut w = win(0, 40, 1920, 1000);
        assert!(!is_fullscreen_on(&w, &m));
        // Vidéo en plein écran dans un navigateur maximisé : Windows la dit encore maximisée,
        // mais elle couvre tout l'écran.
        w.rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
        assert!(is_fullscreen_on(&w, &m));
    }

    #[test]
    fn ignorable_window_is_not_fullscreen() {
        let m = monitor(0, 0, 1920, 1080, 1.0);
        let mut w = win(0, 0, 1920, 1080);
        w.ignorable = true;
        assert!(!is_fullscreen_on(&w, &m));
    }

    #[test]
    fn fullscreen_on_other_monitor_does_not_count() {
        let m = monitor(0, 0, 1920, 1080, 1.0);
        assert!(!is_fullscreen_on(&win(-1920, 0, 1920, 1080), &m));
    }

    #[test]
    fn fullscreen_monitors_uses_topmost_window_per_monitor() {
        let mut main = monitor(0, 0, 1920, 1080, 1.0);
        main.id = "main".into();
        let mut top = monitor(0, -2160, 3840, 2160, 2.0);
        top.id = "top".into();
        let monitors = vec![main, top];
        let mut overlay = win(-2000, -2200, 6000, 4000);
        overlay.ignorable = true;
        let windows = vec![
            overlay,                       // surcouche transparente : ignorée
            win(100, 100, 800, 600),       // fenêtre normale au-dessus sur l'écran principal
            win(0, -2160, 3840, 2160),     // film en plein écran sur l'écran du haut
            win(0, 0, 1920, 1080),         // jeu plein écran sous la fenêtre normale
        ];
        assert_eq!(fullscreen_monitors(&windows, &monitors), vec!["top"]);
        assert!(fullscreen_monitors(&[], &monitors).is_empty());
    }

    fn monitor(x: i32, y: i32, w: i32, h: i32, scale: f32) -> MonitorInfo {
        MonitorInfo {
            id: "m".into(),
            device_name: r"\\.\DISPLAY1".into(),
            rect: Rect { x, y, w, h },
            work: Rect { x, y, w, h },
            scale,
            primary: false,
            portrait: h > w,
        }
    }

    fn named(id: &str, x: i32) -> MonitorInfo {
        MonitorInfo { id: id.into(), ..monitor(x, 0, 1920, 1080, 1.0) }
    }

    #[test]
    fn same_screens_ignores_work_area_and_order() {
        let a = vec![named("a", 0), named("b", 1920)];
        let mut b = vec![named("b", 1920), named("a", 0)];
        // Nos propres barres réservent de la place : la zone de travail change, pas les écrans.
        b[1].work.y = 40;
        assert!(same_screens(&a, &b));
    }

    #[test]
    fn same_screens_sees_added_moved_or_rescaled_screen() {
        let a = vec![named("a", 0)];
        assert!(!same_screens(&a, &[named("a", 0), named("b", 1920)]));
        assert!(!same_screens(&a, &[named("a", 10)]));
        assert!(!same_screens(&a, &[MonitorInfo { scale: 1.5, ..named("a", 0) }]));
        assert!(!same_screens(&a, &[MonitorInfo { primary: true, ..named("a", 0) }]));
    }

    #[test]
    fn appbar_rect_at_100_percent() {
        let r = appbar_rect_for_monitor(&monitor(0, 0, 1920, 1080, 1.0), 40);
        assert_eq!(r, Rect { x: 0, y: 0, w: 1920, h: 40 });
    }

    #[test]
    fn appbar_rect_at_150_percent() {
        let r = appbar_rect_for_monitor(&monitor(1920, 0, 2560, 1440, 1.5), 40);
        assert_eq!(r, Rect { x: 1920, y: 0, w: 2560, h: 60 });
    }

    #[test]
    fn appbar_rect_at_200_percent_above() {
        let r = appbar_rect_for_monitor(&monitor(-1, -2160, 3840, 2160, 2.0), 40);
        assert_eq!(r, Rect { x: -1, y: -2160, w: 3840, h: 80 });
    }

    #[test]
    fn appbar_rect_rounds_fractional_scale() {
        // 40 × 1,25 = 50 ; 40 × 1,75 = 70 ; 40 × 1,1 = 44.
        assert_eq!(appbar_rect_for_monitor(&monitor(0, 0, 1080, 1920, 1.25), 40).h, 50);
        assert_eq!(appbar_rect_for_monitor(&monitor(0, 0, 1080, 1920, 1.1), 40).h, 44);
    }

    #[test]
    fn appbar_rect_portrait_negative_x() {
        let r = appbar_rect_for_monitor(&monitor(-1080, -300, 1080, 1920, 1.0), 40);
        assert_eq!(r, Rect { x: -1080, y: -300, w: 1080, h: 40 });
    }
}
