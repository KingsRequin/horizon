//! Fenêtre de réglages (`ui/settings-window.slint`), sur le fil de l'interface : une seule
//! instance, fenêtre Windows ordinaire (cadre, barre de titre sombre si Windows l'est,
//! redimensionnable, clavier, Tab, molette), placée au centre de l'écran voulu et à son échelle
//! (`settings_window::target_monitor`, `centered_on`), titre de la page. La rouvrir la ramène au
//! premier plan, sur la nouvelle page (et sur l'écran voulu s'il a changé). Les barres le savent
//! (`EV_SETTINGS_WINDOW`, `is_open`) : leur mode édition ne se termine pas quand elle prend le
//! focus.
//!
//! Pages : « Réglages d'Horizon » (`ui::options`), réglages d'un widget
//! (`ui::widget_settings`), profils (`ui::profiles`). Une page de plus : un élément
//! `visible: root.page == …` dans `ui/settings-window.slint`, sa liaison appelée par `show`, et
//! au départ de la page (`leave_page`) et à la fermeture (`close`) ce qui attend est enregistré.
//! Retraits annulables : `offer_undo` (bandeau `UndoBar`, définitif après 5 s, au changement de
//! page ou à la fermeture). Un rappel d'annulation ne touche pas à cette fenêtre (`window`,
//! `offer_undo`) : il s'exécute pendant qu'elle est tenue.

use super::SettingsWindow;
use crate::events::EV_SETTINGS_WINDOW;
use crate::host::{AppHandle, Emitter};
use crate::platform::{self, Frame, WindowSpec};
use crate::settings_window::{LABEL, SettingsPage, centered_on, target_monitor};
use crate::undoable::{Removal, Undoable};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::time::{Duration, Instant};

struct Open {
    window: SettingsWindow,
    page: SettingsPage,
    /// Écran où la fenêtre a été posée (identifiant).
    monitor: Option<String>,
    undo: Undoable,
    undo_timer: slint::Timer,
}

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

/// La fenêtre de réglages est ouverte.
pub fn is_open() -> bool {
    OPEN.with_borrow(Option::is_some)
}

/// Page affichée, si la fenêtre est ouverte.
pub fn page() -> Option<SettingsPage> {
    OPEN.with_borrow(|o| o.as_ref().map(|o| o.page.clone()))
}

/// La fenêtre ouverte (copie de la poignée), pour une page qui la remplit.
pub fn window() -> Option<SettingsWindow> {
    OPEN.with_borrow(|o| o.as_ref().map(|o| o.window.clone_strong()))
}

/// Ouvre (ou ramène) la fenêtre sur `page`. Fil de l'interface.
pub fn show(app: &AppHandle, page: SettingsPage) {
    if let Err(e) = try_show(app, &page) {
        log::error!("ouverture des réglages ({page:?}) : {e:#}");
    }
}

fn try_show(app: &AppHandle, page: &SettingsPage) -> anyhow::Result<()> {
    let title = page.title(app);
    let monitor = target_monitor(app, page);
    // Autre page (ou autre widget) : ce qu'attend la page d'avant part, son retrait annulable
    // devient définitif.
    let previous = self::page();
    if previous.as_ref().is_some_and(|p| p != page) {
        leave_page(previous.as_ref());
        settle_undo();
    }
    let reused = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        o.page = page.clone();
        o.window.set_page(page_name(page).into());
        o.window.set_window_title(title.as_str().into());
        Some((o.window.clone_strong(), o.monitor.clone()))
    });
    let window = match reused {
        Some((w, at)) => {
            // Déjà ouverte : ramenée, déplacée si l'écran voulu a changé, agrandie si elle est
            // plus petite que la taille prévue pour la page (une page de widget laisse la
            // fenêtre trop étroite pour les « Réglages d'Horizon »).
            let size = w.window().size();
            let scale = w.window().scale_factor();
            let current = (size.width as f32 / scale, size.height as f32 / scale);
            let wanted = grown(current, page.size());
            if let Some(m) = &monitor
                && (at.as_deref() != Some(m.id.as_str()) || wanted != current)
            {
                let rect = centered_on(m, wanted);
                platform::configure(w.window(), spec(&title, rect))?;
                OPEN.with_borrow_mut(|o| {
                    if let Some(o) = o.as_mut() {
                        o.monitor = Some(m.id.clone());
                    }
                });
            }
            w.show()?;
            platform::activate(w.window());
            w
        }
        None => create(app, page, &title, monitor.as_ref())?,
    };
    match page {
        SettingsPage::Options(section) => super::options::show(app, &window, section),
        SettingsPage::Widget(id) => super::widget_settings::show(app, &window, id),
        SettingsPage::Profiles => super::profiles::show(app, &window),
    }
    Ok(())
}

/// Taille logique d'une fenêtre réutilisée : au moins celle prévue pour la page.
fn grown(current: (f32, f32), wanted: (f32, f32)) -> (f32, f32) {
    // Arrondi : la taille physique divisée par l'échelle n'est pas exacte.
    let fit = |c: f32, w: f32| if c + 0.5 < w { w } else { c };
    (fit(current.0, wanted.0), fit(current.1, wanted.1))
}

/// La page `page` n'est plus affichée : ce qui attend est enregistré, ce qui tournait s'arrête.
fn leave_page(page: Option<&SettingsPage>) {
    match page {
        Some(SettingsPage::Options(_)) => super::options::leave(),
        Some(SettingsPage::Widget(_)) => super::widget_settings::leave(),
        Some(SettingsPage::Profiles) => super::profiles::leave(),
        None => {}
    }
}

/// Rend définitif le retrait en cours (bandeau caché). Les rappels s'exécutent hors de la
/// fenêtre tenue.
fn settle_undo() {
    let pending = OPEN.with_borrow_mut(|o| o.as_mut().map(|o| std::mem::take(&mut o.undo)));
    if let Some(mut u) = pending {
        u.settle();
    }
    show_undo();
}

/// Nom de la page pour l'interface (`SettingsWindow.page`).
fn page_name(page: &SettingsPage) -> &'static str {
    match page {
        SettingsPage::Options(_) => "options",
        SettingsPage::Widget(_) => "widget",
        SettingsPage::Profiles => "profiles",
    }
}

fn spec(title: &str, rect: (i32, i32, u32, u32)) -> WindowSpec {
    WindowSpec { title: title.to_owned(), frame: Frame::Normal, rect: Some(rect), focusable: true, topmost: false, taskbar: true, ..Default::default() }
}

fn create(app: &AppHandle, page: &SettingsPage, title: &str, monitor: Option<&horizon_core::geometry::MonitorInfo>) -> anyhow::Result<SettingsWindow> {
    let window = SettingsWindow::new()?;
    window.set_page(page_name(page).into());
    window.set_window_title(title.into());
    let size = page.size();
    let spec = match monitor {
        Some(m) => spec(title, centered_on(m, size)),
        None => WindowSpec { title: title.to_owned(), frame: Frame::Normal, size, ..Default::default() },
    };
    let hwnd = platform::configure(window.window(), spec)?;
    app.register_window(LABEL, hwnd);
    window.global::<super::Tip>().on_show(move |text| super::tooltip::show(hwnd, &text));
    window.global::<super::Tip>().on_hide(super::tooltip::hide);
    window.on_undo_undo(|| {
        OPEN.with_borrow_mut(|o| {
            if let Some(o) = o.as_mut() {
                o.undo.undo();
            }
        });
        show_undo();
    });
    let a = app.clone();
    window.window().on_close_requested(move || {
        let a = a.clone();
        // Hors du traitement de l'événement : la fenêtre est lâchée ensuite.
        slint::Timer::single_shot(Duration::ZERO, move || close(&a));
        slint::CloseRequestResponse::HideWindow
    });
    super::accent::follow(&window);
    super::options::bind(app, &window);
    super::widget_settings::bind(app, &window);
    super::profiles::bind(app, &window);
    window.show()?;
    OPEN.set(Some(Open {
        window: window.clone_strong(),
        page: page.clone(),
        monitor: monitor.map(|m| m.id.clone()),
        undo: Undoable::default(),
        undo_timer: slint::Timer::default(),
    }));
    let _ = app.emit(EV_SETTINGS_WINDOW, true);
    Ok(window)
}

/// Ferme la fenêtre : ce qui attend part (enregistrement, retrait), puis elle est lâchée.
pub fn close(app: &AppHandle) {
    let Some(mut o) = OPEN.take() else { return };
    super::options::closing(app);
    super::widget_settings::closing(app);
    super::profiles::closing(app);
    o.undo.settle();
    o.undo_timer.stop();
    let _ = o.window.hide();
    app.unregister_window(LABEL);
    super::tooltip::hide();
    drop(o);
    let _ = app.emit(EV_SETTINGS_WINDOW, false);
}

/// Propose un retrait à l'annulation (bandeau en bas de la fenêtre) : définitif après le délai
/// ou à la fermeture, annulé par « Annuler ».
pub fn offer_undo(removal: Removal) {
    let deadline = OPEN.with_borrow_mut(|o| {
        let o = o.as_mut()?;
        o.undo.offer(removal, Instant::now());
        o.undo.deadline()
    });
    show_undo();
    let Some(d) = deadline else { return };
    OPEN.with_borrow(|o| {
        if let Some(o) = o.as_ref() {
            o.undo_timer.start(slint::TimerMode::SingleShot, d.saturating_duration_since(Instant::now()), || {
                OPEN.with_borrow_mut(|o| {
                    if let Some(o) = o.as_mut() {
                        o.undo.tick(Instant::now());
                    }
                });
                show_undo();
            });
        }
    });
}

fn show_undo() {
    OPEN.with_borrow(|o| {
        if let Some(o) = o.as_ref() {
            o.window.set_undo_label(o.undo.label().unwrap_or_default().into());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::grown;

    #[test]
    fn fenetre_reutilisee_agrandie_a_la_taille_de_la_page() {
        // Page de widget (575 × 659) → « Réglages d'Horizon » (760 × 640) : seule la largeur grandit.
        assert_eq!(grown((575.0, 659.0), (760.0, 640.0)), (760.0, 659.0));
        // Plus grande que prévu (agrandie par l'utilisateur) : gardée.
        assert_eq!(grown((900.0, 700.0), (760.0, 640.0)), (900.0, 700.0));
        // Écart d'arrondi de l'échelle : pas de redimensionnement.
        assert_eq!(grown((759.7, 640.0), (760.0, 640.0)), (759.7, 640.0));
    }
}
