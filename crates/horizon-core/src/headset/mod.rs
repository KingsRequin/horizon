//! Batterie des casques sans fil, lue directement par USB (HID), sans le logiciel du fabricant.
//!
//! Protocoles repris de Halo Battery (licence MIT), lui-même appuyé sur HeadsetControl, Solaar,
//! OpenRazer et d'autres projets cités dans chaque module. Un module par famille de casques :
//! sa `FAMILY` dit quelles collections HID il sait lire (`probe`) et crée son pilote (`new`).
//! L'énumération et l'ouverture des collections sont dans `horizon_sys::headset`.
//!
//! Règle commune : seules des requêtes de lecture sont envoyées (jamais de réglage du casque),
//! et un pilote ne garde que les réponses qui sont les siennes (un logiciel du fabricant peut
//! interroger le casque en même temps).

mod astro;
mod audeze;
pub mod bluetooth;
mod centurion;
mod corsair;
mod hyperx;
mod hyperx_alpha2;
mod hyperx_cloud3;
mod hyperx_cloud3s;
mod jbl;
mod logitech;
mod razer;
mod razer_barracuda;
mod razer_blackshark;
mod razer_blackshark2020;
mod steelseries;
mod steelseries_elite;

/// Familles connues, dans l'ordre d'essai : les protocoles propres à un modèle avant les
/// protocoles généraux d'un fabricant (Centurion avant HID++, Barracuda avant Razer…).
pub const FAMILIES: &[Family] = &[
    centurion::FAMILY,
    astro::FAMILY,
    logitech::FAMILY,
    steelseries_elite::FAMILY,
    steelseries::FAMILY,
    hyperx_alpha2::FAMILY,
    hyperx_cloud3s::FAMILY,
    hyperx_cloud3::FAMILY,
    hyperx::FAMILY,
    razer_barracuda::FAMILY,
    razer_blackshark::FAMILY,
    razer_blackshark2020::FAMILY,
    razer::FAMILY,
    corsair::FAMILY,
    audeze::FAMILY,
    jbl::FAMILY,
];

/// Lien vers une collection HID ouverte.
pub trait Link {
    /// Rapport de sortie ; le premier octet est l'identifiant du rapport (0 s'il n'y en a pas).
    fn write(&mut self, report: &[u8]) -> anyhow::Result<()>;
    /// Prochain rapport d'entrée tel que hidapi le rend (identifiant en tête pour les rapports
    /// numérotés), vide après `timeout_ms` sans rien.
    fn read(&mut self, timeout_ms: i32) -> anyhow::Result<Vec<u8>>;
    /// Rapport de fonction (« feature ») envoyé ; premier octet = identifiant.
    fn send_feature(&mut self, _report: &[u8]) -> anyhow::Result<()> {
        anyhow::bail!("rapport de fonction non pris en charge")
    }
    /// Rapport de fonction lu : `len` octets au plus, identifiant `report_id` en tête.
    fn get_feature(&mut self, _report_id: u8, _len: usize) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("rapport de fonction non pris en charge")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Battery { level: u8, charging: bool },
    /// Le récepteur (ou la base) répond, pas le casque : éteint ou hors de portée.
    Off,
}

/// Lecteur de batterie d'un casque ; garde son état (index découverts…) entre deux lectures.
pub trait Driver: Send {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status>;
    /// Reconnaît, parmi les rapports d'entrée reçus sans requête, ceux qui annoncent un
    /// changement d'état (casque allumé ou éteint) : la collection est alors écoutée en continu
    /// et chaque avis provoque une relecture immédiate. `None` : relectures périodiques seules.
    fn notice(&self) -> Option<fn(&[u8]) -> bool> {
        None
    }
}

/// Collection HID énumérée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidInfo {
    pub vid: u16,
    pub pid: u16,
    pub usage_page: u16,
    pub usage: u16,
    /// Numéro d'interface USB (-1 si inconnu, Bluetooth par exemple).
    pub interface: i32,
}

/// Famille de casques parlant le même protocole.
pub struct Family {
    /// Nom pour le journal.
    pub name: &'static str,
    /// Fabricants à énumérer (l'énumération ne porte que sur eux).
    pub vids: &'static [u16],
    /// Nom affiché du casque si cette collection est celle à ouvrir pour le lire.
    pub probe: fn(&HidInfo) -> Option<&'static str>,
    pub new: fn(&HidInfo) -> Box<dyn Driver>,
}

/// Famille qui lit cette collection, et nom du casque.
pub fn find(d: &HidInfo) -> Option<(&'static Family, &'static str)> {
    FAMILIES.iter().find_map(|f| Some((f, (f.probe)(d)?)))
}

/// Fabricants à énumérer, sans doublon.
pub fn vids() -> Vec<u16> {
    let mut v: Vec<u16> = FAMILIES.iter().flat_map(|f| f.vids.iter().copied()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_lists_its_vendors() {
        for f in FAMILIES {
            assert!(!f.vids.is_empty(), "{} : aucun fabricant", f.name);
        }
        let mut names: Vec<_> = FAMILIES.iter().map(|f| f.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), FAMILIES.len(), "noms de familles en double");
    }

    #[test]
    fn centurion_is_found_on_its_vendor_collection_only() {
        let d = HidInfo { vid: 0x046D, pid: 0x0AF7, usage_page: 0xFFA0, usage: 1, interface: 3 };
        assert_eq!(find(&d).map(|(f, n)| (f.name, n)), Some(("Logitech Centurion", "Logitech PRO X 2 LIGHTSPEED")));
        assert!(find(&HidInfo { usage_page: 0x000C, ..d }).is_none());
    }
}
