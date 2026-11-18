//! Retrait annulable des fenêtres de réglages (`app/src/lib/undoable.svelte.ts`) : « « X »
//! retiré · Annuler » pendant 5 s (`ui/undo-bar.slint`). Le retrait est soit déjà fait (`undo`
//! le défait), soit différé (`commit` le fait à la fin du délai, dès qu'un autre retrait arrive
//! ou que la fenêtre se ferme).
//!
//! Logique pure : l'heure est passée par l'appelant, qui relance `tick` à l'échéance
//! (`deadline`, minuteur Slint de la fenêtre).

use std::time::{Duration, Instant};

pub const UNDO_DELAY: Duration = Duration::from_millis(5000);

/// Un retrait proposé à l'annulation.
pub struct Removal {
    /// Texte montré : « Raccourci « YouTube » retiré ».
    pub label: String,
    /// Remet ce qui a été retiré.
    pub undo: Box<dyn FnOnce()>,
    /// Retrait différé : le fait pour de bon.
    pub commit: Option<Box<dyn FnOnce()>>,
}

/// Retrait en cours (un seul à la fois) et son échéance.
pub struct Undoable {
    delay: Duration,
    current: Option<(Removal, Instant)>,
}

impl Default for Undoable {
    fn default() -> Undoable {
        Undoable::new(UNDO_DELAY)
    }
}

impl Undoable {
    pub fn new(delay: Duration) -> Undoable {
        Undoable { delay, current: None }
    }

    /// Texte du retrait en cours (bandeau affiché), s'il y en a un.
    pub fn label(&self) -> Option<&str> {
        self.current.as_ref().map(|(r, _)| r.label.as_str())
    }

    /// Fin du délai du retrait en cours.
    pub fn deadline(&self) -> Option<Instant> {
        self.current.as_ref().map(|(_, d)| *d)
    }

    /// Propose d'annuler ce retrait ; le précédent devient définitif.
    pub fn offer(&mut self, r: Removal, now: Instant) {
        self.settle();
        self.current = Some((r, now + self.delay));
    }

    /// Bouton « Annuler ».
    pub fn undo(&mut self) {
        if let Some((r, _)) = self.current.take() {
            (r.undo)();
        }
    }

    /// Rend définitif le retrait en cours (fin du délai, fermeture de la fenêtre).
    pub fn settle(&mut self) {
        if let Some((r, _)) = self.current.take()
            && let Some(commit) = r.commit
        {
            commit();
        }
    }

    /// Le délai est-il écoulé ? Le retrait devient alors définitif. Vrai si le bandeau change.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.deadline().is_some_and(|d| now >= d) {
            self.settle();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn removal(label: &str, undone: &Rc<Cell<u32>>, committed: Option<&Rc<Cell<u32>>>) -> Removal {
        let u = undone.clone();
        Removal {
            label: label.into(),
            undo: Box::new(move || u.set(u.get() + 1)),
            commit: committed.map(|c| {
                let c = c.clone();
                Box::new(move || c.set(c.get() + 1)) as Box<dyn FnOnce()>
            }),
        }
    }

    #[test]
    fn annuler_defait_le_retrait_sans_le_rendre_definitif() {
        let (undone, committed) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let mut u = Undoable::new(UNDO_DELAY);
        u.offer(removal("a", &undone, Some(&committed)), Instant::now());
        assert_eq!(u.label(), Some("a"));
        u.undo();
        assert_eq!((undone.get(), committed.get()), (1, 0));
        assert!(u.label().is_none());
    }

    #[test]
    fn le_delai_ecoule_rend_le_retrait_definitif() {
        let (undone, committed) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let mut u = Undoable::new(Duration::from_millis(5000));
        let t0 = Instant::now();
        u.offer(removal("a", &undone, Some(&committed)), t0);
        assert!(!u.tick(t0 + Duration::from_millis(4999)));
        assert_eq!(committed.get(), 0);
        assert!(u.tick(t0 + Duration::from_millis(5000)));
        assert_eq!((undone.get(), committed.get()), (0, 1));
        assert!(u.label().is_none() && u.deadline().is_none());
    }

    #[test]
    fn un_nouveau_retrait_rend_le_precedent_definitif() {
        let (undone, first) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
        let mut u = Undoable::default();
        u.offer(removal("a", &undone, Some(&first)), Instant::now());
        u.offer(removal("b", &undone, None), Instant::now());
        assert_eq!(first.get(), 1);
        assert_eq!(u.label(), Some("b"));
        // Retrait déjà fait (sans `commit`) : le rendre définitif ne fait rien de plus.
        u.settle();
        assert_eq!(undone.get(), 0);
    }
}
