//! Fenêtres d'essai de la plateforme (`--essai`) : une fenêtre Windows ordinaire et une fenêtre
//! sans cadre qui prend le focus. Voir `ui/essai.slint`.

use super::{EssaiWindow, icons};
use crate::platform::{self, Frame, WindowSpec};
use slint::ComponentHandle;
use crate::undoable::{Removal, Undoable};
use std::cell::RefCell;
use std::time::Instant;

thread_local! {
    static OPEN: RefCell<Vec<EssaiWindow>> = RefCell::default();
    /// Retrait d'essai annulable (bandeau `UndoBar`), commun aux deux fenêtres.
    static UNDO: RefCell<Undoable> = RefCell::default();
    static UNDO_TIMER: slint::Timer = slint::Timer::default();
    static REMOVED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Montre le retrait en cours dans les deux fenêtres.
fn show_undo() {
    let label = UNDO.with_borrow(|u| u.label().unwrap_or_default().to_owned());
    OPEN.with_borrow(|o| o.iter().for_each(|w| w.set_undo_label(label.as_str().into())));
}

/// « Retirer (essai) » : retrait proposé à l'annulation 5 s, journalisé quand il devient
/// définitif ou qu'il est annulé.
fn offer_undo() {
    let n = REMOVED.replace(REMOVED.get() + 1) + 1;
    let removal = Removal {
        label: format!("Élément d'essai {n} retiré"),
        undo: Box::new(move || log::info!("essai : retrait {n} annulé")),
        commit: Some(Box::new(move || log::info!("essai : retrait {n} définitif"))),
    };
    UNDO.with_borrow_mut(|u| u.offer(removal, Instant::now()));
    show_undo();
    if let Some(d) = UNDO.with_borrow(Undoable::deadline) {
        UNDO_TIMER.with(|t| {
            t.start(slint::TimerMode::SingleShot, d.saturating_duration_since(Instant::now()), || {
                UNDO.with_borrow_mut(|u| u.tick(Instant::now()));
                show_undo();
            })
        });
    }
}

pub fn open() -> anyhow::Result<()> {
    let specs = [
        (true, WindowSpec { title: "Horizon — essai".into(), frame: Frame::Normal, size: (520.0, 320.0), ..Default::default() }),
        (
            false,
            WindowSpec {
                title: "Horizon — essai sans cadre".into(),
                frame: Frame::Layered,
                rect: Some((160, 160, 520, 320)),
                topmost: true,
                taskbar: false,
                ..Default::default()
            },
        ),
    ];
    for (opaque, spec) in specs {
        let w = EssaiWindow::new()?;
        w.set_opaque(opaque);
        w.set_icon_click(icons::image("sparkle"));
        w.on_undo_offer(offer_undo);
        w.on_undo_undo(|| {
            UNDO.with_borrow_mut(Undoable::undo);
            show_undo();
        });
        platform::configure(w.window(), spec)?;
        super::accent::follow(&w);
        w.show()?;
        OPEN.with_borrow_mut(|o| o.push(w));
    }
    Ok(())
}
