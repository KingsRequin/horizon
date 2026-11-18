//! Page « Disposition » (`pages/Layout.svelte`, `lib/PlanEditor.svelte`,
//! `lib/DeviceSettings.svelte`) : appareils (nom, état, clignotement, pilotage, zones et segments,
//! forme de l'élément choisi, connexion, couleurs), ajout et retrait d'appareils, et l'éditeur du
//! plan du bureau (glisser les éléments et les écrans, extrémités, aimantation coupée par Alt,
//! flèches, Échap). Global `PrismeLayout` ; la toile passe par `PrismeCanvas`.

use super::plan::{self, Drag, ItemRef, Pt, Shape, pt};
use super::render::{self, Frame, Hit, PlanScene, Size, View};
use super::rules::js;
use super::scenes::number;
use super::screens;
use super::types::{DeviceStatus, Screen};
use super::{Ctx, act, call, edited, list, select, sync_rows, with};
use crate::ui::{PrismeCanvas, PrismeDevice, PrismeFound, PrismeLayout, PrismeWindow, PrismeZone};
use serde_json::{Map, Value, json};
use slint::platform::Key;
use slint::{ComponentHandle, SharedString, VecModel};
use std::rc::Rc;
use std::time::Duration;

#[derive(Default)]
pub(super) struct State {
    selected: Option<ItemRef>,
    selected_screen: Option<String>,
    open_device: Option<String>,
    /// Repérage en cours (« appareil:zone:segment »), 4 s.
    blinking: Option<String>,
    error: String,
    found: Option<Vec<(String, Value)>>,
    searching: bool,
    drag: Option<Drag>,
    view: View,
    cursor: i32,
    conn_open: bool,
    colors_open: bool,
    devices: Vec<PrismeDevice>,
    devices_model: Option<Rc<VecModel<PrismeDevice>>>,
    zones: Vec<PrismeZone>,
    zones_model: Option<Rc<VecModel<PrismeZone>>>,
}

thread_local! {
    static BLINK: slint::Timer = slint::Timer::default();
}

/// Nom de chaque pilote.
fn driver_name(d: &str) -> &str {
    match d {
        "logitech" => "Souris Logitech",
        "asus_aura" => "Carte mère ASUS Aura",
        "mountain_everest_max" => "Clavier Mountain Everest Max",
        "serial" => "Bande sur port série",
        "ddp" => "WLED / contrôleur réseau",
        "yeelight" => "Ampoule Yeelight",
        other => other,
    }
}

fn status<'a>(c: &'a Ctx, id: &str) -> Option<&'a DeviceStatus> {
    c.devices.iter().find(|d| d.id == id)
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// Nom du matériel (connecté), sinon le type et son adresse.
fn hardware_name(e: &Value, st: Option<&DeviceStatus>) -> String {
    if let Some(st) = st.filter(|s| s.connected && !s.name.is_empty()) {
        return st.name.clone();
    }
    let d = e.get("device").cloned().unwrap_or(Value::Null);
    let driver = s(&d, "driver");
    let at = match driver {
        "serial" => s(&d, "port"),
        "ddp" | "yeelight" => s(&d, "host"),
        _ => "",
    };
    if at.is_empty() { driver_name(driver).to_owned() } else { format!("{} · {at}", driver_name(driver)) }
}

fn title(e: &Value, st: Option<&DeviceStatus>) -> String {
    let n = s(e, "name").trim();
    if n.is_empty() { hardware_name(e, st) } else { n.to_owned() }
}

fn dot(st: Option<&DeviceStatus>) -> i32 {
    match st {
        Some(s) if s.connected => 1,
        Some(s) if s.error.is_some() => 2,
        _ => 0,
    }
}

fn device<'a>(cfg: &'a Value, id: &str) -> Option<&'a Value> {
    plan::devices(cfg).iter().find(|d| plan::device_id(d) == id)
}

fn device_mut<'a>(c: &'a mut Ctx, id: &str) -> Option<&'a mut Map<String, Value>> {
    c.config.draft_mut()?.get_mut("devices")?.as_array_mut()?.iter_mut().find(|d| plan::device_id(d) == id)?.as_object_mut()
}

/// Appareil ouvert, à modifier.
fn open_mut(c: &mut Ctx) -> Option<&mut Map<String, Value>> {
    let id = c.layout.open_device.clone()?;
    device_mut(c, &id)
}

fn sub<'a>(m: &'a mut Map<String, Value>, k: &str) -> Option<&'a mut Map<String, Value>> {
    m.entry(k).or_insert_with(|| json!({})).as_object_mut()
}

/// Écrans du plan, avec les positions corrigées du brouillon.
fn live_screens(c: &Ctx) -> Vec<Screen> {
    let screens = c.state.as_ref().map_or(&[][..], |s| &s.screens);
    match c.config.draft() {
        Some(cfg) => plan::live_screens(screens, cfg),
        None => screens.to_vec(),
    }
}

/// Élément choisi : zone (nom, LED), placement (`zp`), segment, forme.
struct Sel {
    device: String,
    zone: usize,
    segment: Option<usize>,
    zone_leds: i64,
    segments: Vec<Value>,
    seg: Option<Value>,
    shape: Shape,
}

fn sel(c: &Ctx) -> Option<Sel> {
    let r = c.layout.selected.as_ref()?;
    let cfg = c.config.draft()?;
    let e = device(cfg, &r.device)?;
    let zones = plan::zones_of(e, &c.devices);
    let zone_leds = zones.get(r.zone).map_or(0, |z| z.1);
    let zp = e.get("zones").and_then(|z| z.get(r.zone));
    let segments = plan::segments(zp).to_vec();
    let seg = r.segment.and_then(|i| segments.get(i).cloned());
    let shape = seg
        .as_ref()
        .and_then(Shape::read)
        .or_else(|| zp.and_then(Shape::read))
        .unwrap_or_else(|| plan::default_shape(if zones.get(r.zone).is_some() { zone_leds } else { 1 }));
    Some(Sel { device: r.device.clone(), zone: r.zone, segment: r.segment, zone_leds, segments, seg, shape })
}

fn blink_key(device: &str, zone: Option<usize>, segment: Option<usize>) -> String {
    let n = |v: Option<usize>| v.map(|v| v.to_string()).unwrap_or_default();
    format!("{device}:{}:{}", n(zone), n(segment))
}

/// `2500` mA → « 2,5 » (`toLocaleString('fr-FR', { maximumFractionDigits: 1 })`).
fn amps_fr(ma: f64) -> String {
    let a = (ma / 100.0).round() / 10.0;
    js(a).replace('.', ",")
}

/// Limite de courant conseillée par pilote.
fn auto_limit(driver: &str) -> Option<f64> {
    (driver == "asus_aura").then_some(2500.0)
}

pub(super) fn refresh(c: &mut Ctx) {
    let g = c.win.global::<PrismeLayout>();
    let Some(cfg) = c.config.draft().cloned() else {
        g.set_ready(false);
        return;
    };
    g.set_ready(true);
    let l = &c.layout;
    g.set_error(l.error.as_str().into());

    // Écran choisi sur le plan.
    let screens = c.state.as_ref().map_or(&[][..], |s| &s.screens);
    let screen = l.selected_screen.as_ref().and_then(|n| screens.iter().find(|s| &s.name == n));
    g.set_screen_shown(screen.is_some());
    if let Some(sc) = screen {
        g.set_screen_title(screens::label(Some(&sc.name), screens).into());
        g.set_screen_primary(sc.primary);
        g.set_screen_can_reset(!sc.primary && cfg.get("screens").and_then(|m| m.get(&sc.name)).is_some());
    }

    // Appareils.
    let rows: Vec<PrismeDevice> = plan::devices(&cfg)
        .iter()
        .map(|e| {
            let id = plan::device_id(e);
            let st = status(c, id);
            PrismeDevice {
                id: id.into(),
                title: title(e, st).into(),
                dot: dot(st),
                disabled: e.get("enabled").and_then(Value::as_bool) != Some(true),
                open: l.open_device.as_deref() == Some(id),
            }
        })
        .collect();

    // Appareil ouvert.
    let mut zone_rows = Vec::new();
    if let Some(e) = l.open_device.as_deref().and_then(|id| device(&cfg, id)) {
        let id = plan::device_id(e);
        let st = status(c, id);
        let name = s(e, "name");
        g.set_name(name.into());
        g.set_placeholder(hardware_name(e, st).into());
        let enabled = e.get("enabled").and_then(Value::as_bool) == Some(true);
        let (text, err) = match st {
            Some(st) if st.connected => {
                let pre = if name.is_empty() { String::new() } else { format!("{} · ", hardware_name(e, Some(st))) };
                (format!("{pre}{} LED · {:.0} images/s", st.leds, st.fps), false)
            }
            Some(DeviceStatus { error: Some(e), .. }) => (e.clone(), true),
            _ if !enabled => ("Prisme ne pilote pas cet appareil.".to_owned(), false),
            _ => ("En attente de connexion…".to_owned(), false),
        };
        g.set_state_text(text.into());
        g.set_state_error(err);
        g.set_connected(st.is_some_and(|s| s.connected));
        let blinking = |k: String| l.blinking.as_ref() == Some(&k);
        g.set_blink_text(if blinking(blink_key(id, None, None)) { "Clignote…" } else { "Faire clignoter" }.into());
        g.set_enabled(enabled);
        let zps = e.get("zones").and_then(Value::as_array);
        for (zi, (zname, zleds)) in plan::zones_of(e, &c.devices).into_iter().enumerate() {
            let r = ItemRef { device: id.to_owned(), zone: zi, segment: None };
            zone_rows.push(PrismeZone {
                label: zname.into(),
                count: format!("{zleds} LED").into(),
                active: l.selected.as_ref() == Some(&r),
                zone: zi as i32,
                segment: -1,
            });
            for (si, seg) in plan::segments(zps.and_then(|z| z.get(zi))).iter().enumerate() {
                let r = ItemRef { segment: Some(si), ..r.clone() };
                let label = s(seg, "name");
                zone_rows.push(PrismeZone {
                    label: if label.is_empty() { format!("Segment {}", si + 1) } else { label.to_owned() }.into(),
                    count: format!("{} LED", seg.get("leds").and_then(Value::as_i64).unwrap_or(0)).into(),
                    active: l.selected.as_ref() == Some(&r),
                    zone: zi as i32,
                    segment: si as i32,
                });
            }
        }

        // Élément choisi (s'il est de cet appareil).
        let sl = sel(c).filter(|s| s.device == id);
        g.set_sel_shown(sl.is_some());
        if let Some(sl) = sl {
            g.set_shape(sl.shape.kind().into());
            g.set_locate_text(if blinking(blink_key(id, Some(sl.zone), sl.segment)) { "Clignote…" } else { "Repérer" }.into());
            let mode = if sl.seg.is_some() {
                0
            } else if !sl.segments.is_empty() {
                1
            } else if sl.zone_leds > 1 {
                2
            } else {
                3
            };
            g.set_sel_mode(mode);
            if let Some(seg) = &sl.seg {
                let used: i64 = sl.segments.iter().map(|s| s.get("leds").and_then(Value::as_i64).unwrap_or(0)).sum();
                g.set_seg_name(s(seg, "name").into());
                g.set_seg_leds(seg.get("leds").map(|v| v.to_string()).unwrap_or_default().into());
                let over = used > sl.zone_leds;
                g.set_used_text(
                    format!("{used} LED réparties sur {}{}.", sl.zone_leds, if over { " : trop, la fin est ignorée" } else { "" }).into(),
                );
                g.set_used_error(over);
                g.set_can_forward(sl.segment != Some(0));
                g.set_can_back(sl.segment.map(|i| i + 1) != Some(sl.segments.len()));
            }
        }

        // Réglages de l'appareil.
        let d = e.get("device").cloned().unwrap_or(Value::Null);
        let driver = s(&d, "driver").to_owned();
        g.set_conn_open(l.conn_open);
        g.set_colors_open(l.colors_open);
        g.set_channels(
            d.get("channels")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default()
                .into(),
        );
        g.set_mainboard(d.get("mainboard").and_then(Value::as_bool).unwrap_or(false));
        g.set_port(s(&d, "port").into());
        let num = |k: &str| d.get(k).filter(|v| v.is_number()).map(|v| v.to_string()).unwrap_or_default();
        g.set_leds(num("leds").into());
        g.set_baud(num("baud").into());
        g.set_protocol(
            match s(&d, "protocol") {
                "adalight" => "Adalight",
                "awa" => "AWA (HyperSerial)",
                _ => "",
            }
            .into(),
        );
        g.set_host(s(&d, "host").into());
        let cal = e.get("calibration").cloned().unwrap_or(Value::Null);
        g.set_brightness(cal.get("brightness").and_then(Value::as_f64).unwrap_or(1.0) as f32);
        let white: Vec<f32> =
            (0..3).map(|i| cal.get("white").and_then(|w| w.get(i)).and_then(Value::as_f64).unwrap_or(1.0) as f32).collect();
        g.set_white(list(white));
        let transfer = cal.get("transfer").cloned().unwrap_or(Value::Null);
        g.set_transfer(
            match &transfer {
                Value::Number(_) => "Gamma personnalisé",
                Value::String(t) if t == "linear" => "Linéaire (LED brutes)",
                Value::String(t) if t == "srgb" => "sRGB (corrigé par l'appareil)",
                Value::String(_) => "",
                _ => "Automatique",
            }
            .into(),
        );
        g.set_gamma_shown(transfer.is_number());
        g.set_gamma(if transfer.is_number() { transfer.to_string() } else { String::new() }.into());
        let limit = cal.get("current_limit_ma").and_then(Value::as_f64);
        g.set_limit(
            match limit {
                None => match auto_limit(&driver) {
                    Some(a) => format!("Auto : {} A", amps_fr(a)),
                    None => "Auto : aucune".to_owned(),
                },
                Some(0.0) => "Aucune".to_owned(),
                Some(_) => "Personnalisée".to_owned(),
            }
            .into(),
        );
        g.set_limit_custom(limit.is_some_and(|v| v != 0.0));
        g.set_amps(js(limit.unwrap_or(0.0) / 1000.0).into());
        g.set_dither(cal.get("dither").and_then(Value::as_bool) == Some(true));
        g.set_driver(driver.into());
    } else {
        g.set_sel_shown(false);
    }

    // Ajout d'appareils.
    g.set_searching(l.searching);
    g.set_has_found(l.found.is_some());
    let found: Vec<PrismeFound> = l
        .found
        .iter()
        .flatten()
        .map(|(desc, d)| PrismeFound { description: desc.as_str().into(), done: plan::already_configured(&cfg, d) })
        .collect();
    g.set_found(list(found));

    let devices_model = c.layout.devices_model.get_or_insert_with(|| {
        let m = Rc::new(VecModel::default());
        g.set_devices(m.clone().into());
        m
    });
    sync_rows(devices_model, &mut c.layout.devices, rows, Clone::clone);
    let zones_model = c.layout.zones_model.get_or_insert_with(|| {
        let m = Rc::new(VecModel::default());
        g.set_zones(m.clone().into());
        m
    });
    sync_rows(zones_model, &mut c.layout.zones, zone_rows, Clone::clone);
}

/// Image de l'éditeur du plan (cadrage refait, sauf pendant un glissement).
pub(super) fn frame(c: &mut Ctx, size: Size) -> Option<Frame> {
    let cfg = c.config.draft()?;
    let screens = live_screens(c);
    let items = plan::build_items(cfg, &c.devices, &screens);
    if c.layout.drag.is_none() {
        c.layout.view = View::fit(size.w as f64, size.h as f64, &screens, &items);
    }
    let l = &c.layout;
    let scene = PlanScene {
        screens: &screens,
        items: &items,
        selected: l.selected.as_ref(),
        selected_screen: l.selected_screen.as_deref(),
        focus_device: l.open_device.as_deref(),
        preview: c.preview.as_ref(),
        view: l.view,
    };
    render::plan(size, [0x1a, 0x1d, 0x24], &scene)
}

/// Sélection d'un élément du plan : son appareil s'ouvre dans la liste.
fn choose(c: &mut Ctx, r: Option<ItemRef>) {
    if let Some(r) = &r
        && c.layout.open_device.as_ref() != Some(&r.device)
    {
        c.layout.open_device = Some(r.device.clone());
        c.layout.conn_open = false;
        c.layout.colors_open = false;
    }
    c.layout.selected = r;
}

fn set_cursor(c: &mut Ctx, cursor: i32) {
    if c.layout.cursor != cursor {
        c.layout.cursor = cursor;
        c.win.global::<PrismeCanvas>().set_cursor(cursor);
    }
}

fn hit(c: &Ctx, q: Pt) -> Hit {
    let Some(cfg) = c.config.draft() else { return Hit::Nothing };
    let screens = live_screens(c);
    let items = plan::build_items(cfg, &c.devices, &screens);
    render::hit(q, &c.layout.view, &items, &screens, c.layout.selected.as_ref())
}

/// Souris sur le plan : 0 appui, 1 déplacement, 2 relâchement.
pub(super) fn pointer(c: &mut Ctx, kind: i32, x: f64, y: f64, alt: bool) {
    if c.page != super::Page::Layout || c.config.draft().is_none() {
        return;
    }
    let q = c.layout.view.to_plan(x, y);
    match kind {
        0 => {
            match hit(c, q) {
                Hit::Handle(it, h) => {
                    choose(c, Some(it.r.clone()));
                    c.layout.selected_screen = None;
                    c.layout.drag = Some(Drag::Item { r: it.r, handle: Some(h), start: q, orig: it.shape, screen: it.screen, leds: it.leds });
                }
                Hit::Item(it) => {
                    choose(c, Some(it.r.clone()));
                    c.layout.selected_screen = None;
                    c.layout.drag = Some(Drag::Item { r: it.r, handle: None, start: q, orig: it.shape, screen: it.screen, leds: it.leds });
                }
                Hit::Screen(s) => {
                    choose(c, None);
                    c.layout.selected_screen = Some(s.name.clone());
                    if !s.primary {
                        c.layout.drag = Some(Drag::Screen { name: s.name, start: q, orig: pt(s.x, s.y) });
                    }
                }
                Hit::Nothing => {
                    choose(c, None);
                    c.layout.selected_screen = None;
                }
            }
            super::refresh(c);
        }
        1 => {
            let Some(drag) = c.layout.drag.clone() else {
                let cursor = match hit(c, q) {
                    Hit::Handle(..) => 3,
                    Hit::Item(_) => 1,
                    Hit::Screen(s) if !s.primary => 4,
                    _ => 0,
                };
                set_cursor(c, cursor);
                return;
            };
            let tol = if alt { 0.0 } else { 8.0 / c.layout.view.k };
            let screens = live_screens(c);
            match drag {
                Drag::Screen { name, start, orig } => {
                    set_cursor(c, 2);
                    if let Some(cfg) = c.config.draft_mut() {
                        plan::move_screen(cfg, &screens, &name, orig, pt(q.x - start.x, q.y - start.y), tol);
                    }
                }
                Drag::Item { r, handle, start, orig, screen, leds } => {
                    set_cursor(c, if handle.is_some() { 3 } else { 2 });
                    if let Some(cfg) = c.config.draft_mut() {
                        plan::move_item(cfg, &screens, &r, handle, orig, &screen, leds, pt(q.x - start.x, q.y - start.y), tol);
                    }
                }
            }
            edited(c);
        }
        _ => {
            let Some(drag) = c.layout.drag.take() else { return };
            if let Drag::Item { r, .. } = drag {
                let screens = live_screens(c);
                let devices = c.devices.clone();
                if let Some(cfg) = c.config.draft_mut() {
                    plan::reanchor_item(cfg, &devices, &screens, &r);
                }
            }
            edited(c);
            pointer(c, 1, x, y, alt);
        }
    }
}

/// Clavier du plan : flèches (Maj : pas plus grand), Échap. Vrai si la touche est prise.
pub(super) fn key(c: &mut Ctx, text: &str, shift: bool) -> bool {
    let step = if shift { 0.05 } else { 0.005 };
    let k = |key: Key| text == SharedString::from(key).as_str();
    let d = if k(Key::LeftArrow) {
        pt(-step, 0.0)
    } else if k(Key::RightArrow) {
        pt(step, 0.0)
    } else if k(Key::UpArrow) {
        pt(0.0, -step)
    } else if k(Key::DownArrow) {
        pt(0.0, step)
    } else {
        if k(Key::Escape) {
            c.layout.selected = None;
            c.layout.selected_screen = None;
            super::refresh(c);
            return true;
        }
        return false;
    };
    let screens = live_screens(c);
    if let Some(r) = c.layout.selected.clone() {
        let Some(cfg) = c.config.draft() else { return true };
        let items = plan::build_items(cfg, &c.devices, &screens);
        let Some(it) = items.into_iter().find(|i| i.r == r) else { return true };
        let devices = c.devices.clone();
        if let Some(cfg) = c.config.draft_mut() {
            plan::move_item(cfg, &screens, &r, None, it.shape, &it.screen, it.leds, d, 0.0);
            plan::reanchor_item(cfg, &devices, &screens, &r);
        }
        edited(c);
    } else if let Some(name) = c.layout.selected_screen.clone() {
        let sc = plan::find_screen(&screens, Some(&name));
        if !sc.primary
            && let Some(cfg) = c.config.draft_mut()
        {
            plan::move_screen(cfg, &screens, &sc.name, pt(sc.x, sc.y), d, 0.0);
            edited(c);
        }
    }
    true
}

/// Fait clignoter un appareil, une zone ou un segment (4 s).
fn identify(device: String, zone: Option<usize>, segment: Option<usize>) {
    let key = blink_key(&device, zone, segment);
    call("identify", json!({ "device": device, "zone": zone, "segment": segment }), move |r| {
        with(|c| {
            match r {
                Ok(_) => {
                    c.layout.error.clear();
                    c.layout.blinking = Some(key);
                    BLINK.with(|t| {
                        t.start(slint::TimerMode::SingleShot, Duration::from_secs(4), || {
                            with(|c| {
                                c.layout.blinking = None;
                                super::refresh(c);
                            });
                        })
                    });
                }
                Err(e) => c.layout.error = e,
            }
            super::refresh(c);
        });
    });
}

/// Modifie l'appareil ouvert et enregistre.
fn edit_open(f: impl FnOnce(&mut Map<String, Value>) + 'static) {
    act(move |c| {
        if let Some(e) = open_mut(c) {
            f(e);
            edited(c);
        }
    });
}

fn parse_num(t: &str) -> Option<f64> {
    t.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

fn set_text(e: &mut Map<String, Value>, key: &str, t: &str) {
    match key {
        "channels" => {
            let v: Vec<i64> = t.split(',').filter_map(|p| p.trim().parse::<i64>().ok()).filter(|v| *v >= 0).collect();
            if let Some(d) = sub(e, "device") {
                d.insert("channels".into(), json!(v));
            }
        }
        "port" | "host" => {
            if let Some(d) = sub(e, "device") {
                d.insert(key.into(), json!(t));
            }
        }
        "leds" | "baud" => {
            if let (Some(v), Some(d)) = (parse_num(t), sub(e, "device")) {
                d.insert(key.into(), number(v));
            }
        }
        "gamma" => {
            if let (Some(v), Some(cal)) = (parse_num(t), sub(e, "calibration")) {
                cal.insert("transfer".into(), number(v));
            }
        }
        "amps" => {
            if let (Some(v), Some(cal)) = (parse_num(t), sub(e, "calibration")) {
                cal.insert("current_limit_ma".into(), json!(((v * 1000.0).round() as i64).max(100)));
            }
        }
        _ => {}
    }
}

/// ↑ / ↓ d'un champ numérique.
fn step_text(e: &mut Map<String, Value>, key: &str, d: i32) {
    let cur = |m: Option<&Map<String, Value>>, k: &str| m.and_then(|m| m.get(k)).and_then(Value::as_f64);
    let (v, step, min) = match key {
        "leds" => (cur(e.get("device").and_then(Value::as_object), "leds").unwrap_or(0.0), 1.0, 1.0),
        "baud" => (cur(e.get("device").and_then(Value::as_object), "baud").unwrap_or(0.0), 1.0, 0.0),
        "gamma" => (cur(e.get("calibration").and_then(Value::as_object), "transfer").unwrap_or(2.2), 0.1, 0.5),
        "amps" => (cur(e.get("calibration").and_then(Value::as_object), "current_limit_ma").unwrap_or(0.0) / 1000.0, 0.1, 0.1),
        _ => return,
    };
    let next = ((v + step * d as f64).max(min) * 10.0).round() / 10.0;
    set_text(e, key, &js(next));
}

pub(super) fn bind(win: &PrismeWindow) {
    let g = win.global::<PrismeLayout>();
    g.on_reset_screen(|| {
        act(|c| {
            let Some(name) = c.layout.selected_screen.clone() else { return };
            if let Some(m) = c.config.draft_mut().and_then(|d| d.get_mut("screens")).and_then(Value::as_object_mut) {
                m.remove(&name);
                edited(c);
            }
        })
    });
    g.on_toggle_device(|id| {
        act(move |c| {
            let id = id.to_string();
            let l = &mut c.layout;
            l.open_device = if l.open_device.as_ref() == Some(&id) { None } else { Some(id.clone()) };
            l.conn_open = false;
            l.colors_open = false;
            if l.selected.as_ref().map(|s| &s.device) != Some(&id) {
                l.selected = None;
            }
            l.selected_screen = None;
            super::refresh(c);
        })
    });
    g.on_rename(|t| {
        let name: String = t.trim().chars().take(48).collect();
        edit_open(move |e| {
            e.insert("name".into(), json!(name));
        })
    });
    g.on_identify(|zone, _| {
        let target = with(|c| {
            let id = c.layout.open_device.clone()?;
            if zone == -1 {
                return Some((id, None, None));
            }
            let s = c.layout.selected.clone()?;
            Some((s.device, Some(s.zone), s.segment))
        })
        .flatten();
        if let Some((d, z, s)) = target {
            identify(d, z, s);
        }
    });
    g.on_set_enabled(|v| edit_open(move |e| {
        e.insert("enabled".into(), json!(v));
    }));
    g.on_select_zone(|z, s| {
        act(move |c| {
            let Some(id) = c.layout.open_device.clone() else { return };
            choose(c, Some(ItemRef { device: id, zone: z.max(0) as usize, segment: (s >= 0).then_some(s as usize) }));
            super::refresh(c);
        })
    });
    g.on_set_shape(|t| {
        act(move |c| {
            let (Some(r), Some(sl)) = (c.layout.selected.clone(), sel(c)) else { return };
            if let Some(cfg) = c.config.draft_mut() {
                plan::set_shape_type(cfg, &r, sl.shape, sl.zone_leds, &t);
                edited(c);
            }
        })
    });
    g.on_reverse(|| {
        act(|c| {
            let (Some(r), Some(sl)) = (c.layout.selected.clone(), sel(c)) else { return };
            if let Some(cfg) = c.config.draft_mut() {
                plan::reverse(cfg, &r, sl.zone_leds);
                edited(c);
            }
        })
    });
    g.on_seg_name_edited(|t| edit_seg(move |seg| {
        seg.insert("name".into(), json!(t.as_str()));
    }));
    g.on_seg_leds_edited(|t| {
        if let Some(v) = parse_num(&t) {
            edit_seg(move |seg| {
                seg.insert("leds".into(), number(v.round()));
            });
        }
    });
    g.on_seg_leds_step(|d| edit_seg(move |seg| {
        let v = seg.get("leds").and_then(Value::as_i64).unwrap_or(1);
        seg.insert("leds".into(), json!((v + d as i64).max(1)));
    }));
    g.on_move_segment(|d| {
        act(move |c| {
            let Some(r) = c.layout.selected.clone() else { return };
            let Some(si) = r.segment else { return };
            let next = c.config.draft_mut().and_then(|cfg| plan::move_segment(cfg, &r.device, r.zone, si, d < 0));
            if let Some(n) = next {
                c.layout.selected = Some(n);
                edited(c);
            }
        })
    });
    g.on_add_segment(|| {
        act(|c| {
            let Some(sl) = sel(c) else { return };
            let next = c.config.draft_mut().and_then(|cfg| plan::add_segment(cfg, &sl.device, sl.zone, sl.zone_leds));
            if let Some(n) = next {
                c.layout.selected = Some(n);
                edited(c);
            }
        })
    });
    g.on_remove_segment(|| {
        act(|c| {
            let Some(r) = c.layout.selected.clone() else { return };
            let Some(si) = r.segment else { return };
            if let Some(cfg) = c.config.draft_mut() {
                c.layout.selected = Some(plan::remove_segment(cfg, &r.device, r.zone, si));
                edited(c);
            }
        })
    });
    g.on_split(|| {
        act(|c| {
            let Some(sl) = sel(c) else { return };
            let next = c.config.draft_mut().and_then(|cfg| plan::split_zone(cfg, &sl.device, sl.zone, sl.zone_leds));
            if let Some(n) = next {
                c.layout.selected = Some(n);
                edited(c);
            }
        })
    });
    g.on_remove_device(|| {
        act(|c| {
            let Some(id) = c.layout.open_device.take() else { return };
            if let Some(cfg) = c.config.draft_mut() {
                plan::remove_device(cfg, &id);
            }
            c.layout.selected = None;
            edited(c);
        })
    });
    g.on_toggle_conn(|| {
        act(|c| {
            c.layout.conn_open = !c.layout.conn_open;
            super::refresh(c);
        })
    });
    g.on_toggle_colors(|| {
        act(|c| {
            c.layout.colors_open = !c.layout.colors_open;
            super::refresh(c);
        })
    });
    g.on_set_text(|k, t| edit_open(move |e| set_text(e, &k, &t)));
    g.on_step_text(|k, d| edit_open(move |e| step_text(e, &k, d)));
    g.on_set_check(|k, v| {
        edit_open(move |e| match k.as_str() {
            "mainboard" => {
                if let Some(d) = sub(e, "device") {
                    d.insert("mainboard".into(), json!(v));
                }
            }
            "dither" => {
                if let Some(cal) = sub(e, "calibration") {
                    cal.insert("dither".into(), if v { json!(true) } else { Value::Null });
                }
            }
            _ => {}
        })
    });
    g.on_set_number(|k, v| {
        let v = super::effects::snap(v as f64, 0.01, 0.0, if k == "brightness" { 1.0 } else { 1.5 });
        edit_open(move |e| {
            let Some(cal) = sub(e, "calibration") else { return };
            match k.as_str() {
                "brightness" => {
                    cal.insert("brightness".into(), number(v));
                }
                w => {
                    let i = match w {
                        "w0" => 0,
                        "w1" => 1,
                        _ => 2,
                    };
                    let white = cal.entry("white").or_insert_with(|| json!([1, 1, 1]));
                    if let Some(slot) = white.as_array_mut().and_then(|a| a.get_mut(i)) {
                        *slot = number(v);
                    }
                }
            }
        })
    });
    g.on_pick(|k, x, y| {
        act(move |c| {
            let Some(id) = c.layout.open_device.clone() else { return };
            let Some(e) = c.config.draft().and_then(|cfg| device(cfg, &id)).cloned() else { return };
            let driver = e.pointer("/device/driver").and_then(Value::as_str).unwrap_or_default().to_owned();
            let (choices, current): (Vec<(&str, String)>, String) = match k.as_str() {
                "protocol" => (
                    vec![("adalight", "Adalight".into()), ("awa", "AWA (HyperSerial)".into())],
                    e.pointer("/device/protocol").and_then(Value::as_str).unwrap_or_default().to_owned(),
                ),
                "transfer" => {
                    let cur = match e.pointer("/calibration/transfer") {
                        Some(Value::Number(_)) => "gamma".to_owned(),
                        Some(Value::String(t)) => t.clone(),
                        _ => "auto".to_owned(),
                    };
                    (
                        vec![
                            ("auto", "Automatique".into()),
                            ("linear", "Linéaire (LED brutes)".into()),
                            ("srgb", "sRGB (corrigé par l'appareil)".into()),
                            ("gamma", "Gamma personnalisé".into()),
                        ],
                        cur,
                    )
                }
                "limit" => {
                    let cur = match e.pointer("/calibration/current_limit_ma").and_then(Value::as_f64) {
                        None => "auto",
                        Some(0.0) => "none",
                        Some(_) => "custom",
                    };
                    let auto = auto_limit(&driver).map_or_else(|| "Auto : aucune".to_owned(), |a| format!("Auto : {} A", amps_fr(a)));
                    (vec![("auto", auto), ("none", "Aucune".into()), ("custom", "Personnalisée".into())], cur.to_owned())
                }
                _ => return,
            };
            let choices = choices.into_iter().map(|(a, b)| (a.to_owned(), b)).collect();
            let key = k.to_string();
            select(c, x, y, choices, &current, move |c, chosen| {
                let Some(e) = open_mut(c) else { return };
                match key.as_str() {
                    "protocol" => {
                        if let Some(d) = sub(e, "device") {
                            d.insert("protocol".into(), json!(chosen));
                        }
                    }
                    "transfer" => {
                        if let Some(cal) = sub(e, "calibration") {
                            let v = match chosen.as_str() {
                                "auto" => Value::Null,
                                "gamma" => json!(2.2),
                                t => json!(t),
                            };
                            cal.insert("transfer".into(), v);
                        }
                    }
                    _ => {
                        if let Some(cal) = sub(e, "calibration") {
                            let v = match chosen.as_str() {
                                "auto" => Value::Null,
                                "none" => json!(0),
                                _ => number(auto_limit(&driver).unwrap_or(2500.0)),
                            };
                            cal.insert("current_limit_ma".into(), v);
                        }
                    }
                }
                edited(c);
            });
        })
    });
    g.on_discover(|| {
        act(|c| {
            c.layout.searching = true;
            super::refresh(c);
        });
        call("discover", json!({}), |r| {
            with(|c| {
                match r {
                    Ok(v) => {
                        let found = v
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .map(|f| (s(f, "description").to_owned(), f.get("device").cloned().unwrap_or(Value::Null)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        c.layout.found = Some(found);
                        c.layout.error.clear();
                    }
                    Err(e) => c.layout.error = e,
                }
                c.layout.searching = false;
                super::refresh(c);
            });
        });
    });
    g.on_add_found(|i| {
        act(move |c| {
            let Some((_, d)) = c.layout.found.as_ref().and_then(|f| f.get(i as usize)).cloned() else { return };
            let id = c.config.draft_mut().and_then(|cfg| plan::add_device(cfg, &d));
            if let Some(id) = id {
                c.layout.open_device = Some(id);
                c.layout.conn_open = false;
                c.layout.colors_open = false;
                edited(c);
            }
        })
    });
}

/// Modifie le segment choisi et enregistre.
fn edit_seg(f: impl FnOnce(&mut Map<String, Value>) + 'static) {
    act(move |c| {
        let Some(r) = c.layout.selected.clone() else { return };
        if r.segment.is_none() {
            return;
        }
        let Some(cfg) = c.config.draft_mut() else { return };
        if let Some(seg) = plan::placement_of(cfg, &r, 0) {
            f(seg);
            edited(c);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_are_named_like_the_tauri_page() {
        let e = json!({ "id": "serial-1", "device": { "driver": "serial", "port": "COM3" } });
        assert_eq!(hardware_name(&e, None), "Bande sur port série · COM3");
        let st = DeviceStatus { connected: true, name: "Arduino".into(), ..DeviceStatus::default() };
        assert_eq!(hardware_name(&e, Some(&st)), "Arduino");
        let named = json!({ "name": " Bureau ", "device": { "driver": "logitech" } });
        assert_eq!(title(&named, None), "Bureau");
        assert_eq!(title(&json!({ "device": { "driver": "logitech" } }), None), "Souris Logitech");
    }

    #[test]
    fn connection_and_colour_fields() {
        let mut e = json!({ "device": { "driver": "asus_aura" }, "calibration": { "brightness": 1, "white": [1, 1, 1] } });
        let m = e.as_object_mut().unwrap();
        set_text(m, "channels", "60, x, -2, 30");
        set_text(m, "amps", "0,05");
        set_text(m, "gamma", "2.4");
        step_text(m, "gamma", 1);
        assert_eq!(e["device"]["channels"], json!([60, 30]));
        assert_eq!(e["calibration"]["current_limit_ma"], json!(100));
        assert_eq!(e["calibration"]["transfer"], json!(2.5));
        assert_eq!(amps_fr(2500.0), "2,5");
        assert_eq!(amps_fr(3000.0), "3");
        assert_eq!(blink_key("a", Some(0), None), "a:0:");
    }
}
