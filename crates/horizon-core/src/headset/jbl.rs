//! JBL Quantum 910 Wireless par son récepteur USB, sans JBL QuantumENGINE.
//!
//! Protocole repris de Halo Battery (`providers/jbl.py`, licence MIT), lui-même tiré de
//! plugato/JBL_Baterry_Monitor (lecture sous Linux du même identifiant 0ECB:2088) :
//!  * collection constructeur 0xFF13/0x0001 du récepteur ; aucune requête : le casque pousse
//!    lui-même ses rapports ;
//!  * rapport 0x08 : niveau de batterie dans l'octet 1 (`08 1e` = 30 %) ;
//!  * rapport 0x09 : casque allumé (octet 1 non nul) ou éteint ;
//!  * rapport 0x2F : état du micro, pas un niveau ;
//!  * le casque peut se taire longtemps : le dernier niveau entendu est gardé ;
//!  * pas d'indication de charge.
//!
//! Rien n'est envoyé au casque. Écart avec Halo : Halo écoute en permanence dans un fil à lui ;
//! ici, la collection n'est écoutée que pendant chaque lecture (une seconde au plus), si bien
//! qu'un rapport poussé entre deux lectures est perdu. Halo grise le dernier niveau quand rien
//! de neuf n'arrive ; `Status` n'a pas d'état « ancien », il est rendu tel quel.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x0ECB;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x2088, "JBL Quantum 910 Wireless")];
const USAGE_PAGE: u16 = 0xFF13;
const USAGE: u16 = 0x0001;

pub const FAMILY: Family = Family {
    name: "JBL Quantum",
    vids: &[VID],
    probe: |d| {
        let name = PRODUCTS.iter().find(|(pid, _)| d.vid == VID && *pid == d.pid)?.1;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Quantum::default()),
};

const REPORT_BATTERY: u8 = 0x08;
const REPORT_POWER: u8 = 0x09;
const READ_MS: i32 = 250;

fn parse_level(r: &[u8]) -> Option<u8> {
    if r.first() != Some(&REPORT_BATTERY) {
        return None;
    }
    r.get(1).copied().filter(|&l| l <= 100)
}

/// Écoute du récepteur ; garde le dernier niveau et le dernier état d'alimentation entendus.
pub struct Quantum {
    /// Écoute maximale par lecture.
    window: Duration,
    last: Option<u8>,
    /// Le dernier rapport d'alimentation disait « éteint ».
    off: bool,
}

impl Default for Quantum {
    fn default() -> Self {
        Quantum { window: Duration::from_secs(1), last: None, off: false }
    }
}

impl Driver for Quantum {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let end = Instant::now() + self.window;
        while Instant::now() < end {
            let r = link.read(READ_MS)?;
            if let Some(level) = parse_level(&r) {
                self.last = Some(level);
                self.off = false;
                return Ok(Status::Battery { level, charging: false });
            }
            if r.first() == Some(&REPORT_POWER)
                && let Some(&on) = r.get(1)
            {
                self.off = on == 0;
            }
        }
        match self.last {
            Some(level) if !self.off => Ok(Status::Battery { level, charging: false }),
            _ => Ok(Status::Off),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux récepteur : rend les rapports poussés, puis rien.
    struct Fake(VecDeque<Vec<u8>>);

    impl Link for Fake {
        fn write(&mut self, _: &[u8]) -> anyhow::Result<()> {
            panic!("rien ne doit être envoyé au casque")
        }
        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.0.pop_front().unwrap_or_default())
        }
    }

    fn fake(reports: &[&[u8]]) -> Fake {
        Fake(reports.iter().map(|r| r.to_vec()).collect())
    }

    fn reader() -> Quantum {
        Quantum { window: Duration::from_millis(20), ..Quantum::default() }
    }

    #[test]
    fn probe_takes_the_vendor_collection_only() {
        let d = HidInfo { vid: VID, pid: 0x2088, usage_page: USAGE_PAGE, usage: USAGE, interface: 5 };
        assert_eq!((FAMILY.probe)(&d), Some("JBL Quantum 910 Wireless"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 2, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x2089, ..d }), None);
    }

    #[test]
    fn battery_report_gives_the_level() {
        let mut dev = fake(&[&[0x2F, 0x01], &[0x08, 0x1E, 0x00]]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 30, charging: false });
    }

    #[test]
    fn last_level_is_kept_while_the_headset_is_quiet() {
        let mut q = reader();
        q.read(&mut fake(&[&[0x08, 0x40]])).unwrap();
        assert_eq!(q.read(&mut fake(&[])).unwrap(), Status::Battery { level: 64, charging: false });
    }

    #[test]
    fn power_off_report_means_off() {
        let mut q = reader();
        q.read(&mut fake(&[&[0x08, 0x40]])).unwrap();
        assert_eq!(q.read(&mut fake(&[&[0x09, 0x00]])).unwrap(), Status::Off);
        assert_eq!(q.read(&mut fake(&[&[0x09, 0x01]])).unwrap(), Status::Battery { level: 64, charging: false });
    }

    #[test]
    fn nothing_heard_yet_is_off() {
        assert_eq!(reader().read(&mut fake(&[])).unwrap(), Status::Off);
    }

    #[test]
    fn other_and_unreadable_reports_are_ignored() {
        let mut dev = fake(&[&[0x2F, 0x30], &[0x08], &[0x08, 0xC8], &[0x07, 0x30]]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
        assert_eq!(parse_level(&[0x08, 100]), Some(100));
        assert_eq!(parse_level(&[0x08, 101]), None);
    }
}
