//! Fenêtres d'application ouvertes (widget « Fenêtres », palette « Ramener une fenêtre ici ») :
//! lesquelles compter, leur écran, leur groupement par écran, et la place d'une fenêtre ramenée
//! sur un autre écran. Tout est en pixels physiques (coordonnées du bureau virtuel).

use crate::geometry::{MonitorInfo, Rect, screen_names};
use serde::Serialize;

/// Ce que Windows dit d'une fenêtre de premier niveau.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowFacts {
    pub visible: bool,
    /// Masquée par DWM : autre bureau virtuel, appli suspendue, fenêtre fantôme.
    pub cloaked: bool,
    /// `WS_EX_TOOLWINDOW` : palette, bulle, fenêtre d'outil.
    pub tool: bool,
    /// `WS_EX_APPWINDOW` : la fenêtre demande sa place dans la barre des tâches.
    pub app_window: bool,
    /// `WS_EX_NOACTIVATE` : ne prend jamais le focus (surcouches).
    pub no_activate: bool,
    /// Possédée par une autre fenêtre (boîte de dialogue, fenêtre secondaire).
    pub owned: bool,
    /// Fenêtre d'Horizon lui-même.
    pub own_process: bool,
    /// Surface du shell (bureau, barre des tâches, menu Démarrer…).
    pub shell: bool,
    pub title: String,
}

/// Vraie fenêtre d'application, comme la barre des tâches les montre : visible, non masquée,
/// titrée, ni outil, ni Horizon, ni shell ; une fenêtre possédée ou qui ne prend pas le focus
/// seulement si elle demande sa place dans la barre des tâches. Une fenêtre réduite compte.
pub fn is_app_window(f: &WindowFacts) -> bool {
    if !f.visible || f.cloaked || f.tool || f.own_process || f.shell || f.title.trim().is_empty() {
        return false;
    }
    f.app_window || (!f.owned && !f.no_activate)
}

/// Une fenêtre listée dans le panneau et la palette.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowView {
    /// Poignée de la fenêtre (nombre : l'interface la renvoie telle quelle).
    pub hwnd: i64,
    pub title: String,
    /// Nom de l'appli (« Vivaldi »).
    pub app: String,
    /// Cible dont on tire l'icône : exe, ou `shell:AppsFolder\<AUMID>` d'une appli empaquetée.
    pub icon: String,
    pub minimized: bool,
    /// Écran (identifiant stable de `MonitorInfo`).
    pub monitor: String,
}

/// Fenêtres d'un écran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScreenGroup {
    pub monitor_id: String,
    /// « Écran principal », « Écran de droite »…
    pub name: String,
    /// L'écran d'où l'on regarde (celui de la barre ou de la palette) : un clic n'y déplace rien.
    pub here: bool,
    pub windows: Vec<WindowView>,
}

fn area(a: Rect, b: Rect) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0 || h <= 0 { 0 } else { i64::from(w) * i64::from(h) }
}

fn center(r: Rect) -> (i64, i64) {
    (i64::from(r.x) + i64::from(r.w) / 2, i64::from(r.y) + i64::from(r.h) / 2)
}

/// Écran d'un rectangle : celui qui en couvre la plus grande part, sinon le plus proche de son
/// centre (fenêtre hors de tout écran).
pub fn monitor_of(r: Rect, monitors: &[MonitorInfo]) -> Option<&MonitorInfo> {
    let best = monitors.iter().max_by_key(|m| area(r, m.rect)).filter(|m| area(r, m.rect) > 0);
    best.or_else(|| {
        let (cx, cy) = center(r);
        monitors.iter().min_by_key(|m| {
            let (mx, my) = center(m.rect);
            (mx - cx).pow(2) + (my - cy).pow(2)
        })
    })
}

/// Groupe les fenêtres par écran, l'écran `here` d'abord, puis l'écran principal, puis les
/// autres dans l'ordre de Windows ; un écran sans fenêtre n'apparaît pas. Dans un groupe,
/// l'ordre reçu (empilement, la plus haute d'abord) est gardé. Une fenêtre d'un écran inconnu
/// (débranché entre-temps) va sur l'écran principal.
pub fn group_by_screen(windows: Vec<WindowView>, monitors: &[MonitorInfo], here: Option<&str>) -> Vec<ScreenGroup> {
    let names = screen_names(monitors);
    let primary = monitors.iter().find(|m| m.primary).or(monitors.first()).map(|m| m.id.clone());
    let mut order: Vec<&MonitorInfo> = monitors.iter().collect();
    order.sort_by_key(|m| (Some(m.id.as_str()) != here, !m.primary));
    let mut groups: Vec<ScreenGroup> = order
        .iter()
        .map(|m| ScreenGroup {
            monitor_id: m.id.clone(),
            name: names.iter().find(|(id, _)| *id == m.id).map(|(_, n)| n.clone()).unwrap_or_default(),
            here: Some(m.id.as_str()) == here,
            windows: Vec::new(),
        })
        .collect();
    for w in windows {
        let i = groups
            .iter()
            .position(|g| g.monitor_id == w.monitor)
            .or_else(|| groups.iter().position(|g| Some(&g.monitor_id) == primary.as_ref()));
        if let Some(i) = i {
            groups[i].windows.push(w);
        }
    }
    groups.retain(|g| !g.windows.is_empty());
    groups
}

/// Bords invisibles d'une fenêtre (zone de redimensionnement de Windows 10/11) : écart entre
/// son rectangle et son cadre visible, en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Insets {
    /// Bords d'une fenêtre d'après son rectangle et son cadre visible (jamais négatifs).
    pub fn between(window: Rect, frame: Rect) -> Insets {
        Insets {
            left: (frame.x - window.x).max(0),
            top: (frame.y - window.y).max(0),
            right: ((window.x + window.w) - (frame.x + frame.w)).max(0),
            bottom: ((window.y + window.h) - (frame.y + frame.h)).max(0),
        }
    }

    /// Les mêmes bords à une autre échelle (`ratio` = nouvelle échelle / ancienne).
    pub fn scaled(self, ratio: f64) -> Insets {
        let s = |v: i32| (f64::from(v) * ratio).round() as i32;
        Insets { left: s(self.left), top: s(self.top), right: s(self.right), bottom: s(self.bottom) }
    }

    /// Rectangle de fenêtre à donner à Windows pour obtenir le cadre visible `frame`.
    pub fn around(self, frame: Rect) -> Rect {
        Rect {
            x: frame.x - self.left,
            y: frame.y - self.top,
            w: frame.w + self.left + self.right,
            h: frame.h + self.top + self.bottom,
        }
    }
}

/// Cadre visible d'une fenêtre ramenée de l'écran `from` sur l'écran `to` : même taille et même
/// décalage par rapport au coin de la zone de travail, en pixels logiques (une fenêtre de
/// 800 × 600 à 100 % devient 1200 × 900 physiques à 150 %). Trop grande pour la zone de
/// travail d'arrivée : réduite pour y tenir ; décalée si elle en dépasserait.
pub fn target_frame(frame: Rect, from: &MonitorInfo, to: &MonitorInfo) -> Rect {
    let ratio = f64::from(to.scale) / f64::from(from.scale.max(0.1));
    let s = |v: i32| (f64::from(v) * ratio).round() as i32;
    let work = to.work;
    let w = s(frame.w).clamp(1, work.w.max(1));
    let h = s(frame.h).clamp(1, work.h.max(1));
    let dx = s(frame.x - from.work.x).clamp(0, (work.w - w).max(0));
    let dy = s(frame.y - from.work.y).clamp(0, (work.h - h).max(0));
    Rect { x: work.x + dx, y: work.y + dy, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(title: &str) -> WindowFacts {
        WindowFacts { visible: true, title: title.into(), ..Default::default() }
    }

    #[test]
    fn only_real_application_windows_count() {
        assert!(is_app_window(&facts("Vivaldi")));
        assert!(!is_app_window(&WindowFacts { visible: false, ..facts("cachée") }));
        assert!(!is_app_window(&WindowFacts { cloaked: true, ..facts("autre bureau") }));
        assert!(!is_app_window(&WindowFacts { tool: true, ..facts("palette") }));
        assert!(!is_app_window(&WindowFacts { tool: true, app_window: true, ..facts("outil") }));
        assert!(!is_app_window(&WindowFacts { own_process: true, ..facts("Horizon") }));
        assert!(!is_app_window(&WindowFacts { shell: true, ..facts("Program Manager") }));
        assert!(!is_app_window(&facts("")));
        assert!(!is_app_window(&facts("   ")));
        // Fenêtre possédée (boîte de dialogue) : seulement si elle demande sa place.
        assert!(!is_app_window(&WindowFacts { owned: true, ..facts("Enregistrer sous") }));
        assert!(is_app_window(&WindowFacts { owned: true, app_window: true, ..facts("Lecteur") }));
        assert!(!is_app_window(&WindowFacts { no_activate: true, ..facts("Surcouche") }));
    }

    /// Les écrans de l'utilisateur : D1 1080p à 100 % (barre de 40 px), D2 4K à 150 % au-dessus
    /// (barre de 60 px), portrait à droite et 1080p à gauche.
    fn screens() -> Vec<MonitorInfo> {
        let m = |id: &str, x, y, w, h, bar, scale: f32, primary| MonitorInfo {
            id: id.into(),
            device_name: id.into(),
            rect: Rect { x, y, w, h },
            work: Rect { x, y: y + bar, w, h: h - bar },
            scale,
            primary,
            portrait: h > w,
        };
        vec![
            m("main", 0, 0, 1920, 1080, 40, 1.0, true),
            m("right", 1920, -853, 1080, 1920, 40, 1.0, false),
            m("top", -1936, -2160, 3840, 2160, 60, 1.5, false),
            m("left", -1920, 0, 1920, 1080, 40, 1.0, false),
        ]
    }

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn monitor_of_takes_the_largest_share_then_the_nearest() {
        let s = screens();
        assert_eq!(monitor_of(r(100, 100, 800, 600), &s).unwrap().id, "main");
        // À cheval : la plus grande part (ici sur l'écran de droite).
        assert_eq!(monitor_of(r(1800, 100, 800, 600), &s).unwrap().id, "right");
        assert_eq!(monitor_of(r(-1000, -1500, 800, 600), &s).unwrap().id, "top");
        // Hors de tout écran : le plus proche.
        assert_eq!(monitor_of(r(100, 1200, 300, 200), &s).unwrap().id, "main");
        assert!(monitor_of(r(0, 0, 10, 10), &[]).is_none());
    }

    fn win(hwnd: i64, monitor: &str) -> WindowView {
        WindowView {
            hwnd,
            title: format!("f{hwnd}"),
            app: "App".into(),
            icon: String::new(),
            minimized: false,
            monitor: monitor.into(),
        }
    }

    #[test]
    fn windows_are_grouped_by_named_screen() {
        let s = screens();
        let list = vec![win(1, "top"), win(2, "main"), win(3, "top"), win(4, "gone"), win(5, "left")];
        let g = group_by_screen(list.clone(), &s, None);
        let names: Vec<&str> = g.iter().map(|g| g.name.as_str()).collect();
        // Principal d'abord, puis l'ordre de Windows ; l'écran de droite, vide, n'apparaît pas.
        assert_eq!(names, ["Écran principal", "Écran du haut", "Écran de gauche"]);
        // Ordre d'empilement gardé ; écran inconnu → écran principal.
        assert_eq!(g[0].windows.iter().map(|w| w.hwnd).collect::<Vec<_>>(), [2, 4]);
        assert_eq!(g[1].windows.iter().map(|w| w.hwnd).collect::<Vec<_>>(), [1, 3]);
        assert!(g.iter().all(|g| !g.here));
        // Vu depuis la barre du haut : son écran d'abord, marqué « ici ».
        let g = group_by_screen(list, &s, Some("top"));
        assert_eq!(g[0].monitor_id, "top");
        assert!(g[0].here);
        assert_eq!(g[1].monitor_id, "main");
        assert!(group_by_screen(Vec::new(), &s, None).is_empty());
    }

    #[test]
    fn insets_round_trip() {
        let window = r(93, 33, 814, 607);
        let frame = r(100, 40, 800, 593);
        let i = Insets::between(window, frame);
        assert_eq!(i, Insets { left: 7, top: 7, right: 7, bottom: 7 });
        assert_eq!(i.around(frame), window);
        assert_eq!(i.scaled(1.5), Insets { left: 11, top: 11, right: 11, bottom: 11 });
        // Cadre plus grand que la fenêtre (lecture incohérente) : pas de bord négatif.
        assert_eq!(Insets::between(frame, window), Insets::default());
    }

    #[test]
    fn same_logical_size_and_offset_on_a_screen_with_another_scale() {
        let s = screens();
        let (main, top) = (&s[0], &s[2]);
        // 800 × 600 à 100 %, à (100, 50) de la zone de travail → 1200 × 900 à 150 %, à (150, 75).
        let t = target_frame(r(100, 90, 800, 600), main, top);
        assert_eq!(t, r(-1936 + 150, -2160 + 60 + 75, 1200, 900));
        // Et retour : la même fenêtre.
        assert_eq!(target_frame(t, top, main), r(100, 90, 800, 600));
    }

    #[test]
    fn too_big_window_shrinks_to_the_work_area() {
        let s = screens();
        let (main, top, right) = (&s[0], &s[2], &s[1]);
        // 3600 × 2000 physiques à 150 % = 2400 × 1333 logiques : plus grand que D1.
        let t = target_frame(r(-1900, -2080, 3600, 2000), top, main);
        assert_eq!(t, r(0, 40, 1920, 1040));
        // Large fenêtre vers le portrait : réduite en largeur seulement, collée à gauche.
        let t = target_frame(r(200, 140, 1500, 700), main, right);
        assert_eq!(t, r(1920, -853 + 40 + 100, 1080, 700));
    }

    #[test]
    fn window_that_would_overflow_is_shifted_inside() {
        let s = screens();
        let (main, right) = (&s[0], &s[1]);
        // À 1000 px du bord gauche de D1, 900 px de large : décalée pour tenir sur le portrait.
        let t = target_frame(r(1000, 40, 900, 500), main, right);
        assert_eq!(t, r(1920 + 180, -853 + 40, 900, 500));
        // Dépassant à gauche de sa zone de travail : ramenée au bord.
        let t = target_frame(r(-50, 20, 400, 300), main, right);
        assert_eq!((t.x, t.y), (1920, -813));
    }
}
