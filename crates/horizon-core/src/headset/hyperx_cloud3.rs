//! HyperX Cloud III Wireless (03F0:05B7, 03F0:0C9D) et Cloud II Core Wireless (03F0:0995,
//! 03F0:0795) par leur dongle, sans NGENUITY.
//!
//! Protocole repris de Halo Battery (`providers/hyperx_cloud3.py`, licence MIT), lui-même tiré de
//! HyperHeadset (LennardKittner/HyperHeadset, appareil `cloud_iii_wireless`, écrit d'après le
//! trafic de NGENUITY) ; le Cloud II Core Wireless a été reconnu par Halo dans une capture USBPcap
//! de NGENUITY (ticket n° 155 de Halo) :
//!  * collection constructeur 0xFF13/0x0001, à choisir par son usage (la même interface porte
//!    aussi les collections 000c et 000b, qui ne sont pas la batterie) ;
//!  * requête : 62 octets `66 <cmd> 00…` (0x66 = identifiant du rapport) ; commande 0x89 pour
//!    la batterie, 0x8A pour la charge ;
//!  * réponse : 62 octets `66 <écho de la commande ou identifiant de réponse>…` :
//!    batterie (0x89 / 0x0D), `réponse[4]` = pourcentage, seulement si `réponse[2]` ou
//!    `réponse[3]` est non nul (tout à zéro : pas encore de lecture, rendu comme éteint) ;
//!    charge (0x8A / 0x0C), `réponse[2]` = 0 sur batterie, 1 en charge, 2 chargé ;
//!  * sous Windows, certains dongles HyperX ne prennent ces paquets qu'en rapport de fonction :
//!    l'écriture échoue sur « Incorrect function » (ERROR_INVALID_FUNCTION, 0x00000001) et la
//!    requête est renvoyée par `send_feature`, comme le font HyperHeadset et Halo.
//!
//! Une réponse qui ne porte pas la commande attendue est ignorée ; un niveau au-delà de 100 est
//! refusé plutôt qu'affiché. Sans réponse, le dongle est là mais pas le casque : éteint.

use super::hyperx::{product, wait_for, HP_VID};
use super::{Driver, Family, HidInfo, Link, Status};
use std::time::Duration;

const USAGE_PAGE: u16 = 0xFF13;
const USAGE: u16 = 0x0001;
/// Casques connus : identifiant produit et nom affiché. Les deux identifiants du Cloud II Core
/// peuvent répondre en même temps (dongle et second mode) : le premier lu l'emporte.
const PRODUCTS: &[(u16, &str)] = &[
    (0x05B7, "HyperX Cloud III Wireless"),
    (0x0C9D, "HyperX Cloud III Wireless"),
    (0x0995, "HyperX Cloud II Core Wireless"),
    (0x0795, "HyperX Cloud II Core Wireless"),
];

pub const FAMILY: Family = Family {
    name: "HyperX Cloud III Wireless",
    vids: &[HP_VID],
    probe: |d| {
        let name = product(PRODUCTS, d)?;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Cloud3::default()),
};

const REPORT_ID: u8 = 0x66;
const PACKET_LEN: usize = 62;
const CMD_BATTERY: u8 = 0x89;
const RESP_BATTERY: u8 = 0x0D;
const CMD_CHARGING: u8 = 0x8A;
const RESP_CHARGING: u8 = 0x0C;
const READ_MS: i32 = 200;
/// Signes d'un dongle qui ne prend que des rapports de fonction (message de Windows en anglais,
/// ou son code, présent quelle que soit la langue).
const FEATURE_ONLY: [&str; 2] = ["incorrect function", "0x00000001"];

/// Branchement du casque d'après l'état de charge des casques HyperX récents (Cloud III, Cloud
/// III S) : 0 sur batterie, 1 en charge, 2 chargé (toujours sur le câble), autre chose inconnu.
pub(super) fn cable_state(state: u8) -> Option<bool> {
    match state {
        0 => Some(false),
        1 | 2 => Some(true),
        _ => None,
    }
}

/// Lecteur de batterie du Cloud III Wireless.
pub struct Cloud3 {
    /// Attente maximale d'une réponse (Halo : six lectures de 200 ms).
    window: Duration,
}

impl Default for Cloud3 {
    fn default() -> Self {
        Cloud3 { window: Duration::from_millis(1200) }
    }
}

impl Cloud3 {
    /// Première réponse portant l'un des identifiants `expected`, `None` sans réponse.
    fn query(&self, link: &mut dyn Link, cmd: u8, expected: [u8; 2]) -> anyhow::Result<Option<Vec<u8>>> {
        let mut packet = vec![0; PACKET_LEN];
        packet[..2].copy_from_slice(&[REPORT_ID, cmd]);
        if let Err(e) = link.write(&packet) {
            let message = format!("{e:#}").to_lowercase();
            if !FEATURE_ONLY.iter().any(|s| message.contains(s)) {
                return Err(e);
            }
            link.send_feature(&packet)?;
        }
        wait_for(link, self.window, READ_MS, |r| {
            (r.len() > 1 && r[0] == REPORT_ID && expected.contains(&r[1])).then(|| r.to_vec())
        })
    }
}

impl Driver for Cloud3 {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let Some(r) = self.query(link, CMD_BATTERY, [CMD_BATTERY, RESP_BATTERY])? else {
            return Ok(Status::Off);
        };
        if r.len() <= 4 {
            anyhow::bail!("réponse de batterie trop courte : {r:02x?}");
        }
        if r[2] == 0 && r[3] == 0 {
            return Ok(Status::Off);
        }
        let level = r[4];
        if level > 100 {
            anyhow::bail!("niveau de batterie illisible : {r:02x?}");
        }
        // Comme Halo : sans état de charge lisible, le casque n'est pas en charge.
        let charging = self
            .query(link, CMD_CHARGING, [CMD_CHARGING, RESP_CHARGING])
            .ok()
            .flatten()
            .and_then(|r| cable_state(*r.get(2)?))
            .unwrap_or(false);
        Ok(Status::Battery { level, charging })
    }
}

#[cfg(test)]
mod tests {
    use super::super::hyperx::fake::Fake;
    use super::*;

    fn reader() -> Cloud3 {
        Cloud3 { window: Duration::from_millis(30) }
    }

    /// Dongle dont le casque répond par l'identifiant de réponse (0x0D, 0x0C) ; `state` = octet 2
    /// de la réponse de charge, `head` = octets 2 et 3 de la réponse de batterie.
    fn dongle(level: u8, state: u8, head: [u8; 2]) -> Fake {
        Fake::new(move |w| {
            assert_eq!(w.len(), PACKET_LEN);
            let mut r = vec![0; 62];
            r[0] = REPORT_ID;
            match w[1] {
                CMD_BATTERY => {
                    r[1] = RESP_BATTERY;
                    r[2..5].copy_from_slice(&[head[0], head[1], level]);
                }
                CMD_CHARGING => {
                    r[1] = RESP_CHARGING;
                    r[2] = state;
                }
                _ => return vec![],
            }
            // Rapports étrangers (autre identifiant de réponse, autre rapport) avant la réponse.
            vec![vec![REPORT_ID, 0x0A, 1, 2, 3], vec![0x0B, 0x89], r]
        })
    }

    #[test]
    fn reads_level_and_charging() {
        let mut dev = dongle(48, 0, [0x0E, 0xD7]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 48, charging: false });
        assert_eq!(&dev.writes[0][..3], &[0x66, 0x89, 0x00]);
        assert_eq!(&dev.writes[1][..3], &[0x66, 0x8A, 0x00]);
        for (state, charging) in [(1, true), (2, true), (3, false), (0xFF, false)] {
            let mut dev = dongle(48, state, [0x0E, 0xD7]);
            assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 48, charging });
        }
    }

    #[test]
    fn command_echo_is_accepted_as_reply() {
        // Capture du Cloud II Core : `66 89 0e d7 30…` (48 %), `66 8a 00 00` (sur batterie).
        let mut dev = Fake::new(|w| match w[1] {
            CMD_BATTERY => vec![vec![0x66, 0x89, 0x0E, 0xD7, 0x30, 0x00]],
            _ => vec![vec![0x66, 0x8A, 0x00, 0x00]],
        });
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 48, charging: false });
    }

    #[test]
    fn feature_report_when_the_dongle_refuses_output_reports() {
        let mut dev = dongle(70, 1, [1, 0]);
        dev.write_error = Some("WriteFile: (0x00000001) Fonction incorrecte.");
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 70, charging: true });
        assert_eq!(dev.features.len(), 2);
        assert_eq!(&dev.features[0][..2], &[0x66, 0x89]);
    }

    #[test]
    fn other_write_errors_are_errors() {
        let mut dev = dongle(70, 1, [1, 0]);
        dev.write_error = Some("périphérique débranché");
        assert!(reader().read(&mut dev).is_err());
        assert!(dev.features.is_empty());
    }

    #[test]
    fn headset_off_without_a_reply_or_without_a_reading() {
        assert_eq!(reader().read(&mut Fake::new(|_| vec![])).unwrap(), Status::Off);
        assert_eq!(reader().read(&mut dongle(0, 0, [0, 0])).unwrap(), Status::Off);
    }

    #[test]
    fn unreadable_replies_are_refused() {
        assert!(reader().read(&mut dongle(0x9C, 0, [1, 0])).is_err());
        assert!(reader().read(&mut Fake::new(|_| vec![vec![0x66, 0x0D, 1, 0]])).is_err());
    }

    #[test]
    fn probe_takes_the_ff13_0001_collection_only() {
        let d = HidInfo { vid: HP_VID, pid: 0x05B7, usage_page: 0xFF13, usage: 0x0001, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("HyperX Cloud III Wireless"));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0995, ..d }), Some("HyperX Cloud II Core Wireless"));
        for (page, usage) in [(0x000C, 0x0001), (0x000B, 0x0001), (0xFF13, 0xFF00)] {
            assert_eq!((FAMILY.probe)(&HidInfo { usage_page: page, usage, ..d }), None);
        }
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x08BE, ..d }), None);
        assert_eq!(super::super::find(&d).map(|(f, _)| f.name), Some(FAMILY.name));
    }
}
