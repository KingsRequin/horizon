//! Razer Barracuda Pro en 2,4 GHz, récepteur 1532:053A.
//!
//! Protocole repris de Halo Battery (`providers/barracuda.py`, licence MIT), relevé par Halo sur
//! une capture USBPcap de Synapse (son ticket n° 10). Même famille que le protocole « PA » du
//! BlackShark V2 Pro (OpenRazer, `razer_blackshark`), mais décalée de deux octets : les trames
//! diffèrent, d'où un module à part.
//!  * collection constructeur (page 0xFF00), rapports de sortie et d'entrée de 64 octets,
//!    identifiant 0x01 ;
//!  * requête : `01 80 08 'P' 'A' 08 08 03 <commande>` (0x21 = niveau en %, 0x2A = en charge) ;
//!  * « mode distant », envoyé par Synapse avant d'interroger : `01 80 07 'P' 'A' 0e 08 02 e1
//!    <1|0>` ; Halo l'active, interroge, puis le désactive ;
//!  * réponse : `01 80 <longueur> 'P' 'I' 08 …`, `[13]` écho de la commande, `[14]` = 1 (réponse)
//!    ou 2 (notification), `[15]` longueur (1 à 32), `[16..]` données.
//!
//! Récepteur qui refuse les trames ou casque qui ne répond pas : casque éteint.

use super::razer::VID;
use super::{Driver, Family, HidInfo, Link, Status};
use std::thread::sleep;
use std::time::Duration;

const PID: u16 = 0x053A;
const USAGE_PAGE: u16 = 0xFF00;

pub const FAMILY: Family = Family {
    name: "Razer Barracuda Pro",
    vids: &[VID],
    probe: |d| (d.vid == VID && d.pid == PID && d.usage_page == USAGE_PAGE).then_some("Razer Barracuda Pro (2.4 GHz)"),
    new: |_: &HidInfo| Box::new(Barracuda::default()),
};

const REPORT_ID: u8 = 0x01;
const LEN: usize = 64;
const TYPE_REMOTE: u8 = 0x02;
const TYPE_QUERY: u8 = 0x03;
const CMD_REMOTE: u8 = 0xE1;
const CMD_BATTERY: u8 = 0x21;
const CMD_CHARGING: u8 = 0x2A;
/// Position de l'écho de la commande dans une réponse (identifiant de rapport compris).
const REPLY_CMD: usize = 13;
const ATTEMPTS: usize = 3;
const WAKE_ATTEMPTS: u32 = 4;
/// Lectures d'une réponse par tentative.
const READS: usize = 6;
const READ_MS: i32 = 150;
const DRAIN_MS: i32 = 20;

fn frame_query(cmd: u8) -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8]) =
        (REPORT_ID, 0x80, 0x08, b'P', b'A', 0x08, 0x08, TYPE_QUERY, cmd);
    b
}

fn frame_remote(on: bool) -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    (b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]) =
        (REPORT_ID, 0x80, 0x07, b'P', b'A', 0x0E, 0x08, TYPE_REMOTE, CMD_REMOTE, u8::from(on));
    b
}

/// Données de la réponse à `cmd` ; sans identifiant de rapport en tête, tout est décalé d'un octet.
fn parse_reply(d: &[u8], cmd: u8) -> Option<&[u8]> {
    let missing = usize::from(d.first() != Some(&REPORT_ID));
    [0usize, 1].into_iter().filter(|&m| m == 0 || missing == 1).find_map(|m| {
        let c = REPLY_CMD - m;
        let n = usize::from(*d.get(c + 2)?);
        let ok = d.len() > c + 3 && d[c] == cmd && matches!(d[c + 1], 1 | 2) && (1..=32).contains(&n);
        ok.then(|| &d[c + 3..(c + 3 + n).min(d.len())])
    })
}

pub struct Barracuda {
    /// Pas des nouvelles tentatives de réveil (×1, ×2, ×3…).
    wake_step: Duration,
    /// Entre le réveil et la première requête.
    settle: Duration,
}

impl Default for Barracuda {
    fn default() -> Self {
        Barracuda { wake_step: Duration::from_millis(150), settle: Duration::from_millis(50) }
    }
}

impl Barracuda {
    fn drain(link: &mut dyn Link) {
        for _ in 0..8 {
            if link.read(DRAIN_MS).map_or(true, |r| r.is_empty()) {
                return;
            }
        }
    }

    fn remote(link: &mut dyn Link, on: bool) -> bool {
        Self::drain(link);
        link.write(&frame_remote(on)).is_ok()
    }

    fn query(link: &mut dyn Link, cmd: u8) -> Option<Vec<u8>> {
        for _ in 0..ATTEMPTS {
            Self::drain(link);
            link.write(&frame_query(cmd)).ok()?;
            for _ in 0..READS {
                let data = link.read(READ_MS).unwrap_or_default();
                if let Some(p) = parse_reply(&data, cmd) {
                    return Some(p.to_vec());
                }
            }
        }
        None
    }
}

impl Driver for Barracuda {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let woke = (1..=WAKE_ATTEMPTS).any(|n| {
            let ok = Self::remote(link, true);
            if !ok {
                sleep(self.wake_step * n);
            }
            ok
        });
        if !woke {
            return Ok(Status::Off);
        }
        sleep(self.settle);
        let Some(battery) = Self::query(link, CMD_BATTERY) else {
            Self::remote(link, false);
            return Ok(Status::Off);
        };
        let charging = Self::query(link, CMD_CHARGING).is_some_and(|c| c[0] != 0);
        Self::remote(link, false);
        match battery[0] {
            level @ 0..=100 => Ok(Status::Battery { level, charging }),
            v => anyhow::bail!("niveau de batterie illisible : {v:#04x}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux récepteur. `level` : `None` = casque éteint (le récepteur ne répond pas).
    struct Fake {
        level: Option<u8>,
        charging: bool,
        /// Trames refusées avant d'accepter.
        refuse: usize,
        /// Notifications et réponses étrangères glissées avant chaque réponse.
        noise: bool,
        no_report_id: bool,
        /// Réponse d'état 2 (notification) au lieu de 1.
        notification: bool,
        queue: VecDeque<Vec<u8>>,
        remote: Option<bool>,
    }

    impl Fake {
        fn new(level: Option<u8>) -> Fake {
            Fake { level, charging: false, refuse: 0, noise: false, no_report_id: false, notification: false, queue: VecDeque::new(), remote: None }
        }

        fn reply(&self, cmd: u8, kind: u8, data: &[u8]) -> Vec<u8> {
            let mut r = vec![0u8; LEN];
            (r[0], r[1], r[2], r[3], r[4], r[5]) = (REPORT_ID, 0x80, 16 + data.len() as u8, b'P', b'I', 0x08);
            (r[13], r[14], r[15]) = (cmd, kind, data.len() as u8);
            r[16..16 + data.len()].copy_from_slice(data);
            if self.no_report_id {
                r.remove(0);
            }
            r
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!((w.len(), w[0], &w[3..5]), (LEN, REPORT_ID, &b"PA"[..]));
            if self.refuse > 0 {
                self.refuse -= 1;
                anyhow::bail!("trame refusée");
            }
            match (w[7], w[8]) {
                (TYPE_REMOTE, CMD_REMOTE) => self.remote = Some(w[9] == 1),
                (TYPE_QUERY, cmd) => {
                    let Some(level) = self.level else { return Ok(()) };
                    if self.noise {
                        self.queue.push_back(self.reply(0x99, 1, &[0x42]));
                        self.queue.push_back(self.reply(cmd, 3, &[0x42]));
                    }
                    let data = match cmd {
                        CMD_BATTERY => [level],
                        CMD_CHARGING => [u8::from(self.charging)],
                        _ => return Ok(()),
                    };
                    let kind = if self.notification { 2 } else { 1 };
                    self.queue.push_back(self.reply(cmd, kind, &data));
                }
                _ => panic!("trame inattendue {w:02x?}"),
            }
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn reader() -> Barracuda {
        Barracuda { wake_step: Duration::ZERO, settle: Duration::ZERO }
    }

    #[test]
    fn reads_level_and_charging() {
        let mut dev = Fake::new(Some(64));
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 64, charging: false });
        assert_eq!(dev.remote, Some(false), "mode distant désactivé après lecture");
        dev.charging = true;
        dev.notification = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 64, charging: true });
    }

    #[test]
    fn replies_without_report_id_are_read() {
        let mut dev = Fake::new(Some(12));
        dev.no_report_id = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 12, charging: false });
    }

    #[test]
    fn foreign_reports_are_ignored() {
        let mut dev = Fake::new(Some(88));
        dev.noise = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 88, charging: false });
    }

    #[test]
    fn headset_off() {
        assert_eq!(reader().read(&mut Fake::new(None)).unwrap(), Status::Off);
        let mut dev = Fake::new(Some(50));
        dev.refuse = WAKE_ATTEMPTS as usize;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off, "récepteur qui refuse les trames");
        let mut dev = Fake::new(Some(50));
        dev.refuse = 1;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 50, charging: false }, "réveillé");
    }

    #[test]
    fn unreadable_level_is_an_error() {
        assert!(reader().read(&mut Fake::new(Some(0xC8))).is_err());
        let mut r = vec![0u8; LEN];
        (r[0], r[13], r[14], r[15]) = (REPORT_ID, CMD_BATTERY, 1, 0);
        assert_eq!(parse_reply(&r, CMD_BATTERY), None, "longueur nulle");
        assert_eq!(parse_reply(&[REPORT_ID, 0x80], CMD_BATTERY), None, "trop court");
    }

    #[test]
    fn frames_layout() {
        assert_eq!(&frame_query(CMD_BATTERY)[..9], &[0x01, 0x80, 0x08, b'P', b'A', 0x08, 0x08, 0x03, 0x21]);
        assert_eq!(&frame_remote(false)[..10], &[0x01, 0x80, 0x07, b'P', b'A', 0x0E, 0x08, 0x02, 0xE1, 0]);
    }

    #[test]
    fn probe_vendor_collection_only() {
        let d = HidInfo { vid: VID, pid: PID, usage_page: USAGE_PAGE, usage: 0, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("Razer Barracuda Pro (2.4 GHz)"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, usage: 1, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0555, ..d }), None);
    }
}
