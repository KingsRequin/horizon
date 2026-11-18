//! SteelSeries Arctis Nova Elite (base 1038:2244) par USB, sans SteelSeries GG.
//!
//! Protocole repris de Halo Battery (`providers/steelseries_elite.py`, licence MIT), lui-même
//! tiré de elegos/Linux-Arctis-Manager (`devices/nova_elite.yaml`, relevé USB de SteelSeries GG),
//! de loteran/Arctis-Sound-Manager (`devices/nova_elite.yaml`, disposition de la réponse directe
//! d'après la description de la base par SteelSeries GG) et d'un relevé sur une vraie base
//! (Halo Battery #138) :
//!  * interface 3, collection constructeur 0xFFC0/0x0001 (rapport 01 sous Windows) ;
//!  * requête : rapport de sortie de 64 octets `01 b0 00 …` ;
//!  * réponse directe `01 b0 … <niveau, octet 6> … <alimentation, octet 14> <charge, octet 15>`
//!    (relevé #138 : `01 b0 00 00 01 00 1f 64 …` pour 31 % dans SteelSeries GG) ;
//!  * trames que la base peut aussi envoyer : `07 b7 <niveau> <batterie de rechange> <charge>`
//!    et `07 b5 <?> <bluetooth> <alimentation>` ;
//!  * alimentation : 01 = casque hors ligne, 02 = en charge sur le câble, 04 = veille,
//!    08 = en ligne ; charge : 02 = en charge, 08 = sur batterie.
//!
//! Un niveau au-delà de 100 est refusé plutôt qu'affiché. Un casque éteint s'affiche à 0 % sur
//! cette base (comme dans SteelSeries GG) : l'état « hors ligne » n'arrive pas toujours.
//! Seule la requête de lecture `01 b0` est envoyée.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x1038;
const PID: u16 = 0x2244;
const NAME: &str = "SteelSeries Arctis Nova Elite";
const INTERFACE: i32 = 3;
const USAGE_PAGE: u16 = 0xFFC0;
const USAGE: u16 = 0x0001;

pub const FAMILY: Family = Family {
    name: "SteelSeries Arctis Nova Elite",
    vids: &[VID],
    probe: |d| {
        (d.vid == VID && d.pid == PID && d.interface == INTERFACE && d.usage_page == USAGE_PAGE && d.usage == USAGE)
            .then_some(NAME)
    },
    new: |_: &HidInfo| Box::new(Elite { window: Duration::from_secs(1) }),
};

const PACKET: usize = 64;
/// Identifiant du rapport de la requête et de la réponse directe.
const REPLY: u8 = 0x01;
const STATUS: u8 = 0xB0;
/// Positions dans la réponse directe.
const REPLY_LEVEL: usize = 6;
const REPLY_POWER: usize = 14;
const REPLY_CHARGING: usize = 15;
/// Trames spontanées de la base.
const FRAME: u8 = 0x07;
const BATTERY: u8 = 0xB7;
const POWER: u8 = 0xB5;
const CHARGING: u8 = 0x02;
const POWER_OFFLINE: u8 = 0x01;
const POWER_CABLE: u8 = 0x02;
const READ_MS: i32 = 100;

/// Réponse directe `01 b0` : (niveau s'il est lisible, en charge, alimentation).
fn parse_reply(r: &[u8]) -> Option<(Option<u8>, bool, u8)> {
    if r.len() <= REPLY_CHARGING || r[0] != REPLY || r[1] != STATUS {
        return None;
    }
    let level = Some(r[REPLY_LEVEL]).filter(|&l| l <= 100);
    Some((level, r[REPLY_CHARGING] == CHARGING, r[REPLY_POWER]))
}

#[derive(Debug, PartialEq, Eq)]
enum Frame {
    Battery { level: u8, charging: bool },
    Power(u8),
}

/// Trame `07 b7` ou `07 b5` ; les autres (réglages, niveau au-delà de 100) sont ignorées.
fn parse_frame(r: &[u8]) -> Option<Frame> {
    if r.len() < 5 || r[0] != FRAME {
        return None;
    }
    match r[1] {
        BATTERY if r[2] <= 100 => Some(Frame::Battery { level: r[2], charging: r[4] == CHARGING }),
        POWER => Some(Frame::Power(r[4])),
        _ => None,
    }
}

/// Lecteur de batterie de la base Nova Elite.
pub struct Elite {
    /// Attente maximale du niveau et de l'alimentation.
    window: Duration,
}

impl Driver for Elite {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let mut request = [0u8; PACKET];
        request[0] = REPLY;
        request[1] = STATUS;
        link.write(&request)?;
        let mut level: Option<(u8, bool)> = None;
        let mut power: Option<u8> = None;
        let end = Instant::now() + self.window;
        while Instant::now() < end && (level.is_none() || power.is_none()) {
            let r = link.read(READ_MS)?;
            if r.is_empty() {
                continue;
            }
            if let Some((l, charging, p)) = parse_reply(&r) {
                if let Some(l) = l {
                    level = Some((l, charging));
                }
                power = Some(p);
                continue;
            }
            match parse_frame(&r) {
                Some(Frame::Battery { level: l, charging }) => level = Some((l, charging)),
                Some(Frame::Power(p)) => {
                    power = Some(p);
                    if p == POWER_OFFLINE {
                        break;
                    }
                }
                None => {}
            }
        }
        if power == Some(POWER_OFFLINE) {
            return Ok(Status::Off);
        }
        let (level, charging) =
            level.ok_or_else(|| anyhow::anyhow!("aucun niveau de batterie (réponse 01 b0 ou trame 07 b7)"))?;
        Ok(Status::Battery { level, charging: charging || power == Some(POWER_CABLE) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Fausse base : la requête met en file les rapports donnés.
    struct Fake {
        replies: Vec<Vec<u8>>,
        queue: VecDeque<Vec<u8>>,
        writes: Vec<Vec<u8>>,
    }

    impl Fake {
        fn new(replies: Vec<Vec<u8>>) -> Fake {
            Fake { replies, queue: VecDeque::new(), writes: Vec::new() }
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            self.writes.push(w.to_vec());
            self.queue.extend(self.replies.drain(..));
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    /// Réponse directe, sur le modèle du relevé #138.
    fn reply(level: u8, power: u8, charging: u8) -> Vec<u8> {
        let mut r = vec![0u8; PACKET];
        r[..8].copy_from_slice(&[0x01, 0xB0, 0x00, 0x00, 0x01, 0x00, level, 0x64]);
        r[REPLY_POWER] = power;
        r[REPLY_CHARGING] = charging;
        r
    }

    fn read(replies: Vec<Vec<u8>>) -> anyhow::Result<Status> {
        Elite { window: Duration::from_millis(30) }.read(&mut Fake::new(replies))
    }

    #[test]
    fn request_is_a_64_byte_01_b0_report() {
        let mut dev = Fake::new(vec![reply(31, 0x08, 0x08)]);
        Elite { window: Duration::from_millis(30) }.read(&mut dev).unwrap();
        assert_eq!(dev.writes.len(), 1);
        assert_eq!(dev.writes[0].len(), 64);
        assert_eq!(&dev.writes[0][..2], &[0x01, 0xB0]);
        assert!(dev.writes[0][2..].iter().all(|&b| b == 0));
    }

    #[test]
    fn level_from_the_direct_reply() {
        assert_eq!(read(vec![reply(31, 0x08, 0x08)]).unwrap(), Status::Battery { level: 31, charging: false });
    }

    #[test]
    fn charging_from_the_charge_byte_or_the_cable() {
        assert_eq!(read(vec![reply(60, 0x08, 0x02)]).unwrap(), Status::Battery { level: 60, charging: true });
        assert_eq!(read(vec![reply(60, 0x02, 0x08)]).unwrap(), Status::Battery { level: 60, charging: true });
    }

    #[test]
    fn headset_offline() {
        assert_eq!(read(vec![reply(0, 0x01, 0x00)]).unwrap(), Status::Off);
        assert_eq!(read(vec![vec![0x07, 0xB5, 0x00, 0x00, 0x01]]).unwrap(), Status::Off);
    }

    #[test]
    fn level_from_07_b7_frame_and_power_from_07_b5() {
        let frames = vec![vec![0x07, 0xB7, 0x48, 0x64, 0x08], vec![0x07, 0xB5, 0x00, 0x00, 0x02]];
        assert_eq!(read(frames).unwrap(), Status::Battery { level: 72, charging: true });
        assert_eq!(read(vec![vec![0x07, 0xB7, 0x48, 0x64, 0x02]]).unwrap(), Status::Battery { level: 72, charging: true });
    }

    #[test]
    fn other_reports_are_ignored() {
        let replies = vec![
            vec![0x07, 0xB1, 0x05, 0x00, 0x01],
            vec![0x01, 0xB1, 0x00, 0x00, 0x00, 0x00, 0x10],
            vec![0x07, 0xB7, 0xFF, 0x00, 0x02],
            reply(31, 0x08, 0x08),
        ];
        assert_eq!(read(replies).unwrap(), Status::Battery { level: 31, charging: false });
    }

    #[test]
    fn unreadable_level_is_an_error() {
        assert!(read(vec![reply(0xC8, 0x08, 0x08)]).is_err());
        assert!(read(vec![reply(31, 0x08, 0x08)[..10].to_vec()]).is_err());
    }

    #[test]
    fn silent_station_is_an_error() {
        assert!(read(vec![]).is_err());
    }

    #[test]
    fn probe_on_the_vendor_collection_of_interface_3_only() {
        let d = HidInfo { vid: VID, pid: PID, usage_page: 0xFFC0, usage: 0x0001, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some(NAME));
        assert_eq!(super::super::find(&d).map(|(f, n)| (f.name, n)), Some((FAMILY.name, NAME)));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0xFF00, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 0x0002, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { interface: 4, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x2247, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x22A1, ..d }), None);
    }
}
