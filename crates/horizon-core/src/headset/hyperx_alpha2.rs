//! HyperX Cloud Alpha 2 (03F0:08BE) par sa station 2,4 GHz, sans NGENUITY.
//!
//! Protocole repris de Halo Battery (`providers/hyperx_alpha2.py`, licence MIT), décodé par Halo
//! dans deux captures USBPcap de NGENUITY (ticket n° 26 de Halo) prises à deux niveaux de charge :
//!  * la station a deux fonctions USB : 03F0:08BE (« Wireless Controller », qui porte la
//!    batterie) et 03F0:0ABE (« Wireless Chat », audio seulement, rien à y lire) ;
//!  * collection constructeur 0xFF13/0xFF00 ;
//!  * requête : rapport de sortie de 64 octets `50 02 00…` (0x50 = identifiant du rapport) ;
//!  * réponse : rapport d'entrée `51 02 <niveau %> 00 1a 00 <drapeaux>…` ; le bit 7 des
//!    drapeaux (octet 6) est mis en charge.
//!
//! La trame `61 02` n'est pas la batterie (identique aux deux niveaux). NGENUITY fait passer des
//! milliers de trames `ff 01`, `44`, `45` sur ce canal : les rapports en attente sont vidés avant
//! la requête, puis seule une réponse `51 02` est retenue (un niveau au-delà de 100 est ignoré,
//! comme chez Halo). Sans réponse dans le délai : casque éteint.

use super::hyperx::{product, wait_for, HP_VID};
use super::{Driver, Family, HidInfo, Link, Status};
use std::time::Duration;

const USAGE_PAGE: u16 = 0xFF13;
const USAGE: u16 = 0xFF00;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x08BE, "HyperX Cloud Alpha 2")];

pub const FAMILY: Family = Family {
    name: "HyperX Cloud Alpha 2",
    vids: &[HP_VID],
    probe: |d| {
        let name = product(PRODUCTS, d)?;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Alpha2::default()),
};

const REPORT_REQUEST: u8 = 0x50;
const REPORT_REPLY: u8 = 0x51;
const SUBCMD_BATTERY: u8 = 0x02;
const REQUEST_LEN: usize = 64;
const LEVEL_INDEX: usize = 2;
const FLAGS_INDEX: usize = 6;
const CHARGING_BIT: u8 = 0x80;
/// Vidage avant la requête : au plus tant de rapports, chacun attendu tant de millisecondes.
const DRAIN_MAX: usize = 64;
const DRAIN_MS: i32 = 5;
const READ_MS: i32 = 150;

/// Niveau et charge d'une réponse `51 02`, `None` pour tout autre rapport ou un niveau illisible.
fn parse(r: &[u8]) -> Option<(u8, bool)> {
    if r.len() <= FLAGS_INDEX || r[0] != REPORT_REPLY || r[1] != SUBCMD_BATTERY {
        return None;
    }
    let level = r[LEVEL_INDEX];
    (level <= 100).then_some((level, r[FLAGS_INDEX] & CHARGING_BIT != 0))
}

/// Lecteur de batterie de l'Alpha 2.
pub struct Alpha2 {
    /// Attente maximale de la réponse.
    window: Duration,
}

impl Default for Alpha2 {
    fn default() -> Self {
        Alpha2 { window: Duration::from_millis(1500) }
    }
}

impl Driver for Alpha2 {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        for _ in 0..DRAIN_MAX {
            if link.read(DRAIN_MS)?.is_empty() {
                break;
            }
        }
        let mut req = vec![0; REQUEST_LEN];
        req[..2].copy_from_slice(&[REPORT_REQUEST, SUBCMD_BATTERY]);
        link.write(&req)?;
        Ok(match wait_for(link, self.window, READ_MS, parse)? {
            Some((level, charging)) => Status::Battery { level, charging },
            None => Status::Off,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::hyperx::fake::Fake;
    use super::*;

    fn reader() -> Alpha2 {
        Alpha2 { window: Duration::from_millis(30) }
    }

    /// Station dont le casque répond `51 02 <level> 00 1a 00 <flags>`, précédé du bruit de NGENUITY.
    fn station(level: u8, flags: u8) -> Fake {
        Fake::new(move |w| {
            assert_eq!(w.len(), REQUEST_LEN);
            assert_eq!(&w[..3], &[0x50, 0x02, 0x00]);
            let mut r = vec![0; 64];
            r[..7].copy_from_slice(&[0x51, 0x02, level, 0x00, 0x1A, 0x00, flags]);
            vec![vec![0xFF, 0x01], vec![0x61, 0x02, 0x10, 0x20, 0x30, 0x40, 0x80], vec![0x44, 0x00], r]
        })
    }

    #[test]
    fn reads_level_from_the_51_02_reply() {
        // Valeurs des deux captures de Halo : 0x33 = 51 %, 0x43 = 67 %.
        assert_eq!(reader().read(&mut station(0x33, 0x00)).unwrap(), Status::Battery { level: 51, charging: false });
        assert_eq!(reader().read(&mut station(0x43, 0x00)).unwrap(), Status::Battery { level: 67, charging: false });
    }

    #[test]
    fn bit_7_of_the_flags_is_charging() {
        assert_eq!(reader().read(&mut station(80, 0x80)).unwrap(), Status::Battery { level: 80, charging: true });
        assert_eq!(reader().read(&mut station(80, 0x7F)).unwrap(), Status::Battery { level: 80, charging: false });
    }

    #[test]
    fn stale_reports_are_drained_before_the_request() {
        let mut dev = station(42, 0);
        // Une vieille réponse `51 02` en attente ne doit pas passer pour la nouvelle.
        dev.queue.extend([vec![0x51, 0x02, 99, 0, 0x1A, 0, 0], vec![0xFF, 0x01]]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 42, charging: false });
    }

    #[test]
    fn headset_off_without_a_reply() {
        assert_eq!(reader().read(&mut Fake::new(|_| vec![vec![0xFF, 0x01]])).unwrap(), Status::Off);
    }

    #[test]
    fn unreadable_level_is_never_shown() {
        assert_eq!(reader().read(&mut station(0xC8, 0)).unwrap(), Status::Off);
    }

    #[test]
    fn probe_takes_the_ff13_ff00_collection_only() {
        let d = HidInfo { vid: HP_VID, pid: 0x08BE, usage_page: 0xFF13, usage: 0xFF00, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("HyperX Cloud Alpha 2"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 0x0001, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, usage: 0x0001, ..d }), None);
        // La moitié « Wireless Chat » de la station n'a rien à lire.
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0ABE, ..d }), None);
        assert_eq!(super::super::find(&d).map(|(f, _)| f.name), Some(FAMILY.name));
    }
}
