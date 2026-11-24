//! Widget « Téléphone » de la barre : une pastille par batterie reçue (icône, pourcentage,
//! éclair en charge), dessinée comme celle du casque.

use super::{EV_PHONE, batteries};
use crate::host::AppHandle;
use crate::ui::BarWindow;
use horizon_core::layout::BarLayout;
use horizon_core::phone::battery::BatteryView;
use horizon_core::time::now_ms;
use slint::ComponentHandle;

/// Une batterie telle que la barre la dessine.
pub struct Item {
    pub icon: &'static str,
    pub text: String,
    pub charging: bool,
    /// Couleur d'alerte : même règle que le casque (`headset::is_low`).
    pub low: bool,
    /// Mesure ancienne : estompée.
    pub stale: bool,
    /// « Pixel — 82 %, en charge · il y a 2 min ».
    pub tip: String,
}

/// La batterie `v` pour la barre, à `now_ms`, avec le seuil d'alerte réglé (`None` : coupée).
pub fn item(v: &BatteryView, threshold: Option<u8>, now_ms: u64) -> Item {
    let charge = if v.charging { ", en charge" } else { "" };
    Item {
        icon: v.icon,
        text: format!("{} %", v.percent),
        charging: v.charging,
        low: crate::widgets::headset::is_low(v.percent, v.charging, threshold),
        stale: v.stale,
        tip: format!("{} — {} %{charge} · {}", v.name, v.percent, crate::ui::time::ago(v.at_ms, now_ms)),
    }
}

/// Infobulle du widget : toutes ses batteries.
pub fn title(items: &[Item]) -> String {
    items.iter().map(|i| i.tip.as_str()).collect::<Vec<_>>().join(" ; ")
}

/// Pose les batteries dans une barre (fil de l'interface).
pub fn apply(window: &BarWindow, views: &[BatteryView]) {
    let threshold = crate::state::settings().alerts.phone_threshold();
    let now = now_ms();
    let items: Vec<Item> = views.iter().map(|v| item(v, threshold, now)).collect();
    let rows: Vec<crate::ui::PhoneBattery> = items
        .iter()
        .map(|i| crate::ui::PhoneBattery { icon: i.icon.into(), text: i.text.as_str().into(), charging: i.charging, low: i.low, stale: i.stale })
        .collect();
    let g = window.global::<crate::ui::PhoneData>();
    crate::ui::model::sync_rc(&g.get_items(), rows, |m| g.set_items(m));
    g.set_title(title(&items).into());
}

/// Relie le widget d'une barre au service (fil de l'interface).
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _bar: &BarLayout) {
    apply(window, &batteries(app));
    let w = window.as_weak();
    crate::host::listen(EV_PHONE, Some(label), move |e| {
        if let (Some(views), Some(w)) = (e.payload::<Vec<BatteryView>>(), w.upgrade()) {
            apply(&w, views);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::phone::battery::BatteryView;

    fn view(percent: u8, charging: bool, stale: bool) -> BatteryView {
        BatteryView { key: "t/phone".into(), icon: "deviceMobile", name: "Pixel".into(), percent, charging, stale, at_ms: 0 }
    }

    #[test]
    fn a_view_becomes_a_bar_item() {
        let it = item(&view(14, false, false), Some(15), 120_000);
        assert_eq!((it.icon, it.text.as_str(), it.low, it.tip.as_str()), ("deviceMobile", "14 %", true, "Pixel — 14 % · il y a 2 min"));
        let charging = item(&view(14, true, false), Some(15), 0);
        assert!(!charging.low && charging.charging, "en charge : jamais faible");
        assert_eq!(charging.tip, "Pixel — 14 %, en charge · à l’instant");
        assert!(!item(&view(16, false, false), Some(15), 0).low);
        assert!(item(&view(15, false, false), None, 0).low, "alerte coupée : couleur au seuil par défaut, comme le casque");
        assert!(item(&view(50, false, true), Some(15), 0).stale);
    }

    #[test]
    fn the_widget_tip_lists_every_battery() {
        let items = [item(&view(82, true, false), Some(15), 0), item(&BatteryView { name: "Montre".into(), ..view(64, false, false) }, Some(15), 0)];
        assert_eq!(title(&items), "Pixel — 82 %, en charge · à l’instant ; Montre — 64 % · à l’instant");
        assert_eq!(title(&[]), "");
    }
}
