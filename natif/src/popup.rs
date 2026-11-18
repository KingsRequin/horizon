//! Panneau surgissant : règles et état partagé, repris de `app/src-tauri/src/popup.rs`. La
//! fenêtre et son déroulé (minuteurs, fondus, focus) sont dans `ui/popup.rs`, sur le fil de
//! l'interface ; ce module garde ce qui se teste sans fenêtre (placement, passage d'un widget à
//! l'autre) et ce que lisent les autres fils (`PopupState` : panneau ouvert, écran).
//!
//! Comportement (le même que la version Tauri) :
//! - survol d'un widget = aperçu après 300 ms (`ui::panels`), sans prendre le focus ; il se ferme
//!   `HIDE_DELAY` après la sortie de la souris, sauf si elle entre dans le panneau entre-temps ;
//!   la fermeture joue un fondu de sortie (`CLOSE_ANIM`), annulé si la souris revient ;
//! - clic = panneau épinglé : il prend le focus (Tab, Échap) et ne se ferme plus à la sortie de
//!   la souris, mais à Échap, à la perte du focus (clic ailleurs) ou au clic d'un autre widget ;
//!   un survol ne remplace pas un panneau épinglé ;
//! - passage d'un widget à l'autre : un panneau ouvert est gardé (`keeps`) et le nouveau
//!   s'affiche aussitôt, sans fermeture ni délai ;
//! - taille mesurée sur le contenu ; fenêtre centrée sous le widget, sous la bande du haut
//!   (par-dessus les tuiles d'une barre haute), bornée à l'écran.
//!
//! Différences : pas de fil des fermetures (minuteurs Slint du fil de l'interface), et la mesure
//! est lue aussitôt dans l'interface (pas d'aller-retour avec une page) : la fenêtre s'affiche
//! toujours à la bonne taille.

use crate::host::{AppHandle, Manager};
use crate::util::lock;
use std::sync::Mutex;
use std::time::Duration;

/// Étiquette de la fenêtre surgissante (abonnements des panneaux : `host::listen(…,
/// Some(LABEL), …)`).
pub const LABEL: &str = "popup";
/// Délai avant l'aperçu au survol (`HOVER_MS` de lib/panels.ts).
pub const HOVER_DELAY: Duration = Duration::from_millis(300);
/// Fermeture après la sortie de la souris.
pub const HIDE_DELAY: Duration = Duration::from_millis(250);
/// Fondu de sortie avant que la fenêtre soit cachée : au moins `Theme.dur-fast` (120 ms).
pub const CLOSE_ANIM: Duration = Duration::from_millis(150);
/// Écart entre la barre et le panneau, en pixels logiques.
const GAP: f32 = 6.0;
/// Marge gardée entre le panneau et les bords de l'écran, en pixels logiques.
const MARGIN: f32 = 4.0;
/// Aide du mode édition (`edithelp:<écran>`) : elle reste jusqu'à la fin de l'édition.
pub const HELP_KIND: &str = "edithelp";

/// Ce que les autres fils savent du panneau (mis à jour par `ui::popup`).
#[derive(Default)]
pub struct PopupState {
    kind: Mutex<Option<String>>,
    placement: Mutex<Option<Placement>>,
    /// Genres des panneaux montrés quelque part : fenêtre surgissante ou tuile d'une barre
    /// haute affichée (`ui::panel_hosts`).
    shown: Mutex<Vec<String>>,
}

impl PopupState {
    /// Le panneau `kind` (« system », « weather:w1 ») est ouvert.
    pub fn is_open(&self, kind: &str) -> bool {
        lock(&self.kind).as_deref() == Some(kind)
    }

    /// Un panneau de ce genre est ouvert, quel que soit son widget (« servers:w3 »).
    pub fn is_open_kind(&self, base: &str) -> bool {
        lock(&self.kind).as_deref().is_some_and(|k| base_of(k) == base)
    }

    /// Le panneau de ce genre est montré, dans la fenêtre surgissante ou en tuile (une tuile
    /// Serveurs veut ses latences comme le panneau ouvert).
    pub fn is_shown(&self, base: &str) -> bool {
        lock(&self.shown).iter().any(|b| b == base)
    }

    pub(crate) fn set_shown(&self, bases: Vec<String>) {
        *lock(&self.shown) = bases;
    }

    pub(crate) fn set(&self, kind: Option<&str>, placement: Option<&Placement>) {
        *lock(&self.kind) = kind.map(str::to_owned);
        if let Some(p) = placement {
            *lock(&self.placement) = Some(p.clone());
        }
    }
}

/// Écran (pixels physiques) de la barre d'où vient le panneau en cours ou le dernier ouvert.
pub fn screen(app: &AppHandle) -> Option<horizon_core::geometry::Rect> {
    let p = lock(&app.state::<PopupState>().placement).clone()?;
    let (x, y, w, h) = p.screen;
    Some(horizon_core::geometry::Rect { x, y, w, h })
}

/// Genre du widget d'un panneau : « weather » pour « weather:w1 ».
pub fn base_of(kind: &str) -> &str {
    kind.split(':').next().unwrap_or_default()
}

/// Exemplaire d'un panneau propre à un widget : « w1 » pour « weather:w1 » (tout ce qui suit le
/// premier « : »), vide sinon.
pub fn instance_of(kind: &str) -> &str {
    kind.split_once(':').map_or("", |(_, i)| i)
}

/// Position du panneau demandé, en pixels physiques : centre du widget, haut du panneau et
/// écran qui le contient (x, y, largeur, hauteur).
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub kind: String,
    pub center: i32,
    pub top: i32,
    pub screen: (i32, i32, i32, i32),
    pub scale: f32,
}

/// Rectangle physique du panneau (x, y, largeur, hauteur) pour une taille logique : centré
/// sous le widget, gardé dans l'écran (une hauteur trop grande est bornée au bas de l'écran).
pub fn rect_for(p: &Placement, (w, h): (f32, f32)) -> (i32, i32, u32, u32) {
    let (sx, sy, sw, sh) = p.screen;
    let margin = (MARGIN * p.scale).round() as i32;
    let pw = ((w * p.scale).round() as i32).clamp(1, (sw - 2 * margin).max(1));
    let ph = ((h * p.scale).round() as i32).clamp(1, (sy + sh - margin - p.top).max(1));
    let x = (p.center - pw / 2).clamp(sx + margin, sx + sw - pw - margin);
    (x, p.top, pw as u32, ph as u32)
}

/// Haut du panneau (pixels physiques) : sous le bas de la bande `anchor_y` (pixels logiques de
/// la barre), borné à la fenêtre de barre, plus l'écart. Sans `anchor_y` (aide du mode
/// édition), sous toute la barre.
pub fn panel_top(bar_y: i32, bar_height: u32, anchor_y: Option<f32>, scale: f32) -> i32 {
    let below = anchor_y.map_or(bar_height as i32, |y| ((y * scale).round() as i32).clamp(0, bar_height as i32));
    bar_y + below + (GAP * scale) as i32
}

/// Le panneau ouvert `open` est-il gardé quand la souris entre sur le widget de `kind` ? Oui
/// pour tout panneau de survol ; jamais pour l'aide du mode édition ; un panneau épinglé, par
/// son seul widget.
pub fn keeps(open: Option<&str>, kind: &str, pinned: bool) -> bool {
    match open {
        Some(o) if base_of(o) == HELP_KIND => false,
        Some(o) => !pinned || o == kind,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(center: i32, top: i32) -> Placement {
        Placement { kind: "audio".into(), center, top, screen: (0, 0, 1920, 1080), scale: 1.0 }
    }

    #[test]
    fn panneau_centre_sous_le_widget_et_garde_dans_l_ecran() {
        assert_eq!(rect_for(&at(960, 46), (300.0, 200.0)), (810, 46, 300, 200));
        // Widget au bord droit : le panneau reste à 4 px du bord.
        assert_eq!(rect_for(&at(1910, 46), (300.0, 200.0)), (1616, 46, 300, 200));
        assert_eq!(rect_for(&at(5, 46), (300.0, 200.0)).0, 4);
        // Trop haut pour l'écran : borné au bas, moins la marge.
        assert_eq!(rect_for(&at(960, 46), (300.0, 5000.0)).3, 1080 - 4 - 46);
    }

    #[test]
    fn panneau_sous_la_bande_d_une_barre_haute() {
        // Barre normale (40 px) : sous la barre, comme sans `y`.
        assert_eq!(panel_top(0, 40, None, 1.0), 46);
        assert_eq!(panel_top(0, 40, Some(40.0), 1.0), 46);
        // Barre haute de 480 px : sous la bande pour un widget, sous tout pour l'aide.
        assert_eq!(panel_top(-853, 480, Some(40.0), 1.0), -853 + 46);
        assert_eq!(panel_top(-853, 480, None, 1.0), -853 + 486);
        // Échelle 150 % ; `y` hors de la fenêtre borné à la barre.
        assert_eq!(panel_top(0, 720, Some(40.0), 1.5), 69);
        assert_eq!(panel_top(0, 60, Some(900.0), 1.5), 69);
    }

    #[test]
    fn taille_logique_mise_a_l_echelle() {
        let p = Placement { scale: 1.5, ..at(960, 60) };
        assert_eq!(rect_for(&p, (300.0, 200.0)), (735, 60, 450, 300));
    }

    #[test]
    fn passage_d_un_widget_a_l_autre() {
        // Panneau de survol ouvert : gardé quand la souris entre sur un autre widget.
        assert!(keeps(Some("audio"), "system", false));
        assert!(keeps(Some("weather:w1"), "weather:w1", false));
        // Rien d'ouvert, ou l'aide du mode édition : délai normal.
        assert!(!keeps(None, "system", false));
        assert!(!keeps(Some("edithelp:m1"), "system", false));
        // Panneau épinglé : seul son widget le garde.
        assert!(!keeps(Some("timer"), "system", true));
        assert!(keeps(Some("timer"), "timer", true));
    }

    #[test]
    fn genre_et_exemplaire() {
        assert_eq!((base_of("weather:w1"), instance_of("weather:w1")), ("weather", "w1"));
        assert_eq!((base_of("media"), instance_of("media")), ("media", ""));
        assert_eq!(instance_of("edithelp:a:b"), "a:b", "tout ce qui suit le premier « : »");
        let s = PopupState::default();
        s.set(Some("servers:w3"), None);
        assert!(s.is_open("servers:w3") && s.is_open_kind("servers") && !s.is_open_kind("server"));
        s.set(None, None);
        assert!(!s.is_open_kind("servers"));
    }

    #[test]
    fn la_sortie_couvre_le_fondu() {
        // Theme.dur-fast = 120 ms : la fenêtre ne disparaît pas avant la fin du fondu.
        assert!(CLOSE_ANIM >= Duration::from_millis(120));
    }
}
