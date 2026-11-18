//! Panneau Essaim (global `EssaimPanelData` de `ui/widgets/essaim/panel.slint`) : état, pause et
//! réglages en direct (`SETTING_GROUPS` d'essaim.ts). Une valeur choisie reste affichée jusqu'à
//! ce qu'Essaim la renvoie (au plus `PENDING`) ; un curseur n'envoie qu'après `DEBOUNCE` sans
//! mouvement, ou à son relâchement (Essaim réécrit son fichier à chaque valeur) ; ce qui attend
//! part à la fermeture du panneau. Fil de l'interface.

use super::{EV_ESSAIM, Essaim, essaim_open_config, essaim_set_config, essaim_set_paused, essaim_state};
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{EssaimPanelData, EssaimPanelGroup, EssaimPanelSetting, model};
use crate::widgets::Panel;
use serde_json::{Value, json};
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(super) const PANEL: Panel = Panel { opened: None, closed: Some(closed), ..Panel::new(bind) };

/// Envoi d'un curseur après ce délai sans mouvement.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Au-delà, une valeur envoyée sans écho d'Essaim (refusée, Essaim fermé) n'est plus affichée.
const PENDING: Duration = Duration::from_secs(4);

/// Curseur : bornes, pas, unité, texte de la valeur 0.
#[derive(Debug, Clone, Copy)]
struct Range {
    min: f64,
    max: f64,
    step: f64,
    unit: Option<&'static str>,
    zero: Option<&'static str>,
}

const fn range(min: f64, max: f64, step: f64, unit: Option<&'static str>, zero: Option<&'static str>) -> Option<Range> {
    Some(Range { min, max, step, unit, zero })
}

/// Un réglage du panneau ; sans `range` : case à cocher.
struct Setting {
    section: &'static str,
    key: &'static str,
    label: &'static str,
    range: Option<Range>,
}

const fn s(section: &'static str, key: &'static str, label: &'static str, range: Option<Range>) -> Setting {
    Setting { section, key, label, range }
}

/// Réglages du panneau, par groupe (`SETTING_GROUPS`).
const GROUPS: [(&str, &[Setting]); 3] = [
    (
        "Orbite",
        &[
            s("orbite", "rayon", "Rayon", range(10.0, 40.0, 1.0, Some("px"), None)),
            s("orbite", "vitesse_base", "Vitesse de rotation", range(0.0, 10.0, 0.1, None, None)),
            s("orbite", "vitesse_basse", "Accélération par la basse", range(0.0, 25.0, 0.5, None, None)),
            s("orbite", "eloignement_grave", "Éloignement sur les graves", range(0.0, 2.5, 0.1, None, None)),
            s("orbite", "trainee", "Longueur des traînées", range(0.05, 1.0, 0.05, None, None)),
        ],
    ),
    (
        "Satellites",
        &[
            s("essaim", "duree", "Durée de vie", range(0.5, 6.0, 0.1, Some("s"), None)),
            s("essaim", "maximum", "Nombre maximal", range(0.0, 30.0, 1.0, None, None)),
            s("audio", "sensibilite_kick", "Sensibilité au kick", range(1.0, 10.0, 0.5, None, None)),
        ],
    ),
    (
        "Couleur et effets",
        &[
            s("couleur", "synchro_prisme", "Couleur de l’éclairage Prisme", None),
            s("couleur", "vitesse_teinte", "Vitesse de la teinte", range(0.0, 180.0, 5.0, Some("°/s"), None)),
            s("fissures", "actives", "Fissures sous les clics", None),
            s("curseur", "variantes", "Variantes du curseur (texte, lien…)", None),
            s("rendu", "cadence_repos", "Cadence au repos", range(0.0, 60.0, 1.0, Some("im/s"), Some("sans repos"))),
        ],
    ),
];

impl Setting {
    fn id(&self) -> String {
        format!("{}.{}", self.section, self.key)
    }
}

fn find(id: &str) -> Option<&'static Setting> {
    GROUPS.iter().flat_map(|(_, list)| list.iter()).find(|s| s.id() == id)
}

/// Nombre de décimales d'un pas (0,05 → 2).
fn decimals(step: f64) -> usize {
    let t = format!("{step}");
    t.split('.').nth(1).map_or(0, str::len)
}

/// Valeur ramenée au pas (Essaim garde ses nombres en simple précision : 0,30000001).
fn snap(value: f64, step: f64) -> f64 {
    let d = decimals(step) as i32;
    let p = 10f64.powi(d);
    ((value / step).round() * step * p).round() / p
}

/// Valeur affichée (`formatValue`) : « 2,8 s », « 22 px », « sans repos ».
fn format_value(value: f64, r: &Range) -> String {
    if value == 0.0
        && let Some(z) = r.zero
    {
        return z.to_owned();
    }
    let d = decimals(r.step);
    let mut text = format!("{:.*}", d, snap(value, r.step));
    // Décimales inutiles retirées (`maximumFractionDigits`), virgule française.
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_owned();
    }
    let text = text.replace('.', ",");
    match r.unit {
        Some(u) => format!("{text} {u}"),
        None => text,
    }
}

/// Nombre JSON comme l'écrit la page : entier s'il n'a pas de décimales (un réglage entier
/// d'Essaim reste entier).
fn number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 1e15 { json!(x as i64) } else { json!(x) }
}

/// Même valeur, aux arrondis de la simple précision près (`sameValue`).
fn same_value(a: Option<&Value>, b: &Value) -> bool {
    match (a.and_then(Value::as_f64), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < 1e-3,
        _ => a == Some(b),
    }
}

#[derive(Default)]
struct Local {
    /// Dernier état reçu.
    state: Essaim,
    /// Valeurs choisies, pas encore revenues dans l'état d'Essaim.
    pending: HashMap<String, (Value, Instant)>,
    /// Valeur qui attend la fin du délai d'un curseur.
    waiting: HashMap<String, Value>,
    /// Délai de chaque curseur (gardé : jamais détruit pendant son propre rappel).
    timers: HashMap<String, slint::Timer>,
}

thread_local! {
    static APP: RefCell<Option<AppHandle>> = const { RefCell::new(None) };
    static LOCAL: RefCell<Local> = RefCell::default();
    static GROUP_MODELS: Vec<Rc<VecModel<EssaimPanelSetting>>> = GROUPS.iter().map(|_| Rc::new(VecModel::default())).collect();
}

fn bind(app: &AppHandle, win: &Host) {
    APP.set(Some(app.clone()));
    let g = win.global::<EssaimPanelData>();
    let groups: Vec<EssaimPanelGroup> = GROUP_MODELS.with(|m| {
        GROUPS.iter().zip(m).map(|((title, _), m)| EssaimPanelGroup { title: (*title).into(), settings: ModelRc::from(Rc::clone(m)) }).collect()
    });
    g.set_groups(ModelRc::new(VecModel::from(groups)));
    let a = app.clone();
    g.on_set_paused(move |p| report(essaim_set_paused(a.clone(), p)));
    let a = app.clone();
    g.on_open_config(move || report(essaim_open_config(a.clone())));
    g.on_slide(|id, v| {
        let Some(r) = find(&id).and_then(|s| s.range) else { return };
        // Le curseur rend un `f32` : ramené au pas pour être envoyé tel quel.
        let value = number(snap(f64::from(v), r.step));
        choose(&id, value.clone());
        push(id.to_string(), value);
        show();
    });
    g.on_release(|id| flush(&id));
    g.on_check(|id, v| {
        choose(&id, json!(v));
        push(id.to_string(), json!(v));
        flush(&id);
        show();
    });

    LOCAL.with_borrow_mut(|l| l.state = essaim_state(app.clone()));
    show();
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_ESSAIM, Some(LABEL), |e| {
            if let Some(x) = e.payload::<Essaim>() {
                LOCAL.with_borrow_mut(|l| {
                    l.state = x.clone();
                    // Écho d'Essaim (ou délai dépassé) : la valeur affichée redevient la sienne.
                    let config = &l.state.config;
                    l.pending.retain(|id, (v, at)| {
                        let (section, key) = id.split_once('.').unwrap_or_default();
                        !same_value(config.get(section).and_then(|s| s.get(key)), v) && at.elapsed() <= PENDING
                    });
                });
                show();
            }
        });
    }
}

/// Panneau fermé pendant un glissement : la dernière valeur part quand même.
fn closed(_app: &AppHandle, _kind: &str) {
    let ids: Vec<String> = LOCAL.with_borrow(|l| l.waiting.keys().cloned().collect());
    for id in ids {
        flush(&id);
    }
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::info!("panneau Essaim : {e}");
    }
}

/// La valeur choisie est affichée en attendant l'écho d'Essaim.
fn choose(id: &str, value: Value) {
    LOCAL.with_borrow_mut(|l| {
        l.pending.insert(id.to_owned(), (value, Instant::now()));
    });
}

/// Valeur envoyée `DEBOUNCE` après le dernier mouvement.
fn push(id: String, value: Value) {
    LOCAL.with_borrow_mut(|l| {
        let i = id.clone();
        l.timers.entry(id.clone()).or_default().start(slint::TimerMode::SingleShot, DEBOUNCE, move || flush(&i));
        l.waiting.insert(id, value);
    });
}

/// Envoie tout de suite la valeur qui attend pour `id`.
fn flush(id: &str) {
    let Some(value) = LOCAL.with_borrow_mut(|l| {
        if let Some(t) = l.timers.get(id) {
            t.stop();
        }
        l.waiting.remove(id)
    }) else {
        return;
    };
    let Some((section, key)) = id.split_once('.') else { return };
    // Envoyée : affichée jusqu'à l'écho, au plus `PENDING` à partir de maintenant.
    choose(id, value.clone());
    let app = APP.with_borrow(Clone::clone);
    if let Some(app) = app {
        report(essaim_set_config(app, section.to_owned(), key.to_owned(), value));
    }
}

/// Réglages affichés d'un groupe : valeur choisie, sinon celle d'Essaim ; un réglage dont la
/// valeur manque ou n'est pas du bon type n'est pas montré.
fn rows(list: &[Setting], config: &Value, pending: &HashMap<String, (Value, Instant)>) -> Vec<EssaimPanelSetting> {
    list.iter()
        .filter_map(|s| {
            let id = s.id();
            let v = pending.get(&id).map(|(v, _)| v).or_else(|| config.get(s.section).and_then(|x| x.get(s.key)))?;
            let mut row = EssaimPanelSetting { id: id.as_str().into(), label: s.label.into(), ..Default::default() };
            match (s.range, v) {
                (Some(r), Value::Number(n)) => {
                    let x = n.as_f64()?;
                    row.min = r.min as f32;
                    row.max = r.max as f32;
                    row.step = r.step as f32;
                    row.value = x as f32;
                    row.text = format_value(x, &r).into();
                }
                (None, Value::Bool(b)) => {
                    row.check = true;
                    row.checked = *b;
                }
                _ => return None,
            }
            Some(row)
        })
        .collect()
}

fn show() {
    LOCAL.with_borrow(|l| {
        let e = &l.state;
        let status = super::ui::status(e);
        panel_hosts::each("essaim", |h| {
            let g = h.global::<EssaimPanelData>();
            g.set_status(status);
            g.set_paused(e.paused);
            g.set_gpu(e.gpu);
            g.set_prisme(e.prisme);
            g.set_has_config(e.config.is_object());
        });
        GROUP_MODELS.with(|models| {
            for ((_, list), m) in GROUPS.iter().zip(models) {
                model::sync(m, rows(list, &e.config, &l.pending));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str) -> Range {
        find(id).and_then(|s| s.range).unwrap()
    }

    #[test]
    fn values_are_shown_like_essaim_ts() {
        assert_eq!(format_value(2.8000001, &r("essaim.duree")), "2,8 s");
        assert_eq!(format_value(22.0, &r("orbite.rayon")), "22 px");
        assert_eq!(format_value(0.0, &r("rendu.cadence_repos")), "sans repos");
        assert_eq!(format_value(30.0, &r("rendu.cadence_repos")), "30 im/s");
        assert_eq!(format_value(0.30000001, &r("orbite.trainee")), "0,3");
        assert_eq!(format_value(0.35, &r("orbite.trainee")), "0,35");
        assert_eq!(format_value(3.0, &r("orbite.vitesse_base")), "3");
        assert_eq!(format_value(0.0, &r("orbite.vitesse_base")), "0");
    }

    #[test]
    fn whole_numbers_are_sent_as_integers() {
        assert_eq!(number(22.0), json!(22));
        assert_eq!(number(0.35), json!(0.35));
    }

    #[test]
    fn same_value_ignores_single_precision_noise() {
        assert!(same_value(Some(&json!(0.30000001)), &json!(0.3)));
        assert!(!same_value(Some(&json!(0.31)), &json!(0.3)));
        assert!(same_value(Some(&json!(true)), &json!(true)));
        assert!(!same_value(None, &json!(true)));
    }

    #[test]
    fn settings_need_a_value_of_their_type() {
        let config = json!({ "orbite": { "rayon": 22.0, "vitesse_base": true }, "couleur": { "synchro_prisme": false } });
        let orbit = rows(GROUPS[0].1, &config, &HashMap::new());
        assert_eq!(orbit.len(), 1, "type faux ou valeur absente : pas montré");
        assert_eq!((orbit[0].id.as_str(), orbit[0].text.as_str()), ("orbite.rayon", "22 px"));
        let mut pending = HashMap::new();
        pending.insert("orbite.rayon".to_owned(), (json!(30.0), Instant::now()));
        assert_eq!(rows(GROUPS[0].1, &config, &pending)[0].text.as_str(), "30 px", "valeur choisie d'abord");
        let colour = rows(GROUPS[2].1, &config, &HashMap::new());
        assert!(colour[0].check && !colour[0].checked);
    }
}
