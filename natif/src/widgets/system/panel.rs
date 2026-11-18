//! Panneau Système (global `SystemPanelData` de `ui/widgets/system/panel.slint`) : valeurs,
//! températures, mémoire utilisée, courbes et processus gourmands. Le fil des mesures envoie une
//! mesure complète chaque seconde tant que le panneau est ouvert (`PopupState::is_open`) ; à
//! l'ouverture, la dernière connue est montrée aussitôt (`send_last_to_panel` de la version
//! Tauri). Fil de l'interface.

use super::{EV_SYSTEM, SystemFeed, SystemPayload, spark_sized};
use crate::host::{self, AppHandle, Manager};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{SystemPanelData, SystemPanelMeasure, SystemPanelProcess};
use crate::util::lock;
use crate::widgets::Panel;
use slint::{ModelRc, VecModel};
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel { opened: Some(opened), closed: None, ..Panel::new(bind) };

/// Taille d'une courbe du panneau (`<Sparkline width={64} height={18}>`), px logiques.
const SPARK_W: f32 = 64.0;
const SPARK_H: f32 = 18.0;

thread_local! {
    static TOP: Rc<VecModel<SystemPanelProcess>> = Rc::new(VecModel::default());
}

/// « 12 % », « — » (inconnu).
fn pct(v: Option<f32>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{} %", v.round() as i64))
}

/// « 54 °C », « — ».
fn temp(v: Option<f32>) -> String {
    v.map_or_else(|| "—".to_owned(), |v| format!("{} °C", v.round() as i64))
}

/// Nombre à une décimale, virgule française (« 3,2 » ; `toFixed(1)` : moitié vers le haut).
fn one_decimal(v: f64) -> String {
    format!("{:.1}", (v * 10.0).round() / 10.0).replace('.', ",")
}

/// Octets en gigaoctets (« 9,8 »).
fn gb(b: u64) -> String {
    one_decimal(b as f64 / 1024f64.powi(3))
}

fn measure(value: String, detail: String, history: &[f32]) -> SystemPanelMeasure {
    SystemPanelMeasure { value: value.into(), detail: detail.into(), spark: spark_sized(history, SPARK_W, SPARK_H) }
}

fn show(p: &SystemPayload) {
    let s = &p.sample;
    let top: Vec<SystemPanelProcess> = s
        .top
        .iter()
        .map(|t| SystemPanelProcess { name: t.name.as_str().into(), cpu: format!("{} %", one_decimal(t.cpu as f64)).into() })
        .collect();
    TOP.with(|m| crate::ui::model::sync(m, top));
    // Pas de température du processeur : Windows ne la donne pas sans pilote noyau.
    let cpu = measure(pct(Some(s.cpu)), "—".into(), &p.history.cpu);
    let gpu = measure(pct(s.gpu), temp(s.gpu_temp), &p.history.gpu);
    let ram = measure(pct(Some(p.ram_pct)), format!("{} / {} Go", gb(s.ram_used), gb(s.ram_total)), &p.history.ram);
    panel_hosts::each("system", |h| {
        let g = h.global::<SystemPanelData>();
        g.set_cpu(cpu.clone());
        g.set_gpu(gpu.clone());
        g.set_ram(ram.clone());
    });
}

fn bind(_app: &AppHandle, win: &Host) {
    win.global::<SystemPanelData>().set_top(ModelRc::from(TOP.with(Rc::clone)));
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_SYSTEM, Some(LABEL), |e| {
            if let Some(p) = e.payload::<SystemPayload>() {
                show(p);
            }
        });
    }
}

/// Ouverture : la dernière mesure connue tout de suite, sans attendre la suivante.
fn opened(app: &AppHandle, _kind: &str) {
    if let Some(p) = lock(&app.state::<SystemFeed>().last).clone() {
        show(&p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_follow_panel_svelte() {
        assert_eq!(pct(Some(12.5)), "13 %");
        assert_eq!(pct(None), "—");
        assert_eq!(temp(Some(54.4)), "54 °C");
        assert_eq!(temp(None), "—");
        assert_eq!(gb(10_522_669_875), "9,8");
        assert_eq!(gb(0), "0,0");
        assert_eq!(one_decimal(3.25), "3,3");
        assert_eq!(one_decimal(0.04), "0,0");
    }
}
