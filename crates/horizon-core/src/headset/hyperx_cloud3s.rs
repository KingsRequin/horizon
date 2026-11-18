//! HyperX Cloud III S Wireless (03F0:06BE, 03F0:02CC) par son dongle, sans NGENUITY.
//!
//! Protocole repris de Halo Battery (`providers/hyperx_cloud3s.py`, licence MIT), différent de
//! celui du Cloud III Wireless. Sources citées par Halo : HyperHeadset (LennardKittner/
//! HyperHeadset, `src/devices/cloud_iii_s_wireless.rs` au commit 57dfa2b : paquets et
//! identifiants produit), et deux captures USBPcap de NGENUITY (HyperHeadset, ticket n° 36 et
//! demande de fusion n° 42) :
//!  * NGENUITY envoie chaque requête en rapport de *sortie* à l'interface 3 (le dongle ignore les
//!    mêmes octets en rapport de fonction) ;
//!  * requête : 64 octets `0c 02 03 01 00 <cmd> 00…` (0x0C = identifiant du rapport ; l'octet 3
//!    à 01 demande une valeur, 00 la réglerait : jamais envoyé) ; commande 0x06 pour la
//!    batterie, 0x48 pour la charge ;
//!  * réponse : rapport d'entrée `0c .. .. .. .. <cmd> <valeur>` ; 0xFF = pas de valeur ;
//!    batterie, le pourcentage ; charge, 0 sur batterie, 1 en charge, 2 chargé ;
//!  * le casque pousse aussi des notifications, rapport 0x0D : octet 4 = 1, batterie en octet 5 ;
//!    octet 4 = 10, état de charge en octet 5 ;
//!  * Windows découpe l'interface 3 en six collections. Halo envoie la requête à la première qui
//!    prend le rapport de sortie 0x0C (pages constructeur d'abord ; Windows refuse un
//!    identifiant que la collection ne déclare pas) et lit la réponse sur toutes.
//!
//! Ici chaque collection constructeur (page 0xFF00 et au-delà) du dongle a son propre pilote et ne
//! lit que ses propres rapports : une collection qui refuse la requête n'a rien à dire (`Off`),
//! celle qui la prend lit la réponse si Windows la lui remet. Une valeur au-delà de 100 (dont
//! 0xFF) est ignorée ; sans réponse, casque éteint (un dongle débranché puis rebranché peut aussi
//! se taire jusqu'au redémarrage de Windows, HyperHeadset n° 36).

use super::hyperx::{product, wait_for, HP_VID};
use super::hyperx_cloud3::cable_state;
use super::{Driver, Family, HidInfo, Link, Status};
use std::time::Duration;

/// Première page d'usage « constructeur ».
const VENDOR_PAGES: u16 = 0xFF00;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x02CC, "HyperX Cloud III S Wireless"), (0x06BE, "HyperX Cloud III S Wireless")];

pub const FAMILY: Family = Family {
    name: "HyperX Cloud III S Wireless",
    vids: &[HP_VID],
    probe: |d| {
        let name = product(PRODUCTS, d)?;
        (d.usage_page >= VENDOR_PAGES).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Cloud3S::default()),
};

const REPORT_ID: u8 = 0x0C;
const NOTIFICATION_ID: u8 = 0x0D;
const PACKET_LEN: usize = 64;
const CMD_BATTERY: u8 = 0x06;
const CMD_CHARGING: u8 = 0x48;
const NOTE_BATTERY: u8 = 1;
const NOTE_CHARGING: u8 = 10;
const READ_MS: i32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Battery,
    Charging,
}

/// Valeur portée par une réponse ou une notification, `None` pour un rapport étranger ou un niveau
/// illisible.
fn parse(r: &[u8]) -> Option<(Kind, u8)> {
    if r.len() < 7 {
        return None;
    }
    match r[0] {
        REPORT_ID => match r[5] {
            CMD_BATTERY => (r[6] <= 100).then_some((Kind::Battery, r[6])),
            CMD_CHARGING => Some((Kind::Charging, r[6])),
            _ => None,
        },
        NOTIFICATION_ID => match r[4] {
            NOTE_BATTERY => (r[5] <= 100).then_some((Kind::Battery, r[5])),
            NOTE_CHARGING => Some((Kind::Charging, r[5])),
            _ => None,
        },
        _ => None,
    }
}

/// Lecteur de batterie du Cloud III S Wireless, pour une collection du dongle.
pub struct Cloud3S {
    /// Attente maximale d'une réponse.
    window: Duration,
}

impl Default for Cloud3S {
    fn default() -> Self {
        Cloud3S { window: Duration::from_millis(1000) }
    }
}

impl Cloud3S {
    /// Valeur demandée par `cmd` ; `None` si la collection refuse la requête ou sans réponse.
    fn ask(&self, link: &mut dyn Link, cmd: u8, want: Kind) -> anyhow::Result<Option<u8>> {
        let mut packet = vec![0; PACKET_LEN];
        packet[..6].copy_from_slice(&[REPORT_ID, 0x02, 0x03, 0x01, 0x00, cmd]);
        if link.write(&packet).is_err() {
            return Ok(None);
        }
        wait_for(link, self.window, READ_MS, |r| match parse(r) {
            Some((kind, value)) if kind == want => Some(value),
            _ => None,
        })
    }
}

impl Driver for Cloud3S {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let Some(level) = self.ask(link, CMD_BATTERY, Kind::Battery)? else { return Ok(Status::Off) };
        // Comme Halo : sans état de charge lisible, le casque n'est pas en charge.
        let charging = self
            .ask(link, CMD_CHARGING, Kind::Charging)
            .ok()
            .flatten()
            .and_then(cable_state)
            .unwrap_or(false);
        Ok(Status::Battery { level, charging })
    }
}

#[cfg(test)]
mod tests {
    use super::super::hyperx::fake::Fake;
    use super::*;

    fn reader() -> Cloud3S {
        Cloud3S { window: Duration::from_millis(30) }
    }

    fn reply(cmd: u8, value: u8) -> Vec<u8> {
        let mut r = vec![0; 64];
        r[..7].copy_from_slice(&[0x0C, 0x02, 0x03, 0x00, 0x00, cmd, value]);
        r
    }

    /// Dongle qui répond `level` et l'état de charge `state`, après un rapport étranger.
    fn dongle(level: u8, state: u8) -> Fake {
        Fake::new(move |w| {
            assert_eq!(w.len(), PACKET_LEN);
            assert_eq!(&w[..5], &[0x0C, 0x02, 0x03, 0x01, 0x00], "requête de lecture seulement");
            let value = match w[5] {
                CMD_BATTERY => level,
                CMD_CHARGING => state,
                _ => return vec![],
            };
            vec![vec![0x0C, 0x02, 0x03, 0x00, 0x00, 0x33, 0x05], reply(w[5], value)]
        })
    }

    #[test]
    fn reads_level_and_charging() {
        let mut dev = dongle(77, 0);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 77, charging: false });
        assert_eq!(dev.writes[0][5], 0x06);
        assert_eq!(dev.writes[1][5], 0x48);
        for (state, charging) in [(1, true), (2, true), (0xFF, false)] {
            assert_eq!(reader().read(&mut dongle(77, state)).unwrap(), Status::Battery { level: 77, charging });
        }
    }

    #[test]
    fn notifications_carry_the_values_too() {
        let mut dev = Fake::new(|w| match w[5] {
            CMD_BATTERY => vec![vec![0x0D, 0, 0, 0, NOTE_BATTERY, 33, 0]],
            _ => vec![vec![0x0D, 0, 0, 0, NOTE_CHARGING, 1, 0]],
        });
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 33, charging: true });
    }

    #[test]
    fn collection_refusing_the_request_has_nothing_to_say() {
        let mut dev = dongle(77, 0);
        dev.write_error = Some("WriteFile: (0x00000057) Paramètre incorrect.");
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn headset_off_without_a_reply() {
        assert_eq!(reader().read(&mut Fake::new(|_| vec![])).unwrap(), Status::Off);
    }

    #[test]
    fn no_value_or_unreadable_level_is_never_shown() {
        assert_eq!(reader().read(&mut dongle(0xFF, 0)).unwrap(), Status::Off);
        assert_eq!(reader().read(&mut dongle(101, 0)).unwrap(), Status::Off);
    }

    #[test]
    fn probe_takes_vendor_collections_only() {
        let d = HidInfo { vid: HP_VID, pid: 0x06BE, usage_page: 0xFF00, usage: 0x0001, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("HyperX Cloud III S Wireless"));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x02CC, usage_page: 0xFFC0, ..d }), Some("HyperX Cloud III S Wireless"));
        for page in [0x000C, 0x000B, 0x0001] {
            assert_eq!((FAMILY.probe)(&HidInfo { usage_page: page, ..d }), None);
        }
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x05B7, ..d }), None);
        assert_eq!(super::super::find(&d).map(|(f, _)| f.name), Some(FAMILY.name));
    }
}
