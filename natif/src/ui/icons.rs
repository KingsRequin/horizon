//! Icônes Phosphor embarquées (`natif/icons/phosphor/<nom>.svg`, noms de `app/src/lib/icons.ts`).
//! Rendues par Slint à la taille affichée et colorées comme le texte (`Icon`, propriété
//! `colorize`). Pour une icône fixe dans un `.slint`, `@image-url("…/icons/phosphor/<nom>.svg")`
//! (fonction `image-decoders` de `i-slint-core`) ; ici, les icônes choisies à l'exécution
//! (manifeste d'un widget, données).

use std::cell::RefCell;
use std::collections::HashMap;

include!(concat!(env!("OUT_DIR"), "/icons.rs"));

thread_local! {
    static CACHE: RefCell<HashMap<&'static str, slint::Image>> = RefCell::default();
}

/// Nom `'static` de l'icône `name` si elle est embarquée (icônes choisies par les plugins).
pub fn static_name(name: &str) -> Option<&'static str> {
    ICONS.iter().find(|(n, _)| *n == name).map(|(n, _)| *n)
}

/// Icône `name` ; image vide si elle n'est pas embarquée (ou illisible), signalée au journal.
pub fn image(name: &str) -> slint::Image {
    let Some((key, svg)) = ICONS.iter().find(|(n, _)| *n == name) else {
        log::warn!("icône « {name} » absente de icons/phosphor");
        return slint::Image::default();
    };
    CACHE.with_borrow_mut(|c| {
        c.entry(key)
            .or_insert_with(|| {
                slint::Image::load_from_svg_data(svg).unwrap_or_else(|e| {
                    log::warn!("icône « {name} » illisible : {e}");
                    slint::Image::default()
                })
            })
            .clone()
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_valid_svg() {
        assert!(ICONS.len() >= 80);
        for (name, svg) in ICONS {
            let img = slint::Image::load_from_svg_data(svg).unwrap_or_else(|e| panic!("{name} : {e}"));
            assert!(img.size().width > 0, "{name}");
        }
        assert!(ICONS.iter().any(|(n, _)| *n == "clock"));
    }
}
