//! Batteries reçues des téléphones : dernier envoi de chacun, borné (un téléphone ne peut ni
//! faire grossir l'état ni y écrire n'importe quoi) et daté.

use super::proto::{DeviceBattery, DeviceKind};
use super::registry::short_name;
use crate::low_battery;
use serde::{Deserialize, Serialize};

/// Appareils gardés par téléphone ; les suivants d'un envoi sont ignorés.
pub const MAX_DEVICES: usize = 8;
/// Avance tolérée de l'horloge du téléphone ; au-delà, la mesure prend l'heure de réception.
const MAX_AHEAD_MS: u64 = 300_000;

/// Une batterie telle que la barre la montre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BatteryView {
    /// `<téléphone>/<appareil>` : stable d'un envoi à l'autre.
    pub key: String,
    pub icon: &'static str,
    pub name: String,
    pub percent: u8,
    pub charging: bool,
    /// Mesure trop ancienne pour dire quelque chose d'actuel : estompée, sans alerte.
    pub stale: bool,
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Report {
    phone_id: String,
    devices: Vec<DeviceBattery>,
}

/// Dernier envoi de chaque téléphone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Book {
    reports: Vec<Report>,
}

fn icon(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Phone => "deviceMobile",
        DeviceKind::Watch => "watch",
        DeviceKind::Earbuds => "headphones",
        DeviceKind::Other => "batteryFull",
    }
}

fn rank(kind: DeviceKind) -> u8 {
    match kind {
        DeviceKind::Phone => 0,
        DeviceKind::Watch => 1,
        DeviceKind::Earbuds => 2,
        DeviceKind::Other => 3,
    }
}

/// Mesure ramenée dans les bornes : pourcentage, longueurs, heure.
fn bounded(d: DeviceBattery, now_ms: u64) -> DeviceBattery {
    DeviceBattery {
        id: short_name(&d.id),
        name: short_name(&d.name),
        percent: d.percent.min(100),
        at_ms: if d.at_ms > now_ms.saturating_add(MAX_AHEAD_MS) { now_ms } else { d.at_ms },
        ..d
    }
}

impl Book {
    /// Envoi du téléphone `phone_id`, reçu à `now_ms` : il remplace le précédent. Vrai si
    /// quelque chose a changé.
    pub fn apply(&mut self, phone_id: &str, devices: Vec<DeviceBattery>, now_ms: u64) -> bool {
        let devices: Vec<_> = devices.into_iter().take(MAX_DEVICES).map(|d| bounded(d, now_ms)).collect();
        match self.reports.iter_mut().find(|r| r.phone_id == phone_id) {
            Some(r) if r.devices == devices => false,
            Some(r) => {
                r.devices = devices;
                true
            }
            None if devices.is_empty() => false,
            None => {
                self.reports.push(Report { phone_id: phone_id.to_owned(), devices });
                true
            }
        }
    }

    /// Oublie les batteries du téléphone `phone_id` ; faux s'il n'en avait pas.
    pub fn forget(&mut self, phone_id: &str) -> bool {
        let before = self.reports.len();
        self.reports.retain(|r| r.phone_id != phone_id);
        self.reports.len() != before
    }

    /// Batteries à montrer à `now_ms` : téléphones, montres, écouteurs, puis le reste.
    pub fn view(&self, now_ms: u64) -> Vec<BatteryView> {
        let mut all: Vec<(u8, BatteryView)> = self
            .reports
            .iter()
            .flat_map(|r| r.devices.iter().map(move |d| (r, d)))
            .map(|(r, d)| {
                let view = BatteryView {
                    key: format!("{}/{}", r.phone_id, d.id),
                    icon: icon(d.kind),
                    name: d.name.clone(),
                    percent: d.percent,
                    charging: d.charging,
                    stale: !low_battery::recent(d.at_ms as i64, now_ms as i64),
                    at_ms: d.at_ms,
                };
                (rank(d.kind), view)
            })
            .collect();
        all.sort_by(|a, b| (a.0, &a.1.key).cmp(&(b.0, &b.1.key)));
        all.into_iter().map(|(_, v)| v).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, kind: DeviceKind, percent: u8, charging: bool, at_ms: u64) -> DeviceBattery {
        DeviceBattery { id: id.into(), kind, name: id.into(), percent, charging, at_ms }
    }

    #[test]
    fn a_report_replaces_the_previous_one_of_the_same_phone() {
        let mut b = Book::default();
        assert!(b.apply("t1", vec![dev("phone", DeviceKind::Phone, 82, true, 1_000)], 1_000));
        assert!(!b.apply("t1", vec![dev("phone", DeviceKind::Phone, 82, true, 1_000)], 2_000), "rien de neuf");
        assert!(b.apply("t1", vec![dev("phone", DeviceKind::Phone, 81, false, 3_000), dev("w", DeviceKind::Watch, 64, false, 3_000)], 3_000));
        let v = b.view(3_000);
        assert_eq!(v.iter().map(|x| (x.icon, x.percent, x.charging)).collect::<Vec<_>>(), vec![("deviceMobile", 81, false), ("watch", 64, false)]);
        assert_eq!(v[0].key, "t1/phone");
        assert!(b.forget("t1") && b.view(3_000).is_empty());
        assert!(!b.forget("t1"));
    }

    #[test]
    fn hostile_or_broken_reports_are_bounded() {
        let mut b = Book::default();
        let many: Vec<_> = (0..500).map(|i| dev(&format!("d{i}"), DeviceKind::Other, 250, false, 1)).collect();
        b.apply("t1", many, 1_000);
        let v = b.view(1_000);
        assert_eq!(v.len(), MAX_DEVICES);
        assert!(v.iter().all(|x| x.percent == 100));
        let mut long = dev("phone", DeviceKind::Phone, 50, false, 1_000);
        long.name = "n".repeat(10_000);
        long.id = "i".repeat(10_000);
        b.apply("t2", vec![long], 1_000);
        assert!(b.view(1_000).iter().all(|x| x.name.chars().count() <= 40 && x.key.chars().count() <= 90));
    }

    #[test]
    fn an_old_or_future_measure_is_handled() {
        let mut b = Book::default();
        b.apply("t1", vec![dev("phone", DeviceKind::Phone, 50, false, 0)], 3 * 3_600_000);
        assert!(b.view(3 * 3_600_000)[0].stale, "plus de deux heures");
        b.apply("t1", vec![dev("phone", DeviceKind::Phone, 50, false, 9_999_999_999_999)], 1_000);
        assert_eq!(b.view(1_000)[0].at_ms, 1_000, "horloge du téléphone en avance : heure de réception");
        assert!(!b.view(1_000)[0].stale);
    }

    #[test]
    fn two_phones_keep_their_own_devices_in_a_stable_order() {
        let mut b = Book::default();
        b.apply("t2", vec![dev("buds", DeviceKind::Earbuds, 40, false, 1), dev("phone", DeviceKind::Phone, 30, false, 1)], 1);
        b.apply("t1", vec![dev("phone", DeviceKind::Phone, 90, false, 1)], 1);
        let keys: Vec<_> = b.view(1).into_iter().map(|x| x.key).collect();
        assert_eq!(keys, ["t1/phone", "t2/phone", "t2/buds"], "téléphones d'abord, puis par clé");
        let back: Book = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(back.view(1), b.view(1), "le carnet se relit tel quel");
    }
}
