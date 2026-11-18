//! Glisser-déposer du mode édition : cible d'un dépôt dans une barre (zones de la bande du haut,
//! une rangée où l'abscisse suffit, et zone des tuiles d'une barre haute, en colonnes), repère
//! d'insertion et décalages « FLIP ». Logique pure reprise de `app/src/lib/drag.ts` et de
//! `markerAt` (`app/src/lib/barDrag.ts`) ; les rectangles sont relevés par l'appelant
//! (`ui::edit`), en pixels logiques de la fenêtre de la barre.

use horizon_core::layout::Zone;
use std::collections::HashMap;

/// Rectangle (pixels logiques).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Bounds {
        Bounds { left, top, right, bottom }
    }

    fn center_x(&self) -> f32 {
        (self.left + self.right) / 2.0
    }

    fn center_y(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }

    /// Distance d'un point au rectangle (0 dedans) : somme des écarts horizontal et vertical.
    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = if x < self.left { self.left - x } else if x > self.right { x - self.right } else { 0.0 };
        let dy = if y < self.top { self.top - y } else if y > self.bottom { y - self.bottom } else { 0.0 };
        dx + dy
    }
}

/// Zone d'une barre, ses widgets (hors widget déplacé, dans l'ordre) et, pour la bande, le
/// rectangle de la bande qui la contient (le repère suit la bande, pas la zone).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneBox {
    pub zone: Zone,
    pub bounds: Bounds,
    pub items: Vec<Bounds>,
    /// Zone en colonnes (tuiles) : le rang dépend de la colonne visée, puis de l'ordonnée.
    pub columns: bool,
    /// Bande du haut qui contient la zone (repère vertical) ; `bounds` si absente.
    pub band: Option<Bounds>,
}

/// Où déposer : zone et rang d'insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropTarget {
    pub zone: Zone,
    pub index: usize,
}

/// Colonnes d'une zone en colonnes (éléments dans l'ordre, de haut en bas puis de colonne en
/// colonne) : suites d'éléments au même bord gauche (à 2 px près).
pub fn columns_of(items: &[Bounds]) -> Vec<Vec<Bounds>> {
    let mut cols: Vec<Vec<Bounds>> = Vec::new();
    for b in items {
        match cols.last_mut() {
            Some(col) if (col[0].left - b.left).abs() < 2.0 => col.push(*b),
            _ => cols.push(vec![*b]),
        }
    }
    cols
}

/// Rang d'insertion dans une zone en colonnes : la colonne visée est la dernière qui commence à
/// gauche du pointeur (la première sinon) ; dedans, avant le premier élément dont le centre est
/// plus bas.
pub fn column_index(x: f32, y: f32, items: &[Bounds]) -> usize {
    if items.is_empty() {
        return 0;
    }
    let cols = columns_of(items);
    let c = cols.iter().rposition(|col| col[0].left <= x).unwrap_or(0);
    let before: usize = cols[..c].iter().map(Vec::len).sum();
    before + cols[c].iter().filter(|b| b.center_y() < y).count()
}

/// Zone sous le pointeur (sinon la plus proche) et rang d'insertion dans cette zone.
pub fn drop_target(x: f32, y: f32, zones: &[ZoneBox]) -> Option<DropTarget> {
    // Comme `reduce` : la première des zones à égale distance.
    let best = zones.iter().fold(None::<&ZoneBox>, |a, b| match a {
        Some(a) if b.bounds.distance(x, y) >= a.bounds.distance(x, y) => Some(a),
        _ => Some(b),
    })?;
    let index =
        if best.columns { column_index(x, y, &best.items) } else { best.items.iter().filter(|b| b.center_x() < x).count() };
    Some(DropTarget { zone: best.zone, index })
}

/// Applique un déplacement à des listes locales (zones d'une barre, dans l'ordre de
/// `Zone::ALL`) ; l'index est borné à la fin de la zone. Un identifiant absent ne change rien.
pub fn move_local<T>(zones: &mut [Vec<T>; 4], id_of: impl Fn(&T) -> &str, id: &str, to: DropTarget) {
    let mut moved = None;
    for z in zones.iter_mut() {
        if let Some(i) = z.iter().position(|w| id_of(w) == id) {
            moved = Some(z.remove(i));
        }
    }
    if let Some(w) = moved {
        let target = &mut zones[zone_rank(to.zone)];
        let i = to.index.min(target.len());
        target.insert(i, w);
    }
}

/// Rang d'une zone dans `Zone::ALL`.
pub fn zone_rank(z: Zone) -> usize {
    Zone::ALL.iter().position(|x| *x == z).unwrap_or(0)
}

/// Repère d'insertion (pixels logiques de la barre).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    pub x: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// Épaisseur du trait.
const LINE: f32 = 2.0;

/// Repère d'une cible : trait vertical entre deux widgets de la bande (6 px du haut et du bas
/// de la bande), ou trait horizontal au-dessus de la tuile visée (sous la dernière tuile de sa
/// colonne quand on dépose à la fin).
pub fn marker_at(zone: &ZoneBox, t: DropTarget) -> Marker {
    let z = zone.bounds;
    let slots = &zone.items;
    if zone.columns {
        // Moitié de l'écart entre deux tuiles (10 px).
        let half = 5.0;
        let across = |b: Bounds, y: f32| Marker { x: b.left, top: y - LINE / 2.0, width: b.right - b.left, height: LINE };
        return match (slots.last(), slots.get(t.index)) {
            (None, _) => across(Bounds { left: z.left + 8.0, right: (z.right - 8.0).min(z.left + 300.0), ..z }, z.top + 8.0),
            (Some(last), None) => across(*last, last.bottom + half),
            (_, Some(next)) => across(*next, next.top - half),
        };
    }
    // Dans la bande, le trait suit la bande ; moitié de l'écart entre deux widgets : 3 px.
    let band = zone.band.unwrap_or(z);
    let at = |x: f32| Marker { x: x - LINE / 2.0, top: band.top + 6.0, width: LINE, height: band.bottom - band.top - 12.0 };
    match (slots.last(), slots.get(t.index)) {
        (None, _) => at(z.center_x()),
        (Some(last), None) => at(last.right + 3.0),
        (_, Some(next)) => at(next.left - 3.0),
    }
}

/// Décalage d'un widget entre son ancienne et sa nouvelle position.
#[derive(Debug, Clone, PartialEq)]
pub struct FlipDelta {
    pub id: String,
    pub dx: f32,
    pub dy: f32,
}

/// FLIP : décalage de chaque widget entre son ancienne et sa nouvelle position (widgets
/// présents des deux côtés, déplacés d'au moins `min` px), trié par identifiant.
pub fn flip_deltas(before: &HashMap<String, (f32, f32)>, after: &HashMap<String, (f32, f32)>, min: f32) -> Vec<FlipDelta> {
    let mut out: Vec<FlipDelta> = after
        .iter()
        .filter_map(|(id, (bl, bt))| {
            let (al, at) = before.get(id)?;
            let (dx, dy) = (al - bl, at - bt);
            (dx.abs() >= min || dy.abs() >= min).then(|| FlipDelta { id: id.clone(), dx, dy })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widget de la bande : 20 px de large autour de son centre.
    fn at(c: f32) -> Bounds {
        Bounds::new(c - 10.0, 6.0, c + 10.0, 34.0)
    }

    fn band(zone: Zone, left: f32, right: f32, centers: &[f32]) -> ZoneBox {
        ZoneBox {
            zone,
            bounds: Bounds::new(left, 0.0, right, 40.0),
            items: centers.iter().map(|c| at(*c)).collect(),
            columns: false,
            band: None,
        }
    }

    fn zones() -> Vec<ZoneBox> {
        vec![
            band(Zone::Left, 0.0, 400.0, &[50.0, 150.0]),
            band(Zone::Center, 800.0, 1000.0, &[900.0]),
            band(Zone::Right, 1400.0, 1920.0, &[1500.0, 1700.0, 1850.0]),
        ]
    }

    fn t(zone: Zone, index: usize) -> Option<DropTarget> {
        Some(DropTarget { zone, index })
    }

    #[test]
    fn insere_avant_le_premier_widget_dont_le_centre_est_a_droite() {
        let z = zones();
        assert_eq!(drop_target(100.0, 20.0, &z), t(Zone::Left, 1));
        assert_eq!(drop_target(10.0, 20.0, &z), t(Zone::Left, 0));
        assert_eq!(drop_target(1900.0, 20.0, &z), t(Zone::Right, 3));
    }

    #[test]
    fn zone_la_plus_proche_hors_de_toute_zone() {
        let z = zones();
        assert_eq!(drop_target(500.0, 20.0, &z), t(Zone::Left, 2));
        assert_eq!(drop_target(1100.0, 20.0, &z), t(Zone::Center, 1));
        assert_eq!(drop_target(1300.0, 20.0, &z), t(Zone::Right, 0));
    }

    #[test]
    fn zone_vide_et_aucune_zone() {
        assert_eq!(drop_target(50.0, 20.0, &[band(Zone::Center, 0.0, 100.0, &[])]), t(Zone::Center, 0));
        assert_eq!(drop_target(50.0, 20.0, &[]), None);
    }

    fn tile(left: f32, top: f32, w: f32, h: f32) -> Bounds {
        Bounds::new(left, top, left + w, top + h)
    }

    /// Barre haute de 1080 px de large : bande du haut (0–40) et trois colonnes de tuiles
    /// dessous (la 3e en empile deux).
    fn tall() -> Vec<ZoneBox> {
        let tiles = ZoneBox {
            zone: Zone::Tiles,
            bounds: Bounds::new(0.0, 44.0, 1080.0, 480.0),
            items: vec![
                tile(6.0, 50.0, 350.0, 200.0),
                tile(366.0, 50.0, 350.0, 400.0),
                tile(726.0, 50.0, 350.0, 120.0),
                tile(726.0, 180.0, 350.0, 250.0),
            ],
            columns: true,
            band: None,
        };
        vec![
            band(Zone::Left, 0.0, 400.0, &[50.0, 150.0]),
            band(Zone::Center, 450.0, 600.0, &[520.0]),
            band(Zone::Right, 650.0, 1080.0, &[900.0]),
            tiles,
        ]
    }

    #[test]
    fn sous_la_bande_les_tuiles_colonne_puis_ordonnee() {
        let z = tall();
        assert_eq!(drop_target(100.0, 60.0, &z), t(Zone::Tiles, 0));
        assert_eq!(drop_target(100.0, 300.0, &z), t(Zone::Tiles, 1));
        assert_eq!(drop_target(400.0, 100.0, &z), t(Zone::Tiles, 1));
        assert_eq!(drop_target(400.0, 460.0, &z), t(Zone::Tiles, 2));
        assert_eq!(drop_target(800.0, 60.0, &z), t(Zone::Tiles, 2));
        assert_eq!(drop_target(800.0, 200.0, &z), t(Zone::Tiles, 3));
        // Sous la dernière tuile : à la fin.
        assert_eq!(drop_target(800.0, 470.0, &z), t(Zone::Tiles, 4));
    }

    #[test]
    fn dans_la_bande_d_une_barre_haute_comme_sur_une_barre_normale() {
        let z = tall();
        assert_eq!(drop_target(100.0, 20.0, &z), t(Zone::Left, 1));
        assert_eq!(drop_target(1000.0, 30.0, &z), t(Zone::Right, 1));
    }

    #[test]
    fn rang_dans_une_zone_en_colonnes() {
        assert_eq!(column_index(10.0, 10.0, &[]), 0);
        let cols = [
            Bounds::new(50.0, 50.0, 200.0, 100.0),
            Bounds::new(50.0, 110.0, 200.0, 200.0),
            Bounds::new(210.0, 50.0, 400.0, 90.0),
        ];
        assert_eq!(column_index(0.0, 0.0, &cols), 0);
        assert_eq!(column_index(0.0, 150.0, &cols), 1);
        assert_eq!(column_index(0.0, 300.0, &cols), 2);
        assert_eq!(column_index(500.0, 0.0, &cols), 2);
        assert_eq!(column_index(500.0, 300.0, &cols), 3);
        assert_eq!(columns_of(&cols).iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    }

    type Zones = [Vec<&'static str>; 4];

    #[test]
    fn deplace_entre_zones_et_borne_l_index() {
        let mut z: Zones = [vec!["a", "b"], vec![], vec!["c"], vec![]];
        move_local(&mut z, |s| s, "a", DropTarget { zone: Zone::Right, index: 99 });
        assert_eq!((z[0].clone(), z[2].clone()), (vec!["b"], vec!["c", "a"]));
        move_local(&mut z, |s| s, "a", DropTarget { zone: Zone::Right, index: 0 });
        assert_eq!(z[2], ["a", "c"]);
        move_local(&mut z, |s| s, "absent", DropTarget { zone: Zone::Left, index: 0 });
        assert_eq!(z[0], ["b"]);
    }

    #[test]
    fn passe_de_la_bande_aux_tuiles_et_inversement() {
        let mut z: Zones = [vec!["a"], vec![], vec!["b"], vec!["t"]];
        move_local(&mut z, |s| s, "b", DropTarget { zone: Zone::Tiles, index: 0 });
        assert_eq!(z[3], ["b", "t"]);
        assert!(z[2].is_empty());
        move_local(&mut z, |s| s, "t", DropTarget { zone: Zone::Left, index: 0 });
        assert_eq!(z[0], ["t", "a"]);
        assert_eq!(z[3], ["b"]);
    }

    #[test]
    fn decalages_flip_des_widgets_deplaces_seulement() {
        let m = |v: &[(&str, f32, f32)]| v.iter().map(|(k, l, t)| (k.to_string(), (*l, *t))).collect::<HashMap<_, _>>();
        let before = m(&[("a", 10.0, 0.0), ("b", 100.0, 0.0), ("c", 300.0, 5.0)]);
        let after = m(&[("a", 10.2, 0.0), ("b", 40.0, 0.0), ("c", 1500.0, 5.0), ("d", 0.0, 0.0)]);
        assert_eq!(
            flip_deltas(&before, &after, 0.5),
            [FlipDelta { id: "b".into(), dx: 60.0, dy: 0.0 }, FlipDelta { id: "c".into(), dx: -1200.0, dy: 0.0 }]
        );
        assert!(flip_deltas(&HashMap::new(), &m(&[("a", 1.0, 1.0)]), 0.5).is_empty());
    }

    #[test]
    fn repere_dans_la_bande_et_dans_les_tuiles() {
        let mut left = band(Zone::Left, 0.0, 400.0, &[50.0, 150.0]);
        left.band = Some(Bounds::new(0.0, 0.0, 1920.0, 40.0));
        // Avant le 2e widget (bord gauche 140) : 3 px avant, trait de 2 px centré.
        assert_eq!(marker_at(&left, DropTarget { zone: Zone::Left, index: 1 }), Marker { x: 136.0, top: 6.0, width: 2.0, height: 28.0 });
        // À la fin : 3 px après le dernier (bord droit 160).
        assert_eq!(marker_at(&left, DropTarget { zone: Zone::Left, index: 2 }).x, 162.0);
        // Zone vide : au milieu.
        assert_eq!(marker_at(&band(Zone::Center, 800.0, 1000.0, &[]), DropTarget { zone: Zone::Center, index: 0 }).x, 899.0);
        let z = tall();
        let tiles = &z[3];
        let m = marker_at(tiles, DropTarget { zone: Zone::Tiles, index: 3 });
        assert_eq!(m, Marker { x: 726.0, top: 174.0, width: 350.0, height: 2.0 });
        assert_eq!(marker_at(tiles, DropTarget { zone: Zone::Tiles, index: 4 }).top, 434.0);
        let empty = ZoneBox { items: vec![], ..tiles.clone() };
        assert_eq!(marker_at(&empty, DropTarget { zone: Zone::Tiles, index: 0 }), Marker { x: 8.0, top: 51.0, width: 292.0, height: 2.0 });
    }
}
