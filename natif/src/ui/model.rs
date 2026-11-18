//! Modèles Slint mis à jour ligne par ligne : une ligne inchangée ne coûte rien, une ligne
//! modifiée garde son élément (ses animations en cours, son survol), seules les lignes en trop
//! ou en moins sont ajoutées ou retirées. Brique commune des widgets (phase B).

use slint::{Model, ModelRc, VecModel};
use std::rc::Rc;

/// Remplace le contenu de `model` par `rows`, ligne par ligne.
pub fn sync<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let old = model.row_count();
    for (i, row) in rows.iter().enumerate() {
        if i < old {
            if model.row_data(i).as_ref() != Some(row) {
                model.set_row_data(i, row.clone());
            }
        } else {
            model.push(row.clone());
        }
    }
    for i in (rows.len()..old).rev() {
        model.remove(i);
    }
}

/// Comme [`sync`], pour un modèle lu dans une propriété Slint (`current`) : mis à jour ligne par
/// ligne si c'est déjà un `VecModel` posé par Rust, sinon un nouveau est posé par `set`.
pub fn sync_rc<T: Clone + PartialEq + 'static>(current: &ModelRc<T>, rows: Vec<T>, set: impl FnOnce(ModelRc<T>)) {
    match current.as_any().downcast_ref::<VecModel<T>>() {
        Some(m) => sync(m, rows),
        None => set(ModelRc::new(VecModel::from(rows))),
    }
}

/// Modèle vide à poser une fois sur une propriété Slint, gardé pour les mises à jour (`sync`).
pub fn new<T: Clone + 'static>() -> (Rc<VecModel<T>>, ModelRc<T>) {
    let m = Rc::new(VecModel::default());
    (m.clone(), ModelRc::from(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_updates_in_place_and_resizes() {
        let (m, _) = new::<u32>();
        sync(&m, vec![1, 2, 3]);
        assert_eq!(m.iter().collect::<Vec<_>>(), [1, 2, 3]);
        sync(&m, vec![1, 5]);
        assert_eq!(m.iter().collect::<Vec<_>>(), [1, 5]);
        sync(&m, vec![1, 5, 6, 7]);
        assert_eq!(m.iter().collect::<Vec<_>>(), [1, 5, 6, 7]);
        sync(&m, Vec::new());
        assert_eq!(m.row_count(), 0);
    }
}
