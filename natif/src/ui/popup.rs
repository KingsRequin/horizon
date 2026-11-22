//! Fenêtre surgissante (`ui/popup.slint`) et son déroulé, sur le fil de l'interface : affichage
//! sous le widget, fermeture différée avec fondu de sortie annulable, passage d'un widget à
//! l'autre, épinglage avec focus, Échap et clic ailleurs. Règles et placement : `crate::popup`
//! (repris de `app/src-tauri/src/popup.rs`, où les mêmes étapes passaient par un fil des
//! fermetures et des événements vers la page).
//!
//! Les panneaux eux-mêmes sont reliés par leurs widgets (`Backend::panel`) et prévenus de leur
//! ouverture et de leur fermeture par `ui::panel_hosts` (qui tient aussi les tuiles des barres
//! hautes).

use super::PanelWindow;
use crate::host::{AppHandle, Manager};
use crate::platform::{self, Frame, WindowSpec};
use crate::popup::{self as rules, CLOSE_ANIM, HIDE_DELAY, LABEL, Placement, PopupState};
use crate::util::lock;
use horizon_core::layout::BAND_HEIGHT;
use slint::ComponentHandle;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

/// Durée de l'état « apparition » : la première image est présentée transparente, puis le fondu
/// d'entrée joue (comme le retour d'une barre, `ui::bar::set_visible`).
const ENTER: Duration = Duration::from_millis(30);
/// Taille de la fenêtre pour un panneau vide (marges 14 × 16 et bord), pixels logiques.
const EMPTY: (f32, f32) = (34.5, 30.5);

struct Popup {
    app: AppHandle,
    window: PanelWindow,
    /// Panneau ouvert (« media », « weather:w1 ») ; `None` : fermé (fenêtre cachée).
    kind: Option<String>,
    pinned: bool,
    /// La souris est dans le panneau.
    hovered: bool,
    /// Numéro de la dernière demande (affichage, maintien, fermeture) : une fermeture différée
    /// n'aboutit que si rien n'a été demandé depuis.
    generation: u64,
    /// Numéro de la fermeture demandée sans condition (`close`), tant qu'elle est la dernière
    /// demande : la souris sur le panneau ne la retient pas.
    forced: Option<u64>,
    placement: Option<Placement>,
    /// Dernière taille mesurée de chaque panneau (pixels logiques).
    sizes: HashMap<String, (f32, f32)>,
    visible: bool,
    /// Panneau demandé dont la taille n'est pas encore connue (première ouverture) : affiché à
    /// sa mesure.
    awaiting: bool,
    /// Le panneau a changé dans la fenêtre affichée : fondu du nouveau à son affichage.
    switched: bool,
    /// Un menu contextuel est ouvert depuis le panneau : il reste affiché (`hold_for_menu`).
    held: bool,
}

impl Popup {
    /// Une fermeture sans condition est la dernière demande.
    fn closing_anyway(&self) -> bool {
        self.forced == Some(self.generation)
    }
}

thread_local! {
    static POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
}

/// Accès à l'état (fil de l'interface). Ne jamais appeler Slint ou Windows d'une façon qui
/// rappellerait ce module pendant l'emprunt (les rappels de la fenêtre passent après).
fn with<R>(f: impl FnOnce(&mut Popup) -> R) -> Option<R> {
    POPUP.with_borrow_mut(|p| p.as_mut().map(f))
}

/// Crée la fenêtre (cachée) et relie les panneaux des widgets.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let window = PanelWindow::new()?;
    let spec = WindowSpec {
        title: "Horizon".into(),
        frame: Frame::Layered,
        rect: Some((0, 0, 1, 1)),
        focusable: false,
        topmost: true,
        taskbar: false,
        ..Default::default()
    };
    let hwnd = platform::configure(window.window(), spec)?;
    horizon_sys::appbar::remove_frame(hwnd);
    app.register_window(LABEL, hwnd);
    window.global::<super::Tip>().on_show(move |text| super::tooltip::show(hwnd, &text));
    window.global::<super::Tip>().on_hide(super::tooltip::hide);
    window.on_hover(hover);
    window.on_escape(close);
    window.on_measured(measured);
    let a = app.clone();
    window.on_edit_done(move |monitor| {
        if let Err(e) = crate::edit::edit_end(&a, &monitor) {
            log::warn!("fin du mode édition : {e}");
        }
    });
    // Panneau épinglé : la perte du focus (clic ailleurs) le ferme. Différé : Windows prévient
    // pendant un affichage ou un masquage.
    platform::on_active(window.window(), |active| {
        if !active {
            slint::Timer::single_shot(Duration::ZERO, deactivated);
        }
    });
    super::panel_hosts::add_popup(app, LABEL, &window);
    super::accent::follow(&window);
    POPUP.set(Some(Popup {
        app: app.clone(),
        window,
        kind: None,
        pinned: false,
        hovered: false,
        generation: 0,
        forced: None,
        placement: None,
        sizes: HashMap::new(),
        visible: false,
        awaiting: false,
        switched: false,
        held: false,
    }));
    Ok(())
}

/// Où poser le panneau d'un widget de la barre `label` (abscisse et largeur logiques du widget) :
/// sous la bande du haut, centré sur le widget, dans l'écran de la barre.
fn placement(app: &AppHandle, label: &str, kind: &str, x: f32, w: f32) -> Option<Placement> {
    let (hwnd, monitor) = {
        let bars = app.state::<crate::bars::Bars>();
        let list = lock(&bars.list);
        let b = list.iter().find(|b| b.label == label)?;
        (b.hwnd, b.monitor.clone())
    };
    let mut r = RECT::default();
    // SAFETY : lecture du rectangle d'une fenêtre de barre (vivante tant qu'elle est listée).
    unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
    let scale = monitor.scale;
    let s = monitor.rect;
    Some(Placement {
        kind: kind.to_owned(),
        center: r.left + ((x + w / 2.0) * scale) as i32,
        top: rules::panel_top(r.top, (r.bottom - r.top).max(0) as u32, Some(BAND_HEIGHT as f32), scale),
        screen: (s.x, s.y, s.w, s.h),
        scale,
    })
}

/// Aide du mode édition (`edithelp:<écran>`) sous le milieu de la barre `label` (sous toute la
/// barre, même haute) : elle reste jusqu'à la fin de l'édition (`hide_help`), la souris peut la
/// quitter. Elle remplace le panneau ouvert.
pub fn show_help(label: &str, monitor: &str) {
    let Some(app) = with(|p| p.app.clone()) else { return };
    let (hwnd, screen) = {
        let bars = app.state::<crate::bars::Bars>();
        let list = lock(&bars.list);
        let Some(b) = list.iter().find(|b| b.label == label) else { return };
        (b.hwnd, b.monitor.clone())
    };
    let mut r = RECT::default();
    // SAFETY : lecture du rectangle d'une fenêtre de barre (vivante tant qu'elle est listée).
    if unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.is_err() {
        return;
    }
    let kind = format!("{}:{monitor}", rules::HELP_KIND);
    let placement = Placement {
        kind: kind.clone(),
        center: r.left + (r.right - r.left) / 2,
        top: rules::panel_top(r.top, (r.bottom - r.top).max(0) as u32, None, screen.scale),
        screen: (screen.rect.x, screen.rect.y, screen.rect.w, screen.rect.h),
        scale: screen.scale,
    };
    let graphic = super::bar::graphic(label);
    let Some((window, previous)) = with(|p| {
        p.generation += 1;
        p.pinned = false;
        p.placement = Some(placement.clone());
        (p.window.clone_strong(), p.kind.replace(kind.clone()))
    }) else {
        return;
    };
    app.state::<PopupState>().set(Some(&kind), Some(&placement));
    if previous.is_some_and(|k| k != kind) {
        super::panel_hosts::set_popup(&app, LABEL, None, None);
    }
    window.set_graphic(graphic);
    window.set_kind(rules::HELP_KIND.into());
    window.set_instance(monitor.into());
    window.set_pinned(false);
    window.set_closing(false);
    let Some((known, generation, visible)) = with(|p| {
        p.switched = p.visible;
        let known = p.sizes.get(rules::HELP_KIND).copied();
        p.awaiting = known.is_none();
        (known, p.generation, p.visible)
    }) else {
        return;
    };
    match known {
        Some(size) => display(&window, size),
        None if !visible => {
            window.set_entering(true);
            place(&window, &placement, EMPTY, false);
            if let Err(e) = window.show() {
                log::error!("aide du mode édition : {e}");
            }
        }
        None => {}
    }
    remeasure(generation, 0);
}

/// Fin du mode édition de l'écran `monitor` : son aide se ferme (si c'est elle qui est
/// affichée).
pub fn hide_help(monitor: &str) {
    let kind = format!("{}:{monitor}", rules::HELP_KIND);
    if with(|p| p.kind.as_deref() == Some(kind.as_str())) == Some(true) {
        close();
    }
}

/// Le panneau `kind` est-il gardé quand la souris entre sur son widget ? Si oui (un panneau de
/// survol est ouvert, ou le sien en train de se fermer), sa fermeture est annulée : le widget
/// demande alors son panneau sans attendre (passage sans fermeture ni clignotement).
pub fn keep(kind: &str) -> bool {
    // Widget dont le panneau n'est pas (encore) porté : le voisin se ferme normalement.
    if !crate::widgets::has_panel(rules::base_of(kind)) {
        return false;
    }
    let kept = with(|p| {
        if !rules::keeps(p.kind.as_deref(), kind, p.pinned) {
            return false;
        }
        p.generation += 1;
        p.window.set_closing(false);
        true
    });
    kept.unwrap_or(false)
}

/// Ouvre le panneau `kind` sous le widget (`x`, `w`) de la barre `label` ; épinglé, il prend le
/// focus. Un survol ne remplace pas un panneau épinglé.
pub fn show(label: &str, kind: &str, x: f32, w: f32, pinned: bool) {
    if !crate::widgets::has_panel(rules::base_of(kind)) {
        log::debug!("{label} : pas de panneau {kind}");
        return;
    }
    let Some(app) = with(|p| p.app.clone()) else { return };
    if with(|p| !pinned && p.pinned && p.kind.is_some()) == Some(true) {
        return;
    }
    let Some(placement) = placement(&app, label, kind, x, w) else { return };
    let graphic = super::bar::graphic(label);
    let Some((window, previous)) = with(|p| {
        p.generation += 1;
        p.pinned = pinned;
        p.placement = Some(placement.clone());
        (p.window.clone_strong(), p.kind.replace(kind.to_owned()))
    }) else {
        return;
    };
    app.state::<PopupState>().set(Some(kind), Some(&placement));
    let changed = previous.as_deref() != Some(kind);
    if changed {
        // Données du panneau à jour avant sa mesure (le précédent est caché d'abord).
        super::panel_hosts::set_popup(&app, LABEL, Some(kind), rules::screen(&app));
    }
    window.set_graphic(graphic);
    window.set_kind(rules::base_of(kind).into());
    window.set_instance(rules::instance_of(kind).into());
    window.set_pinned(pinned);
    window.set_closing(false);
    // Taille déjà connue : affiché tout de suite (la mesure suit si le contenu a changé). Sinon
    // (première ouverture) : affiché à sa mesure. Slint ne construit le panneau (`if` de
    // `PanelView`) qu'en dessinant la fenêtre : elle est affichée transparente (fondu d'entrée à
    // son début), sa mesure arrive au tour suivant.
    let Some((known, generation, visible)) = with(|p| {
        p.switched = changed && p.visible;
        let known = p.sizes.get(kind).copied();
        p.awaiting = known.is_none();
        (known, p.generation, p.visible)
    }) else {
        return;
    };
    match known {
        Some(size) => display(&window, size),
        None if !visible => {
            window.set_entering(true);
            place(&window, &placement, EMPTY, pinned);
            if let Err(e) = window.show() {
                log::error!("panneau surgissant : {e}");
            }
        }
        None => {}
    }
    remeasure(generation, 0);
}

/// Relit la taille tant que le panneau demandé (`generation`) attend sa mesure : filet de
/// sécurité si `measured` ne venait pas (la fenêtre est redessinée entre deux lectures).
fn remeasure(generation: u64, tries: u32) {
    const TRIES: u32 = 10;
    let delay = if tries == 0 { Duration::ZERO } else { Duration::from_millis(30) };
    slint::Timer::single_shot(delay, move || {
        let Some(Some(window)) = with(|p| (p.generation == generation && p.kind.is_some()).then(|| p.window.clone_strong())) else {
            return;
        };
        measured(window.get_panel_width(), window.get_panel_height());
        if with(|p| p.awaiting) == Some(true) && tries < TRIES {
            window.window().request_redraw();
            remeasure(generation, tries + 1);
        }
    });
}

/// Place et affiche le panneau demandé à la taille `size` : fondu d'entrée (ou du nouveau
/// panneau), focus s'il est épinglé.
fn display(window: &PanelWindow, size: (f32, f32)) {
    let Some(Some((placement, pinned, shown, switched))) = with(|p| {
        let placement = p.placement.clone()?;
        p.awaiting = false;
        Some((placement, p.pinned, std::mem::replace(&mut p.visible, true), std::mem::take(&mut p.switched)))
    }) else {
        return;
    };
    place(window, &placement, size, pinned);
    if !shown {
        // Déjà affichée transparente si le panneau attendait sa mesure (`show` ne refait rien).
        window.set_entering(true);
        if let Err(e) = window.show() {
            log::error!("panneau surgissant : {e}");
        }
        slint::Timer::single_shot(ENTER, || {
            if let Some(w) = with(|p| p.window.clone_strong()) {
                w.set_entering(false);
            }
        });
    } else if switched {
        // Passage d'un panneau à l'autre : le nouveau apparaît en fondu.
        window.set_switching(true);
        slint::Timer::single_shot(ENTER, || {
            if let Some(w) = with(|p| p.window.clone_strong()) {
                w.set_switching(false);
            }
        });
    }
    if pinned {
        window.invoke_focus_panel();
        if shown {
            platform::activate(window.window());
        }
    }
}

/// Dimensionne et place la fenêtre (taille logique `size`) ; épinglée, elle peut prendre le
/// focus.
fn place(window: &PanelWindow, p: &Placement, size: (f32, f32), focusable: bool) {
    let rect = rules::rect_for(p, size);
    let spec = WindowSpec {
        title: "Horizon".into(),
        frame: Frame::Layered,
        rect: Some(rect),
        focusable,
        topmost: true,
        taskbar: false,
        ..Default::default()
    };
    if let Err(e) = platform::configure(window.window(), spec) {
        log::warn!("placement du panneau : {e:#}");
    }
}

/// Taille du panneau demandé (mesurée par la fenêtre) : gardée pour la prochaine ouverture ; la
/// fenêtre s'affiche si elle l'attendait, sinon elle suit. Un panneau vide (rien que les marges)
/// n'est pas affiché.
fn measured(width: f32, height: f32) {
    if width <= EMPTY.0 && height <= EMPTY.1 {
        return;
    }
    let Some(Some((window, awaiting, placement, pinned))) = with(|p| {
        let kind = p.kind.clone()?;
        // L'aide a la même taille sur toutes les barres d'un même style.
        let key = if rules::base_of(&kind) == rules::HELP_KIND { rules::HELP_KIND.to_owned() } else { kind };
        p.sizes.insert(key, (width, height));
        (p.awaiting || p.visible).then(|| (p.window.clone_strong(), p.awaiting, p.placement.clone(), p.pinned))
    }) else {
        return;
    };
    if awaiting {
        display(&window, (width, height));
    } else if let Some(placement) = placement {
        place(&window, &placement, (width, height), pinned);
    }
}

/// La souris a quitté le widget : le panneau se ferme bientôt, sauf s'il est épinglé, si elle y
/// entre ou s'il est déjà en train de se fermer sans condition.
pub fn hide_soon() {
    if with(|p| p.pinned || p.closing_anyway()) == Some(false) {
        close_after(HIDE_DELAY, false);
    }
}

/// Ferme tout de suite (Échap, action choisie, perte du focus d'un panneau épinglé), avec le
/// fondu de sortie, même si la souris est sur le panneau.
pub fn close() {
    let open = with(|p| {
        p.pinned = false;
        p.kind.is_some()
    });
    if open == Some(true) {
        close_after(Duration::ZERO, true);
    }
}

/// Fermeture numérotée : délai, fondu de sortie, puis masquage, chaque étape abandonnée si une
/// demande plus récente est arrivée, ou la souris dans le panneau quand la fermeture n'est pas
/// `forced`.
fn close_after(delay: Duration, forced: bool) {
    let Some(generation) = with(|p| {
        p.generation += 1;
        p.forced = forced.then_some(p.generation);
        p.generation
    }) else {
        return;
    };
    slint::Timer::single_shot(delay, move || {
        if !wanted(generation) {
            return;
        }
        if let Some(w) = with(|p| p.window.clone_strong()) {
            w.set_closing(true);
        }
        slint::Timer::single_shot(CLOSE_ANIM, move || {
            if wanted(generation) {
                hide_now();
            } else if let Some(w) = with(|p| p.window.clone_strong()) {
                // Souris revenue pendant le fondu : le panneau reste.
                w.set_closing(false);
            }
        });
    });
}

/// La fermeture `generation` tient toujours : rien de demandé depuis, pas de menu contextuel
/// ouvert, souris hors du panneau (sauf fermeture sans condition).
fn wanted(generation: u64) -> bool {
    with(|p| p.generation == generation && (!p.hovered || p.closing_anyway()) && !p.held && p.kind.is_some()) == Some(true)
}

/// Le panneau est-il épinglé (il a le focus) ?
pub fn pinned() -> bool {
    with(|p| p.pinned && p.kind.is_some()) == Some(true)
}

/// Un menu contextuel s'ouvre depuis le panneau : la souris va le quitter pour le menu, il reste
/// affiché jusqu'à [`menu_closed`].
pub fn hold_for_menu() {
    with(|p| {
        p.held = true;
        p.generation += 1;
        p.window.set_closing(false);
    });
}

/// Le menu contextuel est fermé : un panneau de survol reprend sa règle, d'après la place réelle
/// de la souris (elle a pu partir pendant le menu sans que la fenêtre le voie).
pub fn menu_closed() {
    let Some(Some(window)) = with(|p| {
        p.held = false;
        (p.kind.is_some() && !p.pinned).then(|| p.window.clone_strong())
    }) else {
        return;
    };
    let inside = platform::hwnd(window.window()).is_some_and(cursor_inside);
    with(|p| p.hovered = inside);
    if !inside {
        hide_soon();
    }
}

/// La souris est-elle dans le rectangle de la fenêtre `hwnd` ?
fn cursor_inside(hwnd: isize) -> bool {
    let (mut r, mut p) = (RECT::default(), POINT::default());
    // SAFETY : lectures de la position de la souris et du rectangle de notre fenêtre.
    let read = unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r).is_ok() && GetCursorPos(&mut p).is_ok() };
    read && (r.left..r.right).contains(&p.x) && (r.top..r.bottom).contains(&p.y)
}

/// Cache la fenêtre ; le panneau est déchargé (ses minuteurs s'arrêtent avec lui).
fn hide_now() {
    let Some((app, window, kind)) = with(|p| {
        p.awaiting = false;
        p.switched = false;
        p.pinned = false;
        p.hovered = false;
        p.held = false;
        p.forced = None;
        p.visible = false;
        (p.app.clone(), p.window.clone_strong(), p.kind.take())
    }) else {
        return;
    };
    app.state::<PopupState>().set(None, None);
    let _ = window.hide();
    window.set_closing(false);
    window.set_kind("".into());
    window.set_instance("".into());
    if kind.is_some() {
        super::panel_hosts::set_popup(&app, LABEL, None, None);
    }
}

/// La souris entre dans le panneau ou en sort. L'aide du mode édition reste jusqu'à la fin de
/// l'édition.
fn hover(on: bool) {
    if with(|p| p.kind.as_deref().is_some_and(|k| rules::base_of(k) == rules::HELP_KIND)) == Some(true) {
        return;
    }
    let open = with(|p| {
        p.hovered = on;
        if on && !p.closing_anyway() {
            // Retour pendant le fondu de sortie : le panneau reste (la fermeture le constate).
            p.window.set_closing(false);
        }
        p.kind.is_some()
    });
    if !on && open == Some(true) {
        hide_soon();
    }
}

/// Fenêtre désactivée : un panneau épinglé se ferme (clic ailleurs, autre fenêtre), sauf le
/// temps de son menu contextuel.
fn deactivated() {
    if with(|p| p.pinned && !p.held && p.kind.is_some()) == Some(true) {
        close();
    }
}

/// Arrêt d'Horizon : la fenêtre est cachée (et lâchée par Slint).
pub fn stop() {
    if let Some(p) = POPUP.take() {
        let _ = p.window.hide();
    }
}
