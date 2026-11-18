//! Géométrie du plan du bureau (repris de `apps/prisme/lib/plan.ts`) : écrans, éléments
//! déplaçables (zones et segments), conversions écran ↔ plan, aimantation ; et les modifications
//! de la page Disposition et de l'éditeur du plan (`Layout.svelte`, `PlanEditor.svelte`), sur le
//! brouillon JSON. Mêmes conventions que le moteur (`crates/daemon/src/layout.rs`) : unité =
//! largeur de l'écran principal. Logique pure.

use super::types::{DeviceStatus, Screen};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

pub const fn pt(x: f64, y: f64) -> Pt {
    Pt { x, y }
}

/// Forme d'une zone ou d'un segment, relative à son écran (0–1 sur chaque axe).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Point { x: f64, y: f64 },
    Line { from: [f64; 2], to: [f64; 2] },
    Rect { x: f64, y: f64, w: f64, h: f64 },
}

/// Champs de la forme dans le JSON (les autres — nom, LED, écran, segments — sont gardés).
const SHAPE_KEYS: [&str; 7] = ["type", "x", "y", "w", "h", "from", "to"];

fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

fn pair(v: &Value, k: &str) -> [f64; 2] {
    let a = v.get(k).and_then(Value::as_array);
    let at = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
    [at(0), at(1)]
}

/// Arrondi à 1e-4 (celui du plan.ts).
pub fn r4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

impl Shape {
    pub fn read(v: &Value) -> Option<Shape> {
        match v.get("type")?.as_str()? {
            "point" => Some(Shape::Point { x: num(v, "x"), y: num(v, "y") }),
            "line" => Some(Shape::Line { from: pair(v, "from"), to: pair(v, "to") }),
            "rect" => Some(Shape::Rect { x: num(v, "x"), y: num(v, "y"), w: num(v, "w"), h: num(v, "h") }),
            _ => None,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Shape::Point { .. } => "point",
            Shape::Line { .. } => "line",
            Shape::Rect { .. } => "rect",
        }
    }

    fn to_json(self) -> Map<String, Value> {
        let v = match self {
            Shape::Point { x, y } => json!({ "type": "point", "x": x, "y": y }),
            Shape::Line { from, to } => json!({ "type": "line", "from": from, "to": to }),
            Shape::Rect { x, y, w, h } => json!({ "type": "rect", "x": x, "y": y, "w": w, "h": h }),
        };
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    /// Remplace la forme de `target` en gardant ses autres champs (`setShape` du PlanEditor).
    pub fn write(self, target: &mut Map<String, Value>) {
        for k in SHAPE_KEYS {
            target.remove(k);
        }
        target.extend(self.to_json());
    }
}

/// Référence d'un élément du plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemRef {
    pub device: String,
    pub zone: usize,
    /// Segment de la zone, ou `None` pour la zone elle-même (ou ses LED restantes).
    pub segment: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub r: ItemRef,
    pub label: String,
    pub leds: i64,
    /// Indice de l'appareil (couleur).
    pub device_index: usize,
    pub shape: Shape,
    pub screen: Screen,
}

/// Écrans avec les positions corrigées du brouillon (un écran bouge en direct).
pub fn live_screens(screens: &[Screen], cfg: &Value) -> Vec<Screen> {
    screens
        .iter()
        .map(|s| match cfg.get("screens").and_then(|m| m.get(&s.name)) {
            Some(o) => Screen { x: num(o, "x"), y: num(o, "y"), ..s.clone() },
            None => s.clone(),
        })
        .collect()
}

pub fn find_screen(screens: &[Screen], name: Option<&str>) -> Screen {
    screens
        .iter()
        .find(|s| Some(s.name.as_str()) == name)
        .or_else(|| screens.iter().find(|s| s.primary))
        .or_else(|| screens.first())
        .cloned()
        .unwrap_or_else(Screen::fallback)
}

pub fn to_plan(s: &Screen, p: Pt) -> Pt {
    pt(s.x + p.x * s.w, s.y + p.y * s.h)
}

pub fn to_rel(s: &Screen, p: Pt) -> Pt {
    pt((p.x - s.x) / s.w, (p.y - s.y) / s.h)
}

pub fn default_shape(leds: i64) -> Shape {
    if leds > 1 { Shape::Line { from: [0.0, 1.0], to: [1.0, 1.0] } } else { Shape::Point { x: 0.5, y: 1.15 } }
}

fn screen_name(v: &Value) -> Option<&str> {
    v.get("screen").and_then(Value::as_str)
}

/// Appareils du brouillon.
pub fn devices(cfg: &Value) -> &[Value] {
    cfg.get("devices").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

pub fn device_id(d: &Value) -> &str {
    d.get("id").and_then(Value::as_str).unwrap_or_default()
}

/// Zones d'un appareil : celles annoncées par le moteur, sinon une par zone du brouillon.
pub fn zones_of(d: &Value, statuses: &[DeviceStatus]) -> Vec<(String, i64)> {
    let id = device_id(d);
    match statuses.iter().find(|s| s.id == id) {
        Some(s) if !s.zones.is_empty() => s.zones.iter().map(|(n, l)| (n.clone(), *l as i64)).collect(),
        _ => {
            let n = d.get("zones").and_then(Value::as_array).map_or(0, Vec::len);
            (0..n).map(|i| (format!("Zone {}", i + 1), 0)).collect()
        }
    }
}

/// Segments d'une zone du brouillon.
pub fn segments(zone: Option<&Value>) -> &[Value] {
    zone.and_then(|z| z.get("segments")).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn leds_of(v: &Value) -> i64 {
    v.get("leds").and_then(Value::as_i64).unwrap_or(0)
}

/// Tous les éléments déplaçables : une zone, ou ses segments (+ les LED restantes s'il y en a).
pub fn build_items(cfg: &Value, statuses: &[DeviceStatus], screens: &[Screen]) -> Vec<Item> {
    let mut items = Vec::new();
    for (device_index, d) in devices(cfg).iter().enumerate() {
        if d.get("enabled").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let id = device_id(d);
        let zps = d.get("zones").and_then(Value::as_array);
        for (zone, (zname, leds)) in zones_of(d, statuses).into_iter().enumerate() {
            let zp = zps.and_then(|z| z.get(zone));
            let segs = segments(zp);
            let mut used = 0;
            for (segment, seg) in segs.iter().enumerate() {
                let l = leds_of(seg);
                used += l;
                let label = seg.get("name").and_then(Value::as_str).filter(|n| !n.is_empty());
                items.push(Item {
                    r: ItemRef { device: id.to_owned(), zone, segment: Some(segment) },
                    label: label.map_or_else(|| format!("Segment {}", segment + 1), str::to_owned),
                    leds: l,
                    device_index,
                    shape: Shape::read(seg).unwrap_or_else(|| default_shape(l)),
                    screen: find_screen(screens, screen_name(seg)),
                });
            }
            if segs.is_empty() || used < leds {
                items.push(Item {
                    r: ItemRef { device: id.to_owned(), zone, segment: None },
                    label: if segs.is_empty() { zname } else { format!("{zname} (reste)") },
                    leds: leds - used,
                    device_index,
                    shape: zp.and_then(Shape::read).unwrap_or_else(|| default_shape(leds)),
                    screen: find_screen(screens, zp.and_then(screen_name)),
                });
            }
        }
    }
    items
}

/// Point de contrôle d'un élément.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    A,
    B,
    Corner,
    Center,
}

/// Points de contrôle d'un élément, dans le plan.
pub fn handles(it: &Item) -> Vec<(Handle, Pt)> {
    let s = &it.screen;
    match it.shape {
        Shape::Line { from, to } => vec![(Handle::A, to_plan(s, pt(from[0], from[1]))), (Handle::B, to_plan(s, pt(to[0], to[1])))],
        Shape::Rect { x, y, w, h } => vec![(Handle::Corner, to_plan(s, pt(x + w, y + h)))],
        Shape::Point { x, y } => vec![(Handle::Center, to_plan(s, pt(x, y)))],
    }
}

fn seg_dist(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

/// Distance (en unités du plan) d'un point à un élément.
pub fn distance(it: &Item, q: Pt) -> f64 {
    let s = &it.screen;
    match it.shape {
        Shape::Line { from, to } => seg_dist(q, to_plan(s, pt(from[0], from[1])), to_plan(s, pt(to[0], to[1]))),
        Shape::Rect { x, y, w, h } => {
            let (a, b) = (to_plan(s, pt(x, y)), to_plan(s, pt(x + w, y + h)));
            let dx = (a.x - q.x).max(0.0).max(q.x - b.x);
            let dy = (a.y - q.y).max(0.0).max(q.y - b.y);
            dx.hypot(dy)
        }
        Shape::Point { x, y } => {
            let c = to_plan(s, pt(x, y));
            (q.x - c.x).hypot(q.y - c.y)
        }
    }
}

/// Centre d'un élément dans le plan.
pub fn center(it: &Item) -> Pt {
    let s = &it.screen;
    match it.shape {
        Shape::Line { from, to } => to_plan(s, pt((from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0)),
        Shape::Rect { x, y, w, h } => to_plan(s, pt(x + w / 2.0, y + h / 2.0)),
        Shape::Point { x, y } => to_plan(s, pt(x, y)),
    }
}

/// Centre d'une forme relative (pour changer de forme sur place).
pub fn shape_center(s: &Shape) -> Pt {
    match *s {
        Shape::Line { from, to } => pt((from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0),
        Shape::Rect { x, y, w, h } => pt(x + w / 2.0, y + h / 2.0),
        Shape::Point { x, y } => pt(x, y),
    }
}

/// Écran le plus proche d'un point (distance au rectangle), pour ancrer un élément lâché.
pub fn nearest_screen(screens: &[Screen], p: Pt) -> Screen {
    let mut best = screens.first().cloned().unwrap_or_else(Screen::fallback);
    let mut bd = f64::INFINITY;
    for s in screens {
        let dx = (s.x - p.x).max(0.0).max(p.x - (s.x + s.w));
        let dy = (s.y - p.y).max(0.0).max(p.y - (s.y + s.h));
        let d = dx.hypot(dy);
        if d < bd {
            bd = d;
            best = s.clone();
        }
    }
    best
}

/// Ré-exprime une forme (relative à `from`) par rapport à l'écran `to`, sans la déplacer.
pub fn reanchor(shape: Shape, from: &Screen, to: &Screen) -> Shape {
    let conv = |x: f64, y: f64| to_rel(to, to_plan(from, pt(x, y)));
    match shape {
        Shape::Line { from: a, to: b } => {
            let (a, b) = (conv(a[0], a[1]), conv(b[0], b[1]));
            Shape::Line { from: [r4(a.x), r4(a.y)], to: [r4(b.x), r4(b.y)] }
        }
        Shape::Rect { x, y, w, h } => {
            let (a, b) = (conv(x, y), conv(x + w, y + h));
            Shape::Rect { x: r4(a.x), y: r4(a.y), w: r4(b.x - a.x), h: r4(b.y - a.y) }
        }
        Shape::Point { x, y } => {
            let c = conv(x, y);
            Shape::Point { x: r4(c.x), y: r4(c.y) }
        }
    }
}

/// Aimante un point du plan aux bords et coins des écrans (seuil en unités du plan).
pub fn snap(p: Pt, screens: &[Screen], threshold: f64) -> Pt {
    let (mut x, mut y) = (p.x, p.y);
    let (mut bx, mut by) = (threshold, threshold);
    for s in screens {
        for ex in [s.x, s.x + s.w] {
            let d = (p.x - ex).abs();
            if d < bx && p.y > s.y - threshold * 3.0 && p.y < s.y + s.h + threshold * 3.0 {
                bx = d;
                x = ex;
            }
        }
        for ey in [s.y, s.y + s.h] {
            let d = (p.y - ey).abs();
            if d < by && p.x > s.x - threshold * 3.0 && p.x < s.x + s.w + threshold * 3.0 {
                by = d;
                y = ey;
            }
        }
    }
    pt(x, y)
}

fn device_mut<'a>(cfg: &'a mut Value, id: &str) -> Option<&'a mut Map<String, Value>> {
    cfg.get_mut("devices")?.as_array_mut()?.iter_mut().find(|d| device_id(d) == id)?.as_object_mut()
}

/// Zone (ou segment) du brouillon désignée par une référence ; crée la zone si besoin.
pub fn placement_of<'a>(cfg: &'a mut Value, r: &ItemRef, leds: i64) -> Option<&'a mut Map<String, Value>> {
    let d = device_mut(cfg, &r.device)?;
    let zones = d.entry("zones").or_insert_with(|| json!([])).as_array_mut()?;
    while zones.len() <= r.zone {
        zones.push(Value::Object(default_shape(leds).to_json()));
    }
    let z = zones.get_mut(r.zone)?;
    match r.segment {
        None => z.as_object_mut(),
        Some(i) => z.get_mut("segments")?.get_mut(i)?.as_object_mut(),
    }
}

/// Glisser en cours dans l'éditeur du plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    Item { r: ItemRef, handle: Option<Handle>, start: Pt, orig: Shape, screen: Screen, leds: i64 },
    Screen { name: String, start: Pt, orig: Pt },
}

/// Déplace (ou étire) un élément de `d` depuis sa forme d'origine ; `tol` : seuil d'aimantation
/// (0 : aucun). `handle` `None` = le corps.
#[allow(clippy::too_many_arguments)]
pub fn move_item(cfg: &mut Value, screens: &[Screen], r: &ItemRef, handle: Option<Handle>, orig: Shape, s: &Screen, leds: i64, d: Pt, tol: f64) {
    let rel = |p: Pt| to_rel(s, p);
    let snap_p = |p: Pt| if tol > 0.0 { snap(p, screens, tol) } else { p };
    let shape = match orig {
        Shape::Line { from, to } => {
            let (a0, b0) = (to_plan(s, pt(from[0], from[1])), to_plan(s, pt(to[0], to[1])));
            let (a, b) = match handle {
                Some(Handle::A) => (snap_p(pt(a0.x + d.x, a0.y + d.y)), b0),
                Some(Handle::B) => (a0, snap_p(pt(b0.x + d.x, b0.y + d.y))),
                _ => {
                    let na = snap_p(pt(a0.x + d.x, a0.y + d.y));
                    (na, pt(b0.x + (na.x - a0.x), b0.y + (na.y - a0.y)))
                }
            };
            let (ra, rb) = (rel(a), rel(b));
            Shape::Line { from: [r4(ra.x), r4(ra.y)], to: [r4(rb.x), r4(rb.y)] }
        }
        Shape::Rect { x, y, w, h } => {
            let (tl0, br0) = (to_plan(s, pt(x, y)), to_plan(s, pt(x + w, y + h)));
            let (tl, br) = if handle == Some(Handle::Corner) {
                let b = snap_p(pt(br0.x + d.x, br0.y + d.y));
                (tl0, pt(b.x.max(tl0.x + 0.02), b.y.max(tl0.y + 0.02)))
            } else {
                let tl = snap_p(pt(tl0.x + d.x, tl0.y + d.y));
                (tl, pt(br0.x + (tl.x - tl0.x), br0.y + (tl.y - tl0.y)))
            };
            let (a, b) = (rel(tl), rel(br));
            Shape::Rect { x: r4(a.x), y: r4(a.y), w: r4(b.x - a.x), h: r4(b.y - a.y) }
        }
        Shape::Point { x, y } => {
            let c = rel(snap_p(to_plan(s, pt(x + d.x / s.w, y + d.y / s.h))));
            Shape::Point { x: r4(c.x), y: r4(c.y) }
        }
    };
    if let Some(t) = placement_of(cfg, r, leds) {
        shape.write(t);
    }
}

/// Rattache l'élément à l'écran le plus proche de son centre (sans le déplacer).
pub fn reanchor_item(cfg: &mut Value, statuses: &[DeviceStatus], screens: &[Screen], r: &ItemRef) {
    let items = build_items(cfg, statuses, screens);
    let Some(it) = items.iter().find(|i| i.r == *r) else { return };
    let target = nearest_screen(screens, center(it));
    if target.name == it.screen.name {
        return;
    }
    let shape = reanchor(it.shape, &it.screen, &target);
    if let Some(pl) = placement_of(cfg, r, it.leds) {
        shape.write(pl);
        pl.insert("screen".into(), if target.primary { Value::Null } else { json!(target.name) });
    }
}

/// Déplace un écran (aimanté aux bords des autres écrans) : position corrigée du brouillon.
pub fn move_screen(cfg: &mut Value, screens: &[Screen], name: &str, orig: Pt, d: Pt, tol: f64) {
    let Some(me) = screens.iter().find(|s| s.name == name) else { return };
    let (mut x, mut y) = (orig.x + d.x, orig.y + d.y);
    let (mut bx, mut by) = (tol, tol);
    for o in screens.iter().filter(|o| o.name != name) {
        for (mine, theirs) in [(x, o.x + o.w), (x + me.w, o.x), (x, o.x), (x + me.w, o.x + o.w)] {
            if (mine - theirs).abs() < bx {
                bx = (mine - theirs).abs();
                x += theirs - mine;
            }
        }
        for (mine, theirs) in [(y, o.y + o.h), (y + me.h, o.y), (y, o.y), (y + me.h, o.y + o.h)] {
            if (mine - theirs).abs() < by {
                by = (mine - theirs).abs();
                y += theirs - mine;
            }
        }
    }
    if let Some(m) = cfg.as_object_mut() {
        let screens = m.entry("screens").or_insert_with(|| json!({}));
        if let Some(o) = screens.as_object_mut() {
            o.insert(name.to_owned(), json!({ "x": r4(x), "y": r4(y) }));
        }
    }
}

/// Change la forme d'un élément, centrée sur l'ancienne, dans le même repère d'écran.
pub fn set_shape_type(cfg: &mut Value, r: &ItemRef, current: Shape, leds: i64, kind: &str) {
    if current.kind() == kind {
        return;
    }
    let c = shape_center(&current);
    let next = match kind {
        "line" => Shape::Line { from: [c.x - 0.3, c.y], to: [c.x + 0.3, c.y] },
        "rect" => Shape::Rect { x: c.x - 0.3, y: c.y - 0.15, w: 0.6, h: 0.3 },
        _ => Shape::Point { x: c.x, y: c.y },
    };
    if let Some(t) = placement_of(cfg, r, leds) {
        next.write(t);
    }
}

/// Inverse le sens d'une bande (la première LED passe à l'autre bout).
pub fn reverse(cfg: &mut Value, r: &ItemRef, leds: i64) {
    let Some(t) = placement_of(cfg, r, leds) else { return };
    if let Some(Shape::Line { from, to }) = Shape::read(&Value::Object(t.clone())) {
        Shape::Line { from: to, to: from }.write(t);
    }
}

/// Découpe une zone en segments : la zone devient son premier segment.
pub fn split_zone(cfg: &mut Value, device: &str, zone: usize, leds: i64) -> Option<ItemRef> {
    let r = ItemRef { device: device.to_owned(), zone, segment: None };
    let z = placement_of(cfg, &r, leds)?;
    let shape = Shape::read(&Value::Object(z.clone())).unwrap_or_else(|| default_shape(leds));
    let screen = z.get("screen").cloned().unwrap_or(Value::Null);
    let mut seg = shape.to_json();
    seg.insert("screen".into(), screen);
    seg.insert("name".into(), json!("Segment 1"));
    seg.insert("leds".into(), json!(leds));
    z.insert("segments".into(), json!([seg]));
    Some(ItemRef { segment: Some(0), ..r })
}

/// Ajoute un segment à la chaîne, décalé du précédent (pour qu'on le voie et qu'on le glisse).
pub fn add_segment(cfg: &mut Value, device: &str, zone: usize, leds: i64) -> Option<ItemRef> {
    let r = ItemRef { device: device.to_owned(), zone, segment: None };
    let z = placement_of(cfg, &r, leds)?;
    let zone_screen = z.get("screen").cloned().unwrap_or(Value::Null);
    let segs = z.entry("segments").or_insert_with(|| json!([])).as_array_mut()?;
    let used: i64 = segs.iter().map(leds_of).sum();
    let last = segs.last();
    let base = last.and_then(Shape::read).unwrap_or_else(|| default_shape(leds));
    let moved = match base {
        Shape::Line { from, to } => Shape::Line { from: [from[0], from[1] + 0.15], to: [to[0], to[1] + 0.15] },
        Shape::Point { x, y } => Shape::Point { x, y: y + 0.15 },
        Shape::Rect { x, y, w, h } => Shape::Rect { x, y: y + 0.15, w, h },
    };
    let screen = match last.and_then(|l| l.get("screen")) {
        Some(s) if !s.is_null() => s.clone(),
        _ => zone_screen,
    };
    let mut seg = moved.to_json();
    seg.insert("screen".into(), screen);
    seg.insert("name".into(), json!(format!("Segment {}", segs.len() + 1)));
    seg.insert("leds".into(), json!((leds - used).max(1)));
    segs.push(Value::Object(seg));
    Some(ItemRef { segment: Some(segs.len() - 1), ..r })
}

/// Change la place d'un segment dans la chaîne (le premier est branché sur le connecteur).
pub fn move_segment(cfg: &mut Value, device: &str, zone: usize, si: usize, up: bool) -> Option<ItemRef> {
    let d = device_mut(cfg, device)?;
    let segs = d.get_mut("zones")?.get_mut(zone)?.get_mut("segments")?.as_array_mut()?;
    let to = if up { si.checked_sub(1)? } else { si + 1 };
    if to >= segs.len() || si >= segs.len() {
        return None;
    }
    segs.swap(si, to);
    Some(ItemRef { device: device.to_owned(), zone, segment: Some(to) })
}

/// Retire un segment (la liste vide disparaît).
pub fn remove_segment(cfg: &mut Value, device: &str, zone: usize, si: usize) -> ItemRef {
    let r = ItemRef { device: device.to_owned(), zone, segment: None };
    if let Some(z) = device_mut(cfg, device).and_then(|d| d.get_mut("zones")?.get_mut(zone)?.as_object_mut()) {
        let empty = z.get_mut("segments").and_then(Value::as_array_mut).is_some_and(|s| {
            if si < s.len() {
                s.remove(si);
            }
            s.is_empty()
        });
        if empty {
            z.remove("segments");
        }
    }
    r
}

/// Ajoute un appareil trouvé ; renvoie son identifiant (`<pilote>-<n>`).
pub fn add_device(cfg: &mut Value, device: &Value) -> Option<String> {
    let driver = device.get("driver").and_then(Value::as_str)?.to_owned();
    let list = cfg.get_mut("devices")?.as_array_mut()?;
    let mut n = 1;
    while list.iter().any(|e| device_id(e) == format!("{driver}-{n}")) {
        n += 1;
    }
    let id = format!("{driver}-{n}");
    list.push(json!({
        "id": id,
        "enabled": true,
        "device": device,
        "calibration": { "brightness": 1, "white": [1, 1, 1] },
        "zones": [],
    }));
    Some(id)
}

pub fn remove_device(cfg: &mut Value, id: &str) {
    if let Some(list) = cfg.get_mut("devices").and_then(Value::as_array_mut) {
        list.retain(|d| device_id(d) != id);
    }
}

/// Un appareil trouvé est-il déjà configuré (même description de connexion) ?
pub fn already_configured(cfg: &Value, device: &Value) -> bool {
    devices(cfg).iter().any(|e| e.get("device") == Some(device))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<Screen> {
        vec![
            Screen { name: "A".into(), x: 0.0, y: 0.0, w: 1.0, h: 0.5625, primary: true },
            Screen { name: "B".into(), x: 1.0, y: 0.0, w: 1.0, h: 0.5625, primary: false },
        ]
    }

    fn cfg() -> Value {
        json!({
            "devices": [
                { "id": "serial-1", "enabled": true, "device": { "driver": "serial", "port": "COM3", "leds": 90 },
                  "zones": [{ "type": "line", "from": [0, 1], "to": [1, 1], "name": "x" }] },
                { "id": "yeelight-1", "enabled": false, "device": { "driver": "yeelight", "host": "1.2.3.4" }, "zones": [] }
            ],
            "screens": { "B": { "x": 1.2, "y": 0.1 } }
        })
    }

    fn status() -> Vec<DeviceStatus> {
        vec![DeviceStatus { id: "serial-1".into(), zones: vec![("Bande".into(), 90)], ..DeviceStatus::default() }]
    }

    #[test]
    fn items_follow_zones_and_segments() {
        let c = cfg();
        let items = build_items(&c, &status(), &screens());
        assert_eq!(items.len(), 1, "appareil désactivé absent");
        assert_eq!((items[0].label.as_str(), items[0].leds, items[0].screen.name.as_str()), ("Bande", 90, "A"));
        let mut c = cfg();
        let r = split_zone(&mut c, "serial-1", 0, 90).unwrap();
        assert_eq!(r.segment, Some(0));
        let r2 = add_segment(&mut c, "serial-1", 0, 90).unwrap();
        assert_eq!(r2.segment, Some(1));
        let items = build_items(&c, &status(), &screens());
        // Deux segments ; le premier a pris les 90 LED, le second au moins 1 : pas de reste.
        assert_eq!(items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["Segment 1", "Segment 2"]);
        assert_eq!(items[1].shape, Shape::Line { from: [0.0, 1.15], to: [1.0, 1.15] });
        // Le nom gardé à côté de la forme.
        assert_eq!(c["devices"][0]["zones"][0]["name"], json!("x"));
        // Le segment 2 recule… non : il avance.
        assert_eq!(move_segment(&mut c, "serial-1", 0, 1, true).unwrap().segment, Some(0));
        assert_eq!(c["devices"][0]["zones"][0]["segments"][0]["name"], json!("Segment 2"));
        assert!(move_segment(&mut c, "serial-1", 0, 0, true).is_none());
        remove_segment(&mut c, "serial-1", 0, 0);
        remove_segment(&mut c, "serial-1", 0, 0);
        assert!(c["devices"][0]["zones"][0].get("segments").is_none());
    }

    #[test]
    fn live_screens_take_the_draft_positions() {
        let s = live_screens(&screens(), &cfg());
        assert_eq!((s[1].x, s[1].y), (1.2, 0.1));
        assert_eq!(s[0].x, 0.0);
    }

    #[test]
    fn snapping_and_distances() {
        let s = screens();
        assert_eq!(snap(pt(0.995, 0.3), &s, 0.02), pt(1.0, 0.3));
        assert_eq!(snap(pt(0.995, 0.3), &s, 0.0), pt(0.995, 0.3));
        let it = &build_items(&cfg(), &status(), &s)[0];
        assert!((distance(it, pt(0.5, 0.6)) - 0.0375).abs() < 1e-9);
        assert_eq!(center(it), pt(0.5, 0.5625));
        assert_eq!(nearest_screen(&s, pt(1.6, 0.2)).name, "B");
    }

    #[test]
    fn dragging_moves_and_reanchors() {
        let s = screens();
        let mut c = cfg();
        let r = ItemRef { device: "serial-1".into(), zone: 0, segment: None };
        let orig = Shape::Line { from: [0.0, 1.0], to: [1.0, 1.0] };
        // Glissé d'un écran vers la droite, sans aimantation.
        move_item(&mut c, &s, &r, None, orig, &s[0], 90, pt(1.0, 0.0), 0.0);
        assert_eq!(Shape::read(&c["devices"][0]["zones"][0]), Some(Shape::Line { from: [1.0, 1.0], to: [2.0, 1.0] }));
        reanchor_item(&mut c, &status(), &s, &r);
        let z = &c["devices"][0]["zones"][0];
        assert_eq!(z["screen"], json!("B"));
        assert_eq!(Shape::read(z), Some(Shape::Line { from: [0.0, 1.0], to: [1.0, 1.0] }));
        // Étirer une extrémité.
        let orig = Shape::read(&c["devices"][0]["zones"][0]).unwrap();
        move_item(&mut c, &s, &r, Some(Handle::B), orig, &s[1], 90, pt(-0.5, 0.0), 0.0);
        assert_eq!(Shape::read(&c["devices"][0]["zones"][0]), Some(Shape::Line { from: [0.0, 1.0], to: [0.5, 1.0] }));
    }

    #[test]
    fn shapes_change_and_reverse_in_place() {
        let mut c = cfg();
        let r = ItemRef { device: "serial-1".into(), zone: 0, segment: None };
        reverse(&mut c, &r, 90);
        assert_eq!(Shape::read(&c["devices"][0]["zones"][0]), Some(Shape::Line { from: [1.0, 1.0], to: [0.0, 1.0] }));
        set_shape_type(&mut c, &r, Shape::Line { from: [1.0, 1.0], to: [0.0, 1.0] }, 90, "point");
        assert_eq!(Shape::read(&c["devices"][0]["zones"][0]), Some(Shape::Point { x: 0.5, y: 1.0 }));
        assert_eq!(c["devices"][0]["zones"][0]["name"], json!("x"));
    }

    #[test]
    fn screens_snap_to_each_other() {
        let s = screens();
        let mut c = json!({});
        move_screen(&mut c, &s, "B", pt(1.0, 0.0), pt(0.01, 0.3), 0.02);
        assert_eq!(c["screens"]["B"], json!({ "x": 1.0, "y": 0.3 }));
    }

    #[test]
    fn devices_are_added_with_a_free_id() {
        let mut c = cfg();
        let d = json!({ "driver": "serial", "port": "COM4", "leds": 30 });
        assert!(!already_configured(&c, &d));
        assert_eq!(add_device(&mut c, &d).as_deref(), Some("serial-2"));
        assert!(already_configured(&c, &d));
        remove_device(&mut c, "serial-2");
        assert_eq!(devices(&c).len(), 2);
        // Zone créée à la demande.
        let r = ItemRef { device: "yeelight-1".into(), zone: 0, segment: None };
        assert!(placement_of(&mut c, &r, 1).is_some());
        assert_eq!(Shape::read(&c["devices"][1]["zones"][0]), Some(default_shape(1)));
    }
}
