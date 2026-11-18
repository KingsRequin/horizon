//! Barre trop chargée pour son écran (règle de `app/src/lib/overflow.ts`) : la zone gauche ou
//! droite déborde sous le milieu. D'abord resserrer (`compact` : titres raccourcis, détails de
//! barre large retirés, par chaque widget), puis masquer les widgets secondaires, du moins utile
//! au plus utile (`squeeze` de leur `widget.json`), seulement dans la zone qui déborde. Un widget
//! sans `squeeze` n'est jamais masqué. En mode édition, rien n'est masqué.
//!
//! Logique pure : la mesure est faite par l'appelant (largeurs voulues par l'interface Slint,
//! relues après chaque essai, comme le DOM l'était).

/// Zone de la bande d'une barre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Left,
    Center,
    Right,
}

impl Zone {
    pub const ALL: [Zone; 3] = [Zone::Left, Zone::Center, Zone::Right];
}

/// Issue : barre resserrée, et widgets masqués (zone, genre).
#[derive(Debug, Default, PartialEq)]
pub struct Fit {
    pub compact: bool,
    pub hidden: Vec<(Zone, &'static str)>,
}

/// Largeurs naturelles annoncées par l'interface, gardées par état (normal, resserré) : décider de
/// desserrer d'après des largeurs mesurées resserrées ferait croire que tout tient, et la barre
/// basculerait sans fin d'un état à l'autre. Une largeur jamais mesurée dans un état est prise
/// dans l'autre.
#[derive(Debug, Default, Clone)]
pub struct Widths {
    normal: std::collections::HashMap<String, f32>,
    compact: std::collections::HashMap<String, f32>,
}

impl Widths {
    /// Note la largeur de `id` mesurée dans l'état `compact` ; vrai si elle a changé.
    pub fn insert(&mut self, compact: bool, id: &str, w: f32) -> bool {
        let map = if compact { &mut self.compact } else { &mut self.normal };
        map.insert(id.to_owned(), w) != Some(w)
    }

    /// Largeur de `id` dans l'état `compact`.
    pub fn get(&self, compact: bool, id: &str) -> Option<f32> {
        let (mine, other) = if compact { (&self.compact, &self.normal) } else { (&self.normal, &self.compact) };
        mine.get(id).or_else(|| other.get(id)).copied()
    }
}

/// Zones qui débordent, d'après la largeur intérieure de la bande et la largeur voulue par
/// chaque zone (gauche, centre, droite). Comme en CSS : le centre garde sa largeur, gauche et
/// droite se partagent le reste à parts égales.
pub fn overflowing(inner: f32, need: [f32; 3]) -> [bool; 3] {
    let share = (inner - need[1]) / 2.0;
    [need[0] > share + 0.5, need[1] > inner + 0.5, need[2] > share + 0.5]
}

/// Applique la règle. `measure(compact, hidden)` : zones qui débordent dans cet état ;
/// `present(zone, genre)` : la zone contient ce genre. `order` : genres masquables, du plus
/// secondaire au moins secondaire.
pub fn fit(
    order: &[&'static str],
    editing: bool,
    mut measure: impl FnMut(bool, &[(Zone, &'static str)]) -> [bool; 3],
    present: impl Fn(Zone, &str) -> bool,
) -> Fit {
    if !measure(false, &[]).contains(&true) {
        return Fit::default();
    }
    if editing {
        return Fit { compact: true, hidden: Vec::new() };
    }
    let mut hidden = Vec::new();
    for kind in order {
        let full = measure(true, &hidden);
        if !full.contains(&true) {
            break;
        }
        for (i, z) in Zone::ALL.into_iter().enumerate() {
            if full[i] && present(z, kind) {
                hidden.push((z, *kind));
            }
        }
    }
    Fit { compact: true, hidden }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Barre de 1000 px : chaque widget a sa largeur, resserrer gagne `gain` par zone.
    struct Bar {
        zones: [Vec<(&'static str, f32)>; 3],
        gain: f32,
    }

    impl Bar {
        fn measure(&self, compact: bool, hidden: &[(Zone, &'static str)]) -> [bool; 3] {
            let need = |i: usize| {
                let z = Zone::ALL[i];
                let w: f32 = self.zones[i].iter().filter(|(k, _)| !hidden.contains(&(z, *k))).map(|(_, w)| w).sum();
                if compact { (w - self.gain).max(0.0) } else { w }
            };
            overflowing(1000.0, [need(0), need(1), need(2)])
        }
        fn present(&self, z: Zone, kind: &str) -> bool {
            self.zones[Zone::ALL.iter().position(|x| *x == z).unwrap()].iter().any(|(k, _)| *k == kind)
        }
        fn fit(&self, editing: bool) -> Fit {
            fit(&["suggestions", "weather", "system"], editing, |c, h| self.measure(c, h), |z, k| self.present(z, k))
        }
    }

    #[test]
    fn widths_are_kept_per_state() {
        let mut w = Widths::default();
        assert!(w.insert(false, "media", 300.0));
        assert!(!w.insert(false, "media", 300.0), "même largeur : rien de changé");
        assert_eq!(w.get(true, "media"), Some(300.0), "jamais mesurée resserrée : la normale");
        assert!(w.insert(true, "media", 180.0));
        assert_eq!(w.get(false, "media"), Some(300.0), "la normale ne bouge pas");
        assert_eq!(w.get(true, "media"), Some(180.0));
        assert_eq!(w.get(false, "absent"), None);
    }

    /// La boucle de la barre de l'écran du haut : resserrée, elle tient ; desserrée, elle
    /// déborde. Avec les largeurs par état, le calcul refait sur l'état resserré reste resserré.
    #[test]
    fn a_squeezed_bar_does_not_flip_back() {
        let mut w = Widths::default();
        w.insert(false, "a", 600.0);
        w.insert(true, "a", 400.0);
        let measure = |compact: bool, _: &[(Zone, &'static str)]| overflowing(1000.0, [w.get(compact, "a").unwrap(), 0.0, 0.0]);
        let first = fit(&[], false, measure, |_, _| false);
        assert!(first.compact);
        let again = fit(&[], false, measure, |_, _| false);
        assert_eq!(first, again, "stable");
    }

    #[test]
    fn shares_are_computed_around_the_center() {
        assert_eq!(overflowing(1000.0, [400.0, 200.0, 400.0]), [false, false, false]);
        assert_eq!(overflowing(1000.0, [401.0, 200.0, 100.0]), [true, false, false]);
        assert_eq!(overflowing(1000.0, [0.0, 1001.0, 0.0]), [false, true, false]);
    }

    #[test]
    fn a_bar_that_fits_is_left_alone() {
        let bar = Bar { zones: [vec![("media", 300.0)], vec![("shortcuts", 200.0)], vec![("clock", 100.0)]], gain: 0.0 };
        assert_eq!(bar.fit(false), Fit::default());
    }

    #[test]
    fn compact_first_then_secondary_widgets_of_the_full_zone_only() {
        // Gauche : 460 > 400 ; resserrer ne suffit pas, la météo (2e de l'ordre) suffit.
        let bar = Bar {
            zones: [vec![("media", 300.0), ("weather", 100.0), ("system", 60.0)], vec![("shortcuts", 200.0)], vec![("weather", 50.0)]],
            gain: 20.0,
        };
        assert_eq!(bar.fit(false), Fit { compact: true, hidden: vec![(Zone::Left, "weather")] });
        // Resserrer suffit : rien de masqué.
        let bar = Bar { gain: 70.0, ..bar };
        assert_eq!(bar.fit(false), Fit { compact: true, hidden: vec![] });
    }

    #[test]
    fn widgets_without_rank_are_never_hidden_and_editing_hides_nothing() {
        let bar = Bar { zones: [vec![("media", 900.0)], vec![], vec![("clock", 50.0)]], gain: 0.0 };
        assert_eq!(bar.fit(false), Fit { compact: true, hidden: vec![] });
        let bar = Bar { zones: [vec![("media", 400.0), ("system", 200.0)], vec![], vec![]], gain: 0.0 };
        assert_eq!(bar.fit(true), Fit { compact: true, hidden: vec![] }, "mode édition");
        assert_eq!(bar.fit(false).hidden, vec![(Zone::Left, "system")]);
    }
}
