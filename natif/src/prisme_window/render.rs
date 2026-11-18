//! Dessin des deux toiles de Prisme : le plan en direct (`Stage.svelte` : écrans et LED qui
//! brillent) et l'éditeur du plan (`PlanEditor.svelte` : écrans, zones, poignées). Slint n'a pas
//! de `<canvas>` : l'image est peinte ici avec tiny-skia (anticrénelage, dégradés radiaux, fusion
//! additive des halos comme `globalCompositeOperation = 'lighter'`) en pixels physiques, puis
//! posée dans un `Image` de la fenêtre ; les textes (noms des écrans, nom de l'élément
//! sélectionné) sont des `Text` Slint posés par-dessus (`Label`). Une image n'est refaite que
//! quand une nouvelle image du moteur arrive (≤ 15 par seconde), que la toile change de taille
//! ou que le brouillon, la sélection ou le survol change.

use super::color;
use super::plan::{self, Handle, Item, ItemRef, Pt, Shape, pt};
use super::types::{Preview, Screen};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{
    BlendMode, Color, FillRule, GradientStop, LineCap, Paint, PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient,
    Rect, SpreadMode, Stroke, Transform,
};

/// Texte posé sur la toile (coordonnées logiques, centre du texte).
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub x: f32,
    pub y: f32,
    pub text: String,
    /// Opacité du texte (#ebe8e3).
    pub alpha: f32,
    /// Étiquette de l'élément sélectionné (gras, sur fond).
    pub pill: bool,
}

/// Une image de la toile.
pub struct Frame {
    pub pixels: SharedPixelBuffer<Rgba8Pixel>,
    pub labels: Vec<Label>,
}

/// Taille de la toile : logique, et échelle de l'écran de la fenêtre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
    pub scale: f32,
}

impl Size {
    fn physical(&self) -> (u32, u32) {
        ((self.w * self.scale).round().max(1.0) as u32, (self.h * self.scale).round().max(1.0) as u32)
    }

    pub fn usable(&self) -> bool {
        self.w >= 2.0 && self.h >= 2.0 && self.scale > 0.0
    }
}

fn rgba(c: [u8; 3], a: f32) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let (w, h) = (w.max(0.0), h.max(0.0));
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    // Quart de cercle approché par une cubique (k = 0,5523).
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Halo d'une LED (dégradé radial, fusion additive).
fn glow(layer: &mut Pixmap, t: Transform, x: f32, y: f32, r: f32, c: [u8; 3], a: f32) {
    let stops = vec![GradientStop::new(0.0, rgba(c, a)), GradientStop::new(1.0, rgba(c, 0.0))];
    let Some(shader) = RadialGradient::new(Point::from_xy(x, y), 0.0, Point::from_xy(x, y), r, stops, SpreadMode::Pad, Transform::identity())
    else {
        return;
    };
    let p = Paint { shader, blend_mode: BlendMode::Plus, anti_alias: true, ..Paint::default() };
    if let Some(rect) = Rect::from_xywh(x - r, y - r, r * 2.0, r * 2.0) {
        layer.fill_rect(rect, &p, t, None);
    }
}

fn circle(layer: &mut Pixmap, t: Transform, x: f32, y: f32, r: f32, fill: Option<Color>, stroke: Option<(Color, f32)>) {
    let Some(path) = PathBuilder::from_circle(x, y, r) else { return };
    if let Some(c) = fill {
        layer.fill_path(&path, &paint(c), FillRule::Winding, t, None);
    }
    if let Some((c, w)) = stroke {
        layer.stroke_path(&path, &paint(c), &Stroke { width: w, ..Stroke::default() }, t, None);
    }
}

/// Fond opaque + calque transparent posé dessus (comme la toile du navigateur sur la page).
fn finish(mut base: Pixmap, layer: &Pixmap) -> SharedPixelBuffer<Rgba8Pixel> {
    base.draw_pixmap(0, 0, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(base.width(), base.height());
    // Fond opaque : prémultiplié = direct.
    buf.make_mut_bytes().copy_from_slice(base.data());
    buf
}

fn canvases(size: Size, bg: [u8; 3]) -> Option<(Pixmap, Pixmap, Transform)> {
    let (pw, ph) = size.physical();
    let mut base = Pixmap::new(pw, ph)?;
    base.fill(rgba(bg, 1.0));
    let layer = Pixmap::new(pw, ph)?;
    Some((base, layer, Transform::from_scale(size.scale, size.scale)))
}

/// Ce que le plan en direct renvoie en plus de son image.
pub struct StageInfo {
    /// Nombre de LED montrées (0 : message « Aucune LED à afficher »).
    pub leds: usize,
    /// Couleur dominante (pondérée par la luminosité), pour l'accent de l'interface.
    pub dominant: Option<[f64; 3]>,
}

/// Le plan en direct (`Stage.svelte`) : écrans, halos et point de chaque LED. `focus` : appareil
/// mis en avant (les autres estompés).
pub fn stage(size: Size, bg: [u8; 3], p: Option<&Preview>, focus: Option<&str>) -> Option<(Frame, StageInfo)> {
    let (base, mut layer, t) = canvases(size, bg)?;
    let (w, h) = (size.w as f64, size.h as f64);
    let n = p.map_or(0, |p| p.positions.len());
    let fallback = [Screen::fallback()];
    let screens: &[Screen] = match p {
        Some(p) if !p.screens.is_empty() => &p.screens,
        _ => &fallback,
    };

    // Cadrage : tous les écrans et toutes les LED, avec une marge pour les halos.
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for s in screens {
        (x0, y0, x1, y1) = (x0.min(s.x), y0.min(s.y), x1.max(s.x + s.w), y1.max(s.y + s.h));
    }
    for q in p.map_or(&[][..], |p| &p.positions) {
        (x0, y0, x1, y1) = (x0.min(q[0]), y0.min(q[1]), x1.max(q[0]), y1.max(q[1]));
    }
    let pad = (x1 - x0).max(y1 - y0) * 0.08;
    (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    let k = (w / (x1 - x0)).min(h / (y1 - y0));
    let ox = (w - (x1 - x0) * k) / 2.0 - x0 * k;
    let oy = (h - (y1 - y0) * k) / 2.0 - y0 * k;
    let px = |x: f64| (ox + x * k) as f32;
    let py = |y: f64| (oy + y * k) as f32;

    let range = focus.and_then(|f| p?.devices.iter().find(|(id, ..)| id == f).map(|(_, a, b)| (*a, *b)));
    let in_focus = |i: usize| focus.is_none() || range.is_some_and(|(a, b)| i >= a && i < b);
    let glow_r = (k * 0.06).max(10.0) as f32;
    let (mut ar, mut ag, mut ab, mut aw) = (0.0, 0.0, 0.0, 0.0);
    if let Some(p) = p {
        for (i, q) in p.positions.iter().enumerate() {
            let c = p.colors.get(i).copied().unwrap_or([0, 0, 0]);
            let alpha = if in_focus(i) { 1.0 } else { 0.18 };
            glow(&mut layer, t, px(q[0]), py(q[1]), glow_r, c, 0.55 * alpha);
            let wgt = c[0] as f64 + c[1] as f64 + c[2] as f64;
            ar += c[0] as f64 * wgt;
            ag += c[1] as f64 * wgt;
            ab += c[2] as f64 * wgt;
            aw += wgt;
        }
    }

    // Les écrans passent par-dessus les halos : la lumière semble venir de derrière.
    let mut labels = Vec::new();
    for s in screens {
        let active = p.and_then(|p| p.active_screen.as_deref()) == Some(s.name.as_str());
        if let Some(path) = round_rect(px(s.x) + 1.5, py(s.y) + 1.5, (s.w * k) as f32 - 3.0, (s.h * k) as f32 - 3.0, 6.0) {
            layer.fill_path(&path, &paint(rgba([0x11, 0x13, 0x18], 1.0)), FillRule::Winding, t, None);
            let (lw, a) = if active { (1.5, 0.32) } else { (1.0, 0.07) };
            layer.stroke_path(&path, &paint(rgba([255, 255, 255], a)), &Stroke { width: lw, ..Stroke::default() }, t, None);
        }
        if !s.name.is_empty() && screens.len() > 1 {
            labels.push(Label {
                x: px(s.x + s.w / 2.0),
                y: py(s.y + s.h / 2.0),
                text: super::screens::label(Some(&s.name), &[]),
                alpha: if active { 0.7 } else { 0.28 },
                pill: false,
            });
        }
    }

    // Point net de chaque LED, pour lire la disposition.
    if let Some(p) = p {
        for (i, q) in p.positions.iter().enumerate() {
            let c = p.colors.get(i).copied().unwrap_or([0, 0, 0]);
            let alpha = if in_focus(i) { 0.9 } else { 0.15 };
            let c = [c[0].max(40), c[1].max(40), c[2].max(40)];
            circle(&mut layer, t, px(q[0]), py(q[1]), 2.2, Some(rgba(c, alpha)), None);
        }
    }
    let dominant = (aw > 0.0).then(|| [ar / aw, ag / aw, ab / aw]);
    Some((Frame { pixels: finish(base, &layer), labels }, StageInfo { leds: n, dominant }))
}

/// Accent lissé vers la couleur dominante des LED (`liveAccent` de Stage.svelte : 15 % par image
/// à ~60 images/s ; ici par image du moteur, ~15/s : même constante de temps), puis éclairci
/// pour rester lisible. Renvoie (accent lissé, couleur affichée).
pub fn live_accent(previous: [f64; 3], dominant: [f64; 3], frames_per_s: f64) -> ([f64; 3], [u8; 3]) {
    let per_frame = 1.0 - 0.85f64.powf(60.0 / frames_per_s.max(1.0));
    let a: [f64; 3] = std::array::from_fn(|i| previous[i] + (dominant[i] - previous[i]) * per_frame);
    let m = a.iter().copied().fold(1.0, f64::max);
    let s = 230.0 / m;
    (a, a.map(|v| (v * s * 0.85 + 30.0).min(255.0).round() as u8))
}

/// Cadrage de l'éditeur du plan : échelle et origine (pixels logiques).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub k: f64,
    pub ox: f64,
    pub oy: f64,
}

impl Default for View {
    fn default() -> Self {
        View { k: 100.0, ox: 0.0, oy: 0.0 }
    }
}

impl View {
    pub fn to_px(self, p: Pt) -> Pt {
        pt(self.ox + p.x * self.k, self.oy + p.y * self.k)
    }

    pub fn to_plan(self, x: f64, y: f64) -> Pt {
        pt((x - self.ox) / self.k, (y - self.oy) / self.k)
    }

    /// Tous les écrans et toutes les poignées, avec une marge (`fitView`).
    pub fn fit(w: f64, h: f64, screens: &[Screen], items: &[Item]) -> View {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut add = |p: Pt| {
            (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
        };
        for s in screens {
            add(pt(s.x, s.y));
            add(pt(s.x + s.w, s.y + s.h));
        }
        for it in items {
            for (_, p) in plan::handles(it) {
                add(p);
            }
            if let Shape::Rect { x, y, .. } = it.shape {
                add(plan::to_plan(&it.screen, pt(x, y)));
            }
        }
        if !x0.is_finite() {
            (x0, y0, x1, y1) = (0.0, 0.0, 1.0, 0.56);
        }
        let pad = (x1 - x0).max(y1 - y0) * 0.1;
        (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
        let k = (w / (x1 - x0)).min(h / (y1 - y0));
        View { k, ox: (w - (x1 - x0) * k) / 2.0 - x0 * k, oy: (h - (y1 - y0) * k) / 2.0 - y0 * k }
    }
}

/// Ce que montre l'éditeur du plan.
pub struct PlanScene<'a> {
    pub screens: &'a [Screen],
    pub items: &'a [Item],
    pub selected: Option<&'a ItemRef>,
    pub selected_screen: Option<&'a str>,
    pub focus_device: Option<&'a str>,
    pub preview: Option<&'a Preview>,
    pub view: View,
}

/// L'éditeur du plan (`draw` de PlanEditor.svelte).
pub fn plan(size: Size, bg: [u8; 3], s: &PlanScene<'_>) -> Option<Frame> {
    let (base, mut layer, t) = canvases(size, bg)?;
    let v = s.view;
    let k = v.k;
    let px = |p: Pt| {
        let q = v.to_px(p);
        (q.x as f32, q.y as f32)
    };
    let text = [0xeb, 0xe8, 0xe3];
    let mut labels = Vec::new();

    // Écrans.
    for sc in s.screens {
        let (ax, ay) = px(pt(sc.x, sc.y));
        let sel = s.selected_screen == Some(sc.name.as_str());
        let active = s.preview.and_then(|p| p.active_screen.as_deref()) == Some(sc.name.as_str());
        if let Some(path) = round_rect(ax + 1.0, ay + 1.0, (sc.w * k) as f32 - 2.0, (sc.h * k) as f32 - 2.0, 6.0) {
            layer.fill_path(&path, &paint(rgba([0x12, 0x14, 0x19], 1.0)), FillRule::Winding, t, None);
            let (lw, a) = if sel {
                (2.0, 0.8)
            } else if active {
                (1.0, 0.3)
            } else {
                (1.0, 0.1)
            };
            layer.stroke_path(&path, &paint(rgba(text, a)), &Stroke { width: lw, ..Stroke::default() }, t, None);
        }
        labels.push(Label {
            x: ax + (sc.w * k / 2.0) as f32,
            y: ay + (sc.h * k / 2.0) as f32,
            text: super::screens::label(Some(&sc.name), &[]),
            alpha: if sel || active { 0.65 } else { 0.3 },
            pill: false,
        });
    }

    // Lumière des LED (aperçu du moteur).
    if let Some(p) = s.preview {
        let r = (k * 0.045).max(8.0) as f32;
        for (i, q) in p.positions.iter().enumerate() {
            let c = p.colors.get(i).copied().unwrap_or([0, 0, 0]);
            let (x, y) = px(pt(q[0], q[1]));
            glow(&mut layer, t, x, y, r, c, 0.45);
        }
    }

    // Éléments.
    for it in s.items {
        let sel = s.selected == Some(&it.r);
        let dim = s.focus_device.is_some_and(|f| it.r.device != f) && !sel;
        let alpha = if dim {
            0.18
        } else if sel {
            0.95
        } else {
            0.55
        };
        let base_c = color::device(it.device_index);
        let col = rgba(base_c, alpha);
        let fill = rgba(
            base_c,
            if dim {
                0.05
            } else if sel {
                0.22
            } else {
                0.12
            },
        );
        let lw = if sel { 2.0 } else { 1.5 };
        let sc = &it.screen;
        match it.shape {
            Shape::Line { from, to } => {
                let (ax, ay) = px(plan::to_plan(sc, pt(from[0], from[1])));
                let (bx, by) = px(plan::to_plan(sc, pt(to[0], to[1])));
                let mut pb = PathBuilder::new();
                pb.move_to(ax, ay);
                pb.line_to(bx, by);
                if let Some(path) = pb.finish() {
                    let thick = Stroke { width: if sel { 9.0 } else { 7.0 }, line_cap: LineCap::Round, ..Stroke::default() };
                    layer.stroke_path(&path, &paint(fill), &thick, t, None);
                    let thin = Stroke { width: lw, line_cap: LineCap::Round, ..Stroke::default() };
                    layer.stroke_path(&path, &paint(col), &thin, t, None);
                }
                // Sens de la chaîne de LED.
                let len = (bx - ax).hypot(by - ay);
                if len > 30.0 {
                    let (ux, uy) = ((bx - ax) / len, (by - ay) / len);
                    let (mx, my) = (ax + (bx - ax) * 0.55, ay + (by - ay) * 0.55);
                    let mut pb = PathBuilder::new();
                    pb.move_to(mx - ux * 6.0 - uy * 5.0, my - uy * 6.0 + ux * 5.0);
                    pb.line_to(mx, my);
                    pb.line_to(mx - ux * 6.0 + uy * 5.0, my - uy * 6.0 - ux * 5.0);
                    if let Some(path) = pb.finish() {
                        let thin = Stroke { width: lw, line_cap: LineCap::Round, ..Stroke::default() };
                        layer.stroke_path(&path, &paint(col), &thin, t, None);
                    }
                }
            }
            Shape::Rect { x, y, w, h } => {
                let (ax, ay) = px(plan::to_plan(sc, pt(x, y)));
                let (bx, by) = px(plan::to_plan(sc, pt(x + w, y + h)));
                if let Some(path) = round_rect(ax.min(bx), ay.min(by), (bx - ax).abs(), (by - ay).abs(), 5.0) {
                    layer.fill_path(&path, &paint(fill), FillRule::Winding, t, None);
                    layer.stroke_path(&path, &paint(col), &Stroke { width: lw, ..Stroke::default() }, t, None);
                }
            }
            Shape::Point { x, y } => {
                let (cx, cy) = px(plan::to_plan(sc, pt(x, y)));
                circle(&mut layer, t, cx, cy, if sel { 9.0 } else { 7.0 }, Some(fill), Some((col, lw)));
            }
        }
    }

    // Poignées et nom de l'élément sélectionné.
    if let Some(sel) = s.items.iter().find(|it| s.selected == Some(&it.r)) {
        for (_, h) in plan::handles(sel) {
            let (x, y) = px(h);
            circle(&mut layer, t, x, y, 6.0, Some(rgba(text, 1.0)), Some((rgba([0x1a, 0x1d, 0x24], 1.0), 2.0)));
        }
        let (cx, cy) = px(plan::center(sel));
        labels.push(Label { x: cx, y: cy - 20.0, text: sel.label.clone(), alpha: 1.0, pill: true });
    }
    Some(Frame { pixels: finish(base, &layer), labels })
}

/// Ce qui est sous le point `q` du plan (`hit` du PlanEditor) : poignée de l'élément sélectionné,
/// élément le plus proche (à 10 px près), sinon écran.
pub enum Hit {
    Handle(Item, Handle),
    Item(Item),
    Screen(Screen),
    Nothing,
}

pub fn hit(q: Pt, view: &View, items: &[Item], screens: &[Screen], selected: Option<&ItemRef>) -> Hit {
    let tol = 10.0 / view.k;
    if let Some(sel) = items.iter().find(|it| selected == Some(&it.r)) {
        for (key, p) in plan::handles(sel) {
            if (q.x - p.x).hypot(q.y - p.y) < tol {
                return Hit::Handle(sel.clone(), key);
            }
        }
    }
    let mut best: Option<&Item> = None;
    let mut bd = tol;
    for it in items {
        let d = plan::distance(it, q);
        if d <= bd {
            bd = d;
            best = Some(it);
        }
    }
    if let Some(it) = best {
        return Hit::Item(it.clone());
    }
    screens
        .iter()
        .rev()
        .find(|s| q.x >= s.x && q.x <= s.x + s.w && q.y >= s.y && q.y <= s.y + s.h)
        .map_or(Hit::Nothing, |s| Hit::Screen(s.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview() -> Preview {
        Preview {
            positions: vec![[0.2, 0.6], [0.8, 0.6]],
            screens: vec![Screen { name: "\\\\.\\DISPLAY1".into(), x: 0.0, y: 0.0, w: 1.0, h: 0.5625, primary: true }],
            active_screen: None,
            colors: vec![[255, 0, 0], [0, 0, 255]],
            devices: vec![("a".into(), 0, 1), ("b".into(), 1, 2)],
        }
    }

    #[test]
    fn stage_draws_lights_and_reports_the_dominant_colour() {
        let size = Size { w: 300.0, h: 200.0, scale: 1.5 };
        let (f, info) = stage(size, [0x1a, 0x1d, 0x24], Some(&preview()), None).unwrap();
        assert_eq!((f.pixels.width(), f.pixels.height()), (450, 300));
        assert_eq!(info.leds, 2);
        let d = info.dominant.unwrap();
        assert!((d[0] - 127.5).abs() < 1e-6 && (d[2] - 127.5).abs() < 1e-6);
        // Un seul écran : pas de nom.
        assert!(f.labels.is_empty());
        // Opaque partout.
        assert!(f.pixels.as_slice().iter().all(|p| p.a == 255));
        let (_, empty) = stage(size, [0, 0, 0], None, None).unwrap();
        assert_eq!((empty.leds, empty.dominant), (0, None));
    }

    #[test]
    fn live_accent_converges_and_stays_readable() {
        let (mut a, mut shown) = ([170.0, 182.0, 255.0], [0, 0, 0]);
        for _ in 0..200 {
            (a, shown) = live_accent(a, [255.0, 0.0, 0.0], 15.0);
        }
        assert!(a[0] > 254.0 && a[1] < 1.0);
        assert_eq!(shown, [226, 30, 30]);
    }

    #[test]
    fn view_fits_everything_and_converts_back() {
        let screens = preview().screens;
        let v = View::fit(400.0, 300.0, &screens, &[]);
        let p = v.to_px(pt(1.0, 0.5625));
        let back = v.to_plan(p.x, p.y);
        assert!((back.x - 1.0).abs() < 1e-9 && (back.y - 0.5625).abs() < 1e-9);
        assert!(v.to_px(pt(0.0, 0.0)).x > 0.0, "marge à gauche");
        assert!(matches!(hit(pt(0.5, 0.3), &v, &[], &screens, None), Hit::Screen(_)));
        assert!(matches!(hit(pt(3.0, 3.0), &v, &[], &screens, None), Hit::Nothing));
    }
}
