//! Historique d'annulation du mode édition et des fenêtres de réglages.

use super::Layout;

/// Historique d'annulation (20 états) et de rétablissement.
///
/// Une suite de modifications d'un même « groupe » (les frappes dans la fenêtre de réglages
/// d'un widget) ne compte qu'une fois : seul l'état d'avant la première entre dans
/// l'historique. Le groupe se termine à la modification suivante d'un autre groupe ou sans
/// groupe, ou par `end_group` (nouvelle ouverture de la fenêtre de réglages).
#[derive(Debug, Default)]
pub struct UndoStack {
    states: Vec<Layout>,
    redo: Vec<Layout>,
    group: Option<String>,
}

impl UndoStack {
    pub(crate) const DEPTH: usize = 20;

    /// Note l'état d'avant une modification (sauf dans le groupe en cours) ; le rétablissement
    /// n'a plus de sens après une nouvelle modification.
    pub fn push(&mut self, previous: Layout, group: Option<&str>) {
        self.redo.clear();
        if group.is_some() && self.group.as_deref() == group {
            return;
        }
        self.group = group.map(str::to_owned);
        if self.states.len() == Self::DEPTH {
            self.states.remove(0);
        }
        self.states.push(previous);
    }

    /// Termine le groupe en cours : la prochaine modification entre dans l'historique.
    pub fn end_group(&mut self) {
        self.group = None;
    }

    /// État d'avant ; `current` pourra être rétabli.
    pub fn undo(&mut self, current: Layout) -> Option<Layout> {
        let previous = self.states.pop()?;
        self.group = None;
        self.redo.push(current);
        Some(previous)
    }

    /// État annulé en dernier ; `current` pourra de nouveau être annulé.
    pub fn redo(&mut self, current: Layout) -> Option<Layout> {
        let next = self.redo.pop()?;
        self.group = None;
        self.states.push(current);
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::real_monitors;

    fn layout(n: u32) -> Layout {
        let mut l = Layout::default_for(&real_monitors());
        l.version = n;
        l
    }

    #[test]
    fn undo_then_redo() {
        let mut u = UndoStack::default();
        u.push(layout(1), None);
        u.push(layout(2), None);
        assert_eq!(u.undo(layout(3)).unwrap().version, 2);
        assert_eq!(u.undo(layout(2)).unwrap().version, 1);
        assert!(u.undo(layout(1)).is_none());
        assert_eq!(u.redo(layout(1)).unwrap().version, 2);
        assert_eq!(u.redo(layout(2)).unwrap().version, 3);
        assert!(u.redo(layout(3)).is_none());
        // Nouvelle modification : plus rien à rétablir.
        u.undo(layout(3));
        u.push(layout(2), None);
        assert!(u.redo(layout(4)).is_none());
    }

    #[test]
    fn a_group_counts_once() {
        let mut u = UndoStack::default();
        u.push(layout(1), Some("w1"));
        u.push(layout(2), Some("w1"));
        u.push(layout(3), Some("w1"));
        u.push(layout(4), Some("w2"));
        assert_eq!(u.undo(layout(5)).unwrap().version, 4);
        assert_eq!(u.undo(layout(4)).unwrap().version, 1, "le groupe w1 = un seul état");
        u.push(layout(1), Some("w1"));
        u.end_group();
        u.push(layout(2), Some("w1"));
        assert_eq!(u.undo(layout(3)).unwrap().version, 2, "nouvelle session : nouvel état");
    }
}
