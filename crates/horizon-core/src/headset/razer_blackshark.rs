//! Razer BlackShark V2 Pro (2023), récepteurs 1532:0555 et 1532:0556 : protocole « PA ».
//!
//! Protocole repris de Halo Battery (`providers/blackshark.py` et `providers/razer.py`, licence
//! MIT), lui-même tiré du pilote OpenRazer `razerblackshark_driver` (PR #2862,
//! https://github.com/openrazer/openrazer), décodé d'une capture de Synapse :
//!  * collection constructeur (page 0xFF00 ou 0xFF14), rapports de sortie et d'entrée de 64 octets,
//!    identifiant 0x02 ;
//!  * requête : `02 80 08 00 00 'P' 'A' 08 00 03 <commande>` (0x21 = niveau en %, 0x2A = en
//!    charge) ;
//!  * « mode distant », exigé avant une requête : `02 80 07 00 00 'P' 'A' 0e 00 02 e1 <1|0>` ; Halo
//!    l'active, interroge, puis le désactive (comme Synapse) ;
//!  * réponse : `[12]` écho de la commande, `[13]` = 1 (accusé), `[14]` longueur, `[15..]` données ;
//!  * au repos, la liaison radio s'assoupit : la première trame ne fait que la réveiller.
//!
//! Récepteur qui refuse les trames ou casque qui ne répond pas : casque éteint. Le 1532:0557 (même
//! casque) est lu d'abord par le protocole Razer de 90 octets (`razer`), puis par celui-ci sur ses
//! collections constructeur, comme dans Halo.

use super::razer::{Razer, VID};
use super::{Driver, Family, HidInfo, Link, Status};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Récepteurs lus par le protocole « PA » d'abord.
const PA_PIDS: [u16; 2] = [0x0555, 0x0556];
/// Récepteur lu par le protocole Razer de 90 octets d'abord, puis par « PA ».
const RAZER_FIRST_PID: u16 = 0x0557;
const USAGE_PAGES: [u16; 2] = [0xFF00, 0xFF14];
const NAME: &str = "Razer BlackShark V2 Pro (2023)";

pub const FAMILY: Family = Family {
    name: "Razer BlackShark V2 Pro (2023)",
    vids: &[VID],
    probe: |d| {
        let pid = PA_PIDS.contains(&d.pid) || d.pid == RAZER_FIRST_PID;
        (d.vid == VID && pid && USAGE_PAGES.contains(&d.usage_page)).then_some(NAME)
    },
    new: |d: &HidInfo| {
        let razer_first = (d.pid == RAZER_FIRST_PID).then(|| Razer::new(0x3F));
        Box::new(BlackShark { pace: Pace::default(), razer_first })
    },
};

const REPORT_ID: u8 = 0x02;
const LEN: usize = 64;
const TYPE_REMOTE: u8 = 0x02;
const TYPE_QUERY: u8 = 0x03;
const CMD_REMOTE: u8 = 0xE1;
const CMD_BATTERY: u8 = 0x21;
const CMD_CHARGING: u8 = 0x2A;
/// Position de l'écho de la commande dans une réponse (identifiant de rapport compris).
const REPLY_CMD: usize = 12;
const ATTEMPTS: usize = 3;
const WAKE_ATTEMPTS: u32 = 4;

fn frame_query(cmd: u8) -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    (b[0], b[1], b[2], b[5], b[6], b[7], b[9], b[10]) = (REPORT_ID, 0x80, 0x08, b'P', b'A', 0x08, TYPE_QUERY, cmd);
    b
}

fn frame_remote(on: bool) -> Vec<u8> {
    let mut b = vec![0u8; LEN];
    (b[0], b[1], b[2], b[5], b[6], b[7], b[9], b[10], b[11]) =
        (REPORT_ID, 0x80, 0x07, b'P', b'A', 0x0E, TYPE_REMOTE, CMD_REMOTE, u8::from(on));
    b
}

/// Données de la réponse à `cmd`, avec ou sans identifiant de rapport en tête.
fn parse_reply(d: &[u8], cmd: u8) -> Option<&[u8]> {
    [0usize, 1].into_iter().find_map(|missing| {
        let c = REPLY_CMD - missing;
        let ok = d.len() > c + 3 && (missing == 1 || d[0] == REPORT_ID) && d[c] == cmd && d[c + 1] == 0x01;
        ok.then(|| {
            let n = usize::from(d[c + 2]).max(1);
            &d[c + 3..(c + 3 + n).min(d.len())]
        })
    })
}

/// Attentes du protocole (raccourcies dans les tests).
#[derive(Debug, Clone, Copy)]
struct Pace {
    /// Pas des nouvelles tentatives de réveil (×1, ×2, ×3…).
    wake_step: Duration,
    /// Après une trame de mode distant.
    settle: Duration,
    /// Attente d'une réponse à une requête.
    reply: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace { wake_step: Duration::from_millis(150), settle: Duration::from_millis(35), reply: Duration::from_millis(150) }
    }
}

const READ_MS: i32 = 50;
const DRAIN_MS: i32 = 5;

pub struct BlackShark {
    pace: Pace,
    /// 0557 : lecteur du protocole Razer de 90 octets, essayé d'abord.
    razer_first: Option<Razer>,
}

impl BlackShark {
    /// Vide les rapports en attente.
    fn drain(link: &mut dyn Link) {
        for _ in 0..32 {
            if link.read(DRAIN_MS).map_or(true, |r| r.is_empty()) {
                return;
            }
        }
    }

    fn remote(&self, link: &mut dyn Link, on: bool) {
        let _ = link.write(&frame_remote(on));
        sleep(self.pace.settle);
    }

    fn query(&self, link: &mut dyn Link, cmd: u8) -> Option<Vec<u8>> {
        Self::drain(link);
        self.remote(link, true);
        let mut result = None;
        'attempts: for _ in 0..ATTEMPTS {
            self.remote(link, true);
            if link.write(&frame_query(cmd)).is_err() {
                break;
            }
            let end = Instant::now() + self.pace.reply;
            while Instant::now() < end {
                let data = link.read(READ_MS).unwrap_or_default();
                if let Some(p) = parse_reply(&data, cmd) {
                    result = Some(p.to_vec());
                    break 'attempts;
                }
            }
        }
        self.remote(link, false);
        result
    }

    fn read_pa(&self, link: &mut dyn Link) -> Status {
        // Réveil : le récepteur peut refuser les premières trames.
        let woke = (1..=WAKE_ATTEMPTS).any(|n| {
            let ok = link.write(&frame_remote(true)).is_ok();
            if !ok {
                sleep(self.pace.wake_step * n);
            }
            ok
        });
        if !woke {
            return Status::Off;
        }
        sleep(self.pace.settle);
        let Some(battery) = self.query(link, CMD_BATTERY) else { return Status::Off };
        // Halo ramène à 100 une valeur plus grande.
        let level = battery[0].min(100);
        let charging = self.query(link, CMD_CHARGING).is_some_and(|c| c[0] != 0);
        Status::Battery { level, charging }
    }
}

impl Driver for BlackShark {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        if let Some(razer) = &mut self.razer_first
            && let Ok(s @ Status::Battery { .. }) = razer.read(link)
        {
            return Ok(s);
        }
        Ok(self.read_pa(link))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux récepteur « PA ». `level` : `None` = casque éteint (le récepteur ne répond pas).
    struct Fake {
        level: Option<u8>,
        charging: bool,
        /// Trames refusées avant d'accepter (radio assoupie, USB en veille).
        refuse: usize,
        /// Rapports étrangers glissés avant chaque réponse.
        noise: bool,
        /// Réponses sans identifiant de rapport en tête.
        no_report_id: bool,
        queue: VecDeque<Vec<u8>>,
        remote: Option<bool>,
        writes: Vec<Vec<u8>>,
    }

    impl Fake {
        fn new(level: Option<u8>) -> Fake {
            Fake {
                level,
                charging: false,
                refuse: 0,
                noise: false,
                no_report_id: false,
                queue: VecDeque::new(),
                remote: None,
                writes: vec![],
            }
        }

        fn reply(&self, cmd: u8, data: &[u8]) -> Vec<u8> {
            let mut r = vec![0u8; LEN];
            (r[0], r[1], r[5], r[6], r[12], r[13], r[14]) = (REPORT_ID, 0x80, b'P', b'A', cmd, 0x01, data.len() as u8);
            r[15..15 + data.len()].copy_from_slice(data);
            if self.no_report_id {
                r.remove(0);
            }
            r
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!((w.len(), w[0], &w[5..7]), (LEN, REPORT_ID, &b"PA"[..]));
            if self.refuse > 0 {
                self.refuse -= 1;
                anyhow::bail!("trame refusée");
            }
            self.writes.push(w.to_vec());
            match (w[9], w[10]) {
                (TYPE_REMOTE, CMD_REMOTE) => self.remote = Some(w[11] == 1),
                (TYPE_QUERY, cmd) => {
                    assert_eq!(self.remote, Some(true), "mode distant exigé");
                    let Some(level) = self.level else { return Ok(()) };
                    if self.noise {
                        self.queue.push_back(self.reply(0x99, &[0x42]));
                        let mut nack = self.reply(cmd, &[0x42]);
                        nack[if self.no_report_id { 12 } else { 13 }] = 0x02;
                        self.queue.push_back(nack);
                    }
                    let data = match cmd {
                        CMD_BATTERY => [level],
                        CMD_CHARGING => [u8::from(self.charging)],
                        _ => return Ok(()),
                    };
                    self.queue.push_back(self.reply(cmd, &data));
                }
                _ => panic!("trame inattendue {w:02x?}"),
            }
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn reader() -> BlackShark {
        BlackShark { pace: Pace { wake_step: Duration::ZERO, settle: Duration::ZERO, reply: Duration::from_millis(10) }, razer_first: None }
    }

    #[test]
    fn reads_level_and_charging() {
        let mut dev = Fake::new(Some(73));
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 73, charging: false });
        assert_eq!(dev.remote, Some(false), "mode distant désactivé après lecture");
        dev.charging = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 73, charging: true });
    }

    #[test]
    fn replies_without_report_id_are_read() {
        let mut dev = Fake::new(Some(40));
        dev.no_report_id = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 40, charging: false });
    }

    #[test]
    fn foreign_reports_are_ignored() {
        let mut dev = Fake::new(Some(55));
        dev.noise = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 55, charging: false });
    }

    #[test]
    fn headset_off_when_nothing_answers() {
        assert_eq!(reader().read(&mut Fake::new(None)).unwrap(), Status::Off);
    }

    #[test]
    fn dozing_receiver_is_woken_then_off_if_it_keeps_refusing() {
        let mut dev = Fake::new(Some(60));
        dev.refuse = 2;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 60, charging: false });
        let mut dev = Fake::new(Some(60));
        dev.refuse = WAKE_ATTEMPTS as usize;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn level_above_100_is_capped_and_unreadable_reply_ignored() {
        let mut dev = Fake::new(Some(0xC8));
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
        assert_eq!(parse_reply(&[0x02, 0x80, 0x00], CMD_BATTERY), None);
        let mut r = vec![0u8; LEN];
        (r[0], r[12], r[13], r[14], r[15]) = (REPORT_ID, CMD_BATTERY, 0x00, 1, 50);
        assert_eq!(parse_reply(&r, CMD_BATTERY), None, "sans accusé");
    }

    #[test]
    fn frames_layout() {
        assert_eq!(&frame_query(CMD_BATTERY)[..11], &[0x02, 0x80, 0x08, 0, 0, b'P', b'A', 0x08, 0, 0x03, 0x21]);
        assert_eq!(&frame_remote(true)[..12], &[0x02, 0x80, 0x07, 0, 0, b'P', b'A', 0x0E, 0, 0x02, 0xE1, 1]);
    }

    /// 0557 : protocole Razer de 90 octets d'abord (faux appareil de `razer`), « PA » ensuite.
    struct Both {
        razer: super::super::razer::tests::Fake,
        pa: Fake,
    }

    impl Link for Both {
        fn write(&mut self, r: &[u8]) -> anyhow::Result<()> {
            self.pa.write(r)
        }
        fn read(&mut self, t: i32) -> anyhow::Result<Vec<u8>> {
            self.pa.read(t)
        }
        fn send_feature(&mut self, r: &[u8]) -> anyhow::Result<()> {
            self.razer.send_feature(r)
        }
        fn get_feature(&mut self, id: u8, len: usize) -> anyhow::Result<Vec<u8>> {
            self.razer.get_feature(id, len)
        }
    }

    #[test]
    fn razer_first_receiver_falls_back_to_pa() {
        use super::super::razer::tests::{pace, Fake as RazerFake};
        let mut r = reader();
        r.razer_first = Some(Razer::with_pace(0x3F, pace()));
        let mut dev = Both { razer: RazerFake::new(Some(255)), pa: Fake::new(Some(20)) };
        assert_eq!(r.read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
        assert!(dev.pa.writes.is_empty(), "« PA » inutile");
        let mut dev = Both { razer: RazerFake::new(None), pa: Fake::new(Some(20)) };
        assert_eq!(r.read(&mut dev).unwrap(), Status::Battery { level: 20, charging: false });
    }

    #[test]
    fn probe_vendor_collections_only() {
        let d = HidInfo { vid: VID, pid: 0x0555, usage_page: 0xFF00, usage: 1, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some(NAME));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0556, usage_page: 0xFF14, ..d }), Some(NAME));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0557, ..d }), Some(NAME));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0528, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { vid: 0x046D, ..d }), None);
    }
}
