//! Barre trop chargée pour son écran (règle de `app/src/lib/overflow.ts`) : la zone gauche ou
//! droite déborde sous le milieu. D'abord resserrer (`compact` : titres raccourcis, détails de
//! barre large retirés, par chaque widget), puis limiter la largeur des widgets qui grandissent
//! avec leur nombre d'éléments (`limits` : raccourcis, suggestions, sessions Claude, qui montrent
//! alors « +N » et le reste au survol). En dernier recours seulement, masquer les widgets
//! secondaires, du moins utile au plus utile (`squeeze` de leur `widget.json`), dans la zone qui
//! déborde. Un widget sans `squeeze` n'est jamais masqué. En mode édition, rien n'est masqué.
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

/// Issue : barre resserrée, largeur maximale des widgets qui grandissent (identifiant, px
/// logiques) et widgets masqués (zone, genre).
#[derive(Debug, Default, PartialEq)]
pub struct Fit {
    pub compact: bool,
    pub limits: Vec<(String, f32)>,
    pub hidden: Vec<(Zone, &'static str)>,
}

/// Widget qui grandit avec son nombre d'éléments : sa zone, sa largeur complète et la plus petite
/// largeur qu'on peut lui laisser (la pastille « +N »).
#[derive(Debug, Clone, PartialEq)]
pub struct Grow {
    pub zone: Zone,
    pub id: String,
    pub full: f32,
    pub min: f32,
}

/// Limites qui font tenir chaque zone : son excédent est retiré à ses widgets extensibles, en
/// proportion de ce que chacun peut céder. Un côté qui déborde parce que le centre prend trop de
/// place (le centre garde sa largeur, les côtés se partagent le reste) fait aussi réduire le
/// centre : chaque pixel qui lui est retiré rend un demi-pixel à chaque côté. `need` : largeurs
/// voulues avec les widgets extensibles entiers. Renvoie les limites posées (dans l'ordre de
/// `grows`) et les zones qui débordent encore.
pub fn limits(inner: f32, need: [f32; 3], grows: &[Grow]) -> (Vec<(String, f32)>, [bool; 3]) {
    // Largeur laissée à chaque widget extensible.
    let mut width: Vec<f32> = grows.iter().map(|g| g.full).collect();
    let mut need = need;
    // Retire jusqu'à `excess` aux widgets de `zone`, au prorata de ce qu'il leur reste à céder ;
    // renvoie ce qui a été retiré.
    let cut = |zone: Zone, excess: f32, width: &mut Vec<f32>| -> f32 {
        let mine: Vec<usize> = (0..grows.len()).filter(|&i| grows[i].zone == zone).collect();
        let caps: Vec<f32> = mine.iter().map(|&i| (width[i] - grows[i].min).max(0.0)).collect();
        let total: f32 = caps.iter().sum();
        if excess <= 0.5 || total <= 0.0 {
            return 0.0;
        }
        let taken = excess.min(total);
        let before: f32 = mine.iter().map(|&i| width[i]).sum();
        for (&i, c) in mine.iter().zip(&caps) {
            width[i] = (width[i] - taken * c / total).floor().max(grows[i].min);
        }
        before - mine.iter().map(|&i| width[i]).sum::<f32>()
    };
    let side = |need: &[f32; 3]| {
        let share = (inner - need[1]) / 2.0;
        (need[0] - share).max(need[2] - share)
    };
    need[1] -= cut(Zone::Center, need[1] - inner, &mut width);
    for _ in 0..2 {
        let share = (inner - need[1]) / 2.0;
        need[0] -= cut(Zone::Left, need[0] - share, &mut width);
        need[2] -= cut(Zone::Right, need[2] - share, &mut width);
        // Côté encore trop plein : le centre cède le double de ce qui manque.
        let short = side(&need);
        if short <= 0.5 {
            break;
        }
        need[1] -= cut(Zone::Center, 2.0 * short, &mut width);
    }
    let share = (inner - need[1]) / 2.0;
    let still = [need[0] > share + 0.5, need[1] > inner + 0.5, need[2] > share + 0.5];
    let out = grows.iter().zip(&width).filter(|(g, w)| **w < g.full).map(|(g, w)| (g.id.clone(), *w)).collect();
    (out, still)
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

    /// Note la largeur complète d'un widget extensible (tous ses éléments, sans limite).
    pub fn insert_full(&mut self, compact: bool, id: &str, w: f32) -> bool {
        self.insert(compact, &format!("{FULL}{id}"), w)
    }

    /// Largeur complète d'un widget extensible, sinon sa largeur montrée.
    pub fn full(&self, compact: bool, id: &str) -> Option<f32> {
        self.get(compact, &format!("{FULL}{id}")).or_else(|| self.get(compact, id))
    }
}

/// Préfixe des largeurs complètes dans `Widths`.
const FULL: &str = "full:";

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
        return Fit { compact: true, ..Fit::default() };
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
    Fit { compact: true, hidden, ..Fit::default() }
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

    fn grow(zone: Zone, id: &str, full: f32) -> Grow {
        Grow { zone, id: id.into(), full, min: 40.0 }
    }

    #[test]
    fn a_bar_that_fits_gets_no_limit() {
        let (lims, still) = limits(1000.0, [300.0, 200.0, 300.0], &[grow(Zone::Left, "a", 200.0)]);
        assert!(lims.is_empty());
        assert_eq!(still, [false; 3]);
    }

    #[test]
    fn the_excess_is_taken_from_the_growing_widgets_of_the_zone() {
        // Part de la gauche : (1000 - 200) / 2 = 400 ; elle veut 500 : 100 de trop.
        let grows = [grow(Zone::Left, "a", 240.0), grow(Zone::Left, "b", 140.0), grow(Zone::Right, "c", 300.0)];
        let (lims, still) = limits(1000.0, [500.0, 200.0, 300.0], &grows);
        assert_eq!(still, [false; 3]);
        // Capacités 200 et 100 : 2/3 et 1/3 des 100 px.
        assert_eq!(lims, vec![("a".to_owned(), 173.0), ("b".to_owned(), 106.0)]);
        assert!(!lims.iter().any(|(id, _)| id == "c"), "la droite tient : pas de limite");
    }

    #[test]
    fn the_center_is_limited_first_and_frees_room_for_the_sides() {
        // Centre plus large que la bande (1100) : d'abord ramené à 1000, puis encore de 2 × 100
        // pour que les côtés (100 chacun) aient leur part.
        let grows = [grow(Zone::Center, "m", 900.0)];
        let (lims, still) = limits(1000.0, [100.0, 1100.0, 100.0], &grows);
        assert_eq!(lims, vec![("m".to_owned(), 600.0)]);
        assert_eq!(still, [false; 3]);
    }

    #[test]
    fn a_wide_center_gives_room_to_a_full_side() {
        // Centre de 600 (extensible) : part des côtés (1000 - 600) / 2 = 200 ; la droite veut
        // 300 sans rien d'extensible : le centre cède 2 × 100.
        let (lims, still) = limits(1000.0, [100.0, 600.0, 300.0], &[grow(Zone::Center, "m", 600.0)]);
        assert_eq!(lims, vec![("m".to_owned(), 400.0)]);
        assert_eq!(still, [false; 3]);
    }

    #[test]
    fn not_enough_to_take_leaves_the_zone_overflowing() {
        let (lims, still) = limits(1000.0, [700.0, 200.0, 0.0], &[grow(Zone::Left, "a", 140.0)]);
        assert_eq!(lims, vec![("a".to_owned(), 40.0)], "réduit à la pastille");
        assert_eq!(still, [true, false, false]);
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
        assert_eq!(bar.fit(false), Fit { compact: true, hidden: vec![(Zone::Left, "weather")], ..Fit::default() });
        // Resserrer suffit : rien de masqué.
        let bar = Bar { gain: 70.0, ..bar };
        assert_eq!(bar.fit(false), Fit { compact: true, ..Fit::default() });
    }

    #[test]
    fn widgets_without_rank_are_never_hidden_and_editing_hides_nothing() {
        let bar = Bar { zones: [vec![("media", 900.0)], vec![], vec![("clock", 50.0)]], gain: 0.0 };
        assert_eq!(bar.fit(false), Fit { compact: true, ..Fit::default() });
        let bar = Bar { zones: [vec![("media", 400.0), ("system", 200.0)], vec![], vec![]], gain: 0.0 };
        assert_eq!(bar.fit(true), Fit { compact: true, ..Fit::default() }, "mode édition");
        assert_eq!(bar.fit(false).hidden, vec![(Zone::Left, "system")]);
    }
}
