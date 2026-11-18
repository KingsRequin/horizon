//! Barre haute : rangement des tuiles en colonnes et changements de la disposition (hauteur,
//! tuiles ajoutées ou retirées).
//!
//! Rangement (`.tiles-flow { columns: 330px; column-gap: 10px }` d'App.svelte, multicolonne CSS
//! équilibré) : autant de colonnes d'au moins `COLUMN_MIN` px que la largeur en contient, de même
//! largeur ; les tuiles, dans l'ordre, de haut en bas puis de colonne en colonne, jamais coupées
//! (`break-inside: avoid`), chacune suivie de sa marge (`margin-bottom: 10px`) ; la hauteur des
//! colonnes est la plus petite qui range tout (colonnes équilibrées). Une tuile n'est jamais plus
//! haute que la zone moins sa marge (son panneau défile dedans). Calcul pur : les hauteurs
//! naturelles sont mesurées par l'interface (`ui::bar`), ce module rend des rectangles.
//!
//! Disposition : changements enregistrés (`AppState::update`), puis barres mises à jour
//! (`ui::bar::refresh`).

use crate::host::{AppHandle, Manager};
use crate::state::AppState;
use horizon_core::layout::{BarOptionsPatch, BarRole, Layout, WidgetKind, Zone};

/// Largeur minimale d'une colonne (px logiques).
pub const COLUMN_MIN: f32 = 330.0;
/// Écart entre deux colonnes, et marge sous chaque tuile.
pub const GAP: f32 = 10.0;

/// Élément à ranger : hauteur naturelle, largeur propre (`None` : celle de la colonne), marge
/// dessous.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Item {
    pub height: f32,
    pub width: Option<f32>,
    pub margin: f32,
}

impl Item {
    /// Tuile : à la largeur de sa colonne, marge de 10 px.
    pub fn tile(height: f32) -> Item {
        Item { height, width: None, margin: GAP }
    }
}

/// Place d'un élément dans la zone (px logiques, depuis le coin du contenu).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Résultat du rangement : une place par élément (dans l'ordre), hauteur du contenu (bas de la
/// colonne la plus haute, marges comprises).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Flow {
    pub rects: Vec<Rect>,
    pub content_h: f32,
    pub column_w: f32,
}

/// Nombre de colonnes et leur largeur pour une largeur de zone.
pub fn columns(width: f32) -> (usize, f32) {
    let n = (((width + GAP) / (COLUMN_MIN + GAP)).floor() as usize).max(1);
    let w = ((width - GAP * (n - 1) as f32) / n as f32).max(1.0);
    (n, w)
}

/// Hauteur d'une tuile : sa hauteur naturelle, bornée à la zone moins sa marge.
pub fn bounded(natural: f32, zone_h: f32) -> f32 {
    natural.min((zone_h - GAP).max(0.0)).max(0.0)
}

/// Les éléments tiennent-ils dans `n` colonnes de hauteur `limit` (remplies dans l'ordre) ?
fn fits(spans: &[f32], n: usize, limit: f32) -> bool {
    let mut cols = 1;
    let mut used = 0.0;
    for &s in spans {
        if used > 0.0 && used + s > limit + 0.01 {
            cols += 1;
            used = 0.0;
        }
        if s > limit + 0.01 {
            return false;
        }
        used += s;
    }
    cols <= n
}

/// Range les éléments dans une zone de largeur `width` et de hauteur visible `zone_h` (hauteurs
/// des tuiles déjà bornées par l'appelant : voir [`bounded`]).
pub fn flow(width: f32, items: &[Item]) -> Flow {
    let (n, column_w) = columns(width);
    let spans: Vec<f32> = items.iter().map(|i| i.height + i.margin).collect();
    // Hauteur de colonne : la plus petite des sommes de tuiles consécutives qui range tout
    // (au moins la plus haute tuile).
    let mut candidates: Vec<f32> = Vec::new();
    for a in 0..spans.len() {
        let mut sum = 0.0;
        for s in &spans[a..] {
            sum += s;
            candidates.push(sum);
        }
    }
    candidates.sort_by(f32::total_cmp);
    let limit = candidates.iter().copied().find(|&h| fits(&spans, n, h)).unwrap_or(0.0);
    let mut rects = Vec::with_capacity(items.len());
    let (mut col, mut used, mut content_h) = (0usize, 0.0f32, 0.0f32);
    for (item, &s) in items.iter().zip(&spans) {
        if used > 0.0 && used + s > limit + 0.01 && col + 1 < n {
            col += 1;
            used = 0.0;
        }
        let x = col as f32 * (column_w + GAP);
        rects.push(Rect { x, y: used, w: item.width.unwrap_or(column_w).min(column_w), h: item.height });
        used += s;
        content_h = content_h.max(used);
    }
    Flow { rects, content_h, column_w }
}

/// Hauteur de la barre d'un écran, ramenée entre la bande seule et le maximum par
/// `horizon_core` (`clamp_height`).
pub fn set_height_in(l: &mut Layout, monitor_id: &str, height: u32) -> anyhow::Result<()> {
    l.set_bar_options(monitor_id, BarOptionsPatch { height: Some(height), ..Default::default() })
}

/// Ajoute une tuile du widget `kind` au bout de la zone des tuiles (seulement un widget qui a un
/// panneau, comme le menu « Ajouter une tuile »). Renvoie l'identifiant du nouveau widget.
pub fn add_tile_in(l: &mut Layout, monitor_id: &str, kind: &str) -> anyhow::Result<String> {
    if !crate::widgets::has_panel(kind) {
        anyhow::bail!("le widget « {kind} » n'a pas de panneau à montrer en tuile");
    }
    l.add_widget(monitor_id, Zone::Tiles, WidgetKind::new(kind))
}

/// Retire la tuile `widget_id` (une tuile seulement).
pub fn remove_tile_in(l: &mut Layout, widget_id: &str) -> anyhow::Result<()> {
    let is_tile = l.bars.iter().any(|b| b.tiles.iter().any(|w| w.id == widget_id));
    if !is_tile {
        anyhow::bail!("{widget_id} n'est pas une tuile");
    }
    l.remove_widget(widget_id).map(|_| ())
}

/// Widgets qui peuvent être ajoutés en tuile (ceux qui ont un panneau), dans l'ordre du
/// catalogue.
pub fn tile_choices() -> Vec<&'static str> {
    crate::widgets::MANIFESTS.iter().map(|m| m.id).filter(|id| crate::widgets::has_panel(id)).collect()
}

/// Applique un changement de disposition (annulable comme ceux du mode édition), puis met les
/// barres à jour (`edit::broadcast_layout` : hauteur des fenêtres et place réservée, tuiles).
fn apply<T>(app: &AppHandle, f: impl FnOnce(&mut Layout) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut out = None;
    app.state::<AppState>().mutate(|l| {
        out = Some(f(l)?);
        Ok(())
    })?;
    crate::edit::broadcast_layout(app);
    out.ok_or_else(|| anyhow::anyhow!("changement sans résultat"))
}

/// Hauteur de la barre de l'écran `monitor_id` (40 = normale, sans tuiles ; 240 à 640 au menu,
/// libre de 40 à 900) : fenêtre et place réservée suivent ; revenue à la hauteur normale, la
/// barre garde ses tuiles pour plus tard.
pub fn set_height(app: &AppHandle, monitor_id: &str, height: u32) -> anyhow::Result<()> {
    apply(app, |l| set_height_in(l, monitor_id, height))
}

/// « Ajouter une tuile » : le widget `kind` au bout des tuiles de la barre `monitor_id`.
pub fn add_tile(app: &AppHandle, monitor_id: &str, kind: &str) -> anyhow::Result<String> {
    apply(app, |l| add_tile_in(l, monitor_id, kind))
}

/// Retire la tuile `widget_id`.
pub fn remove_tile(app: &AppHandle, widget_id: &str) -> anyhow::Result<()> {
    apply(app, |l| remove_tile_in(l, widget_id))
}

/// Étapes d'essai (`HORIZON_TILES_ESSAI`, séparées par « ; », une toutes les 4 s après 3 s) :
/// `<rôle>=<hauteur>` (hauteur de la barre de ce rôle : main, top, portrait, side),
/// `<rôle>+<widget>` (tuile ajoutée), `<rôle>-<widget>` (première tuile de ce widget retirée),
/// `edit` / `noedit` (aspect du mode édition sur toutes les barres).
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Height(BarRole, u32),
    Add(BarRole, String),
    Remove(BarRole, String),
    Edit(bool),
}

fn parse_steps(spec: &str) -> Vec<Step> {
    let role = |r: &str| serde_json::from_value::<BarRole>(serde_json::Value::String(r.trim().to_owned())).ok();
    spec.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| match s {
            "edit" => Some(Step::Edit(true)),
            "noedit" => Some(Step::Edit(false)),
            _ => {
                if let Some((r, h)) = s.split_once('=') {
                    Some(Step::Height(role(r)?, h.trim().parse().ok()?))
                } else if let Some((r, k)) = s.split_once('+') {
                    Some(Step::Add(role(r)?, k.trim().to_owned()))
                } else if let Some((r, k)) = s.split_once('-') {
                    Some(Step::Remove(role(r)?, k.trim().to_owned()))
                } else {
                    None
                }
            }
        })
        .collect()
}

fn run_step(app: &AppHandle, step: &Step) -> anyhow::Result<()> {
    let monitor = |role: BarRole| {
        app.state::<AppState>()
            .read(|l| l.bars.iter().find(|b| b.role == role).map(|b| b.monitor_id.clone()))
            .ok_or_else(|| anyhow::anyhow!("aucune barre {role:?}"))
    };
    match step {
        Step::Height(r, h) => set_height(app, &monitor(*r)?, *h),
        Step::Add(r, k) => add_tile(app, &monitor(*r)?, k).map(|_| ()),
        Step::Remove(r, k) => {
            let m = monitor(*r)?;
            let id = app
                .state::<AppState>()
                .read(|l| l.bars.iter().find(|b| b.monitor_id == m).and_then(|b| b.tiles.iter().find(|w| w.kind.as_str() == k)).map(|w| w.id.clone()))
                .ok_or_else(|| anyhow::anyhow!("aucune tuile {k}"))?;
            remove_tile(app, &id)
        }
        Step::Edit(on) => {
            crate::ui::bar::set_editing_all(*on);
            Ok(())
        }
    }
}

/// Lance les étapes d'essai de `HORIZON_TILES_ESSAI` (fil de l'interface).
pub fn essai(app: &AppHandle) -> anyhow::Result<()> {
    let Ok(spec) = std::env::var("HORIZON_TILES_ESSAI") else { return Ok(()) };
    let steps = parse_steps(&spec);
    log::info!("essai des tuiles : {steps:?}");
    for (i, step) in steps.into_iter().enumerate() {
        let a = app.clone();
        slint::Timer::single_shot(std::time::Duration::from_secs(3 + 4 * i as u64), move || {
            log::info!("essai des tuiles : {step:?}");
            if let Err(e) = run_step(&a, &step) {
                log::warn!("essai des tuiles : {e:#}");
            }
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::layout::BAND_HEIGHT;

    #[test]
    fn columns_are_at_least_330_px() {
        // Écran vertical (1080 px, bords graphiques de 4 px) : trois colonnes.
        let (n, w) = columns(1080.0 - 8.0);
        assert_eq!(n, 3);
        assert!((w - (1072.0 - 20.0) / 3.0).abs() < 0.01);
        // 1920 px, bords de 6 px : cinq colonnes ; étroit : une seule, à la largeur de la zone.
        assert_eq!(columns(1908.0).0, 5);
        assert_eq!(columns(200.0), (1, 200.0));
        assert_eq!(columns(669.0).0, 1);
        assert_eq!(columns(670.0).0, 2);
    }

    #[test]
    fn tiles_fill_columns_top_to_bottom_balanced() {
        // Trois colonnes ; cinq tuiles : la colonne la plus haute est la plus basse possible.
        let items: Vec<Item> = [200.0, 400.0, 120.0, 250.0, 100.0].map(Item::tile).to_vec();
        let f = flow(1072.0, &items);
        let col = |r: &Rect| (r.x / (f.column_w + GAP)).round() as usize;
        let cols: Vec<usize> = f.rects.iter().map(col).collect();
        // Ordre gardé (colonne croissante) ; colonnes de 500 px au plus, la plus petite hauteur
        // qui range tout (en dessous, il faudrait une quatrième colonne).
        assert!(cols.is_sorted());
        assert_eq!(cols, [0, 1, 2, 2, 2]);
        assert_eq!(f.rects[3].y, 130.0);
        assert_eq!(f.rects[4].y, 390.0);
        assert_eq!(f.content_h, 500.0);
        assert!(f.rects.iter().all(|r| (r.w - f.column_w).abs() < 0.01));
    }

    #[test]
    fn small_tiles_slide_under_each_other() {
        // Deux colonnes et quatre petites tuiles : deux par colonne, pas une rangée trouée.
        let items: Vec<Item> = [100.0, 100.0, 100.0, 100.0].map(Item::tile).to_vec();
        let f = flow(700.0, &items);
        let ys: Vec<f32> = f.rects.iter().map(|r| r.y).collect();
        assert_eq!(ys, [0.0, 110.0, 0.0, 110.0]);
        assert_eq!(f.content_h, 220.0);
    }

    #[test]
    fn more_columns_than_tiles_and_overflow() {
        let f = flow(1908.0, &[Item::tile(300.0)]);
        assert_eq!((f.rects[0].x, f.rects[0].y, f.content_h), (0.0, 0.0, 310.0));
        // Une seule colonne étroite : tout s'empile, le surplus défile.
        let f = flow(300.0, &[Item::tile(300.0), Item::tile(300.0)]);
        assert_eq!(f.rects[1].y, 310.0);
        assert_eq!(f.content_h, 620.0);
        assert!(flow(500.0, &[]).rects.is_empty());
    }

    #[test]
    fn a_tile_is_never_taller_than_the_zone() {
        assert_eq!(bounded(900.0, 436.0), 426.0);
        assert_eq!(bounded(120.0, 436.0), 120.0);
        assert_eq!(bounded(50.0, 5.0), 0.0);
    }

    #[test]
    fn the_add_button_keeps_its_own_width() {
        let add = Item { height: 96.0, width: Some(140.0), margin: 0.0 };
        let f = flow(700.0, &[Item::tile(100.0), add]);
        assert_eq!((f.rects[1].w, f.rects[1].h), (140.0, 96.0));
    }

    fn layout() -> Layout {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "bars": [{
                "monitor_id": "m", "monitor_pos": [0, 0], "role": "main", "style": "graphic", "height": 480,
                "left": [{ "id": "c", "kind": "clock" }],
                "tiles": [{ "id": "t1", "kind": "media" }],
            }],
        }))
        .unwrap()
    }

    #[test]
    fn essai_steps() {
        assert_eq!(
            parse_steps("main=480; main+media;portrait-system;edit;noedit;x=3;bad"),
            [
                Step::Height(BarRole::Main, 480),
                Step::Add(BarRole::Main, "media".into()),
                Step::Remove(BarRole::Portrait, "system".into()),
                Step::Edit(true),
                Step::Edit(false),
            ]
        );
    }

    #[test]
    fn height_change_keeps_the_tiles() {
        let mut l = layout();
        set_height_in(&mut l, "m", BAND_HEIGHT).unwrap();
        assert_eq!(l.bars[0].height, BAND_HEIGHT);
        assert!(l.bars[0].shown_tiles().is_empty());
        assert_eq!(l.bars[0].tiles.len(), 1, "gardées pour plus tard");
        set_height_in(&mut l, "m", 2000).unwrap();
        assert_eq!(l.bars[0].height, horizon_core::layout::MAX_BAR_HEIGHT);
        assert_eq!(l.bars[0].shown_tiles().len(), 1);
        assert!(set_height_in(&mut l, "inconnu", 480).is_err());
    }

    #[test]
    fn only_widgets_with_a_panel_become_tiles() {
        let mut l = layout();
        let id = add_tile_in(&mut l, "m", "system").unwrap();
        assert_eq!(l.bars[0].tiles.iter().map(|w| w.kind.as_str()).collect::<Vec<_>>(), ["media", "system"]);
        assert!(add_tile_in(&mut l, "m", "batteries").is_err(), "sans panneau");
        assert!(add_tile_in(&mut l, "m", "inconnu").is_err());
        assert!(add_tile_in(&mut l, "autre", "media").is_err(), "écran inconnu");
        assert!(remove_tile_in(&mut l, "c").is_err(), "pas une tuile");
        remove_tile_in(&mut l, &id).unwrap();
        assert_eq!(l.bars[0].tiles.len(), 1);
        assert!(tile_choices().contains(&"media") && !tile_choices().contains(&"batteries"));
    }
}
