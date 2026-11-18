//! Razer BlackShark V2 Pro (2020), récepteur 1532:0528.
//!
//! Protocole repris de Halo Battery (`providers/blackshark2020.py`, licence MIT), qui cite la
//! requête relevée sous Synapse (https://github.com/openrazer/openrazer/issues/1280) et
//! RazerNariBatteryLevel (https://github.com/Modzeleczek/RazerNariBatteryLevel) :
//!  * collection constructeur 0xFF00/0x0001, rapports de fonction de 64 octets, identifiant 0xFF ;
//!  * requête `ff 0a 00 fd 04 12 f1 02 05` complétée de zéros ;
//!  * réponse fraîche `ff 0f 05 fe 12 04 1f 08 05 …` : `[11]` état d'alimentation (9 = en charge,
//!    1, 2, 3 ou 6 = sur batterie ou charge terminée), `[12..14]` tension en mV (gros-boutiste,
//!    0 = casque absent), `[14]` niveau en pourcentage (Halo a vu 100, 80, 50, 30 et 10) ;
//!  * réponse périmée `ff 01 00 fe 12 04 1f 08 05 …` : le casque n'a pas répondu (éteint ou hors
//!    de portée).
//!
//! Halo masque le niveau pendant la charge (peu fiable) : ici, le dernier niveau lu sur batterie
//! est gardé tant que le casque charge. Le câble de charge (1532:052E) n'est pas lu : la batterie
//! se lit par le récepteur.

use super::{Driver, Family, HidInfo, Link, Status};
use std::thread::sleep;
use std::time::{Duration, Instant};

const PID: u16 = 0x0528;
const USAGE_PAGE: u16 = 0xFF00;
const USAGE: u16 = 0x0001;

pub const FAMILY: Family = Family {
    name: "Razer BlackShark V2 Pro (2020)",
    vids: &[super::razer::VID],
    probe: |d| {
        (d.vid == super::razer::VID && d.pid == PID && d.usage_page == USAGE_PAGE && d.usage == USAGE)
            .then_some("Razer BlackShark V2 Pro (2020)")
    },
    new: |_: &HidInfo| Box::new(BlackShark2020::default()),
};

const REPORT_ID: u8 = 0xFF;
const LEN: usize = 64;
const REQUEST: [u8; 9] = [0xFF, 0x0A, 0x00, 0xFD, 0x04, 0x12, 0xF1, 0x02, 0x05];
const REPLY_PREFIX: [u8; 9] = [0xFF, 0x0F, 0x05, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05];
const NO_REPLY_PREFIX: [u8; 9] = [0xFF, 0x01, 0x00, 0xFE, 0x12, 0x04, 0x1F, 0x08, 0x05];
const STATE_CHARGING: u8 = 0x09;

fn request() -> Vec<u8> {
    let mut r = vec![0u8; LEN];
    r[..REQUEST.len()].copy_from_slice(&REQUEST);
    r
}

/// (état d'alimentation, tension en mV, octet de niveau) d'une réponse fraîche.
fn parse(d: &[u8]) -> Option<(u8, u16, u8)> {
    (d.len() == LEN && d.starts_with(&REPLY_PREFIX)).then(|| (d[11], u16::from_be_bytes([d[12], d[13]]), d[14]))
}

pub struct BlackShark2020 {
    /// Pause avant chaque lecture de la réponse.
    poll: Duration,
    /// Attente maximale d'une réponse fraîche.
    window: Duration,
    /// Dernier niveau lu sur batterie, affiché pendant la charge.
    last_level: Option<u8>,
}

impl Default for BlackShark2020 {
    fn default() -> Self {
        BlackShark2020 { poll: Duration::from_millis(100), window: Duration::from_millis(500), last_level: None }
    }
}

impl Driver for BlackShark2020 {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        link.send_feature(&request())?;
        let end = Instant::now() + self.window;
        loop {
            sleep(self.poll);
            let data = link.get_feature(REPORT_ID, LEN)?;
            if let Some((state, voltage, raw)) = parse(&data) {
                if voltage == 0 {
                    return Ok(Status::Off);
                }
                // Halo : état inconnu = charge inconnue, comptée comme « pas en charge ».
                let charging = state == STATE_CHARGING;
                let level = if charging { self.last_level } else { (raw <= 100).then_some(raw) };
                let Some(level) = level else {
                    anyhow::bail!(if charging {
                        "niveau inconnu pendant la charge (pas encore lu sur batterie)".to_owned()
                    } else {
                        format!("niveau de batterie illisible : {raw:#04x}")
                    });
                };
                if !charging {
                    self.last_level = Some(level);
                }
                return Ok(Status::Battery { level, charging });
            }
            if Instant::now() >= end {
                if data.len() == LEN && data.starts_with(&NO_REPLY_PREFIX) {
                    return Ok(Status::Off);
                }
                anyhow::bail!("pas de réponse valable du récepteur : {:02x?}", &data[..data.len().min(16)]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(hex: &str) -> Vec<u8> {
        let mut v: Vec<u8> = hex.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect();
        v.resize(LEN, 0);
        v
    }

    // Relevés de Halo sur le matériel (`providers/blackshark2020.py`).
    const ON_BATTERY: &str = "ff 0f 05 fe 12 04 1f 08 05 03 05 01 0e c0 50";
    const CHARGING: &str = "ff 0f 05 fe 12 04 1f 08 05 05 03 09 10 80 50";
    const CHARGE_COMPLETE: &str = "ff 0f 05 fe 12 04 1f 08 05 06 05 06 10 88 64";
    const LOW: &str = "ff 0f 05 fe 12 04 1f 08 05 03 05 02 0e 08 1e 00";
    const HEADSET_OFF: &str = "ff 01 00 fe 12 04 1f 08 05 05 03 09 10 88 50";

    /// Faux récepteur : rend les réponses dans l'ordre, la dernière ensuite.
    struct Fake {
        replies: Vec<Vec<u8>>,
        reads: usize,
        requests: usize,
    }

    impl Fake {
        fn new(replies: &[&str]) -> Fake {
            Fake { replies: replies.iter().map(|h| frame(h)).collect(), reads: 0, requests: 0 }
        }
    }

    impl Link for Fake {
        fn write(&mut self, _: &[u8]) -> anyhow::Result<()> {
            anyhow::bail!("pas de rapport de sortie")
        }
        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(vec![])
        }
        fn send_feature(&mut self, r: &[u8]) -> anyhow::Result<()> {
            assert_eq!(r, request().as_slice());
            self.requests += 1;
            Ok(())
        }
        fn get_feature(&mut self, id: u8, len: usize) -> anyhow::Result<Vec<u8>> {
            assert_eq!((id, len), (REPORT_ID, LEN));
            let r = self.replies[self.reads.min(self.replies.len() - 1)].clone();
            self.reads += 1;
            Ok(r)
        }
    }

    fn reader() -> BlackShark2020 {
        BlackShark2020 { poll: Duration::ZERO, window: Duration::from_millis(20), last_level: None }
    }

    #[test]
    fn reads_level_on_battery() {
        let mut dev = Fake::new(&[ON_BATTERY]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 80, charging: false });
        let mut dev = Fake::new(&[LOW]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 30, charging: false });
        let mut dev = Fake::new(&[CHARGE_COMPLETE]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
    }

    #[test]
    fn stale_header_is_skipped_until_a_fresh_reply() {
        let mut dev = Fake::new(&[HEADSET_OFF, HEADSET_OFF, ON_BATTERY]);
        // Fenêtre large : la réponse fraîche arrive à la 3e lecture, et une machine chargée
        // pourrait dépasser 20 ms avant (le test finit dès cette réponse).
        let mut r = BlackShark2020 { window: Duration::from_secs(10), ..reader() };
        assert_eq!(r.read(&mut dev).unwrap(), Status::Battery { level: 80, charging: false });
        assert_eq!(dev.requests, 1, "une seule requête");
    }

    #[test]
    fn charging_keeps_the_last_level_read_on_battery() {
        let mut r = reader();
        assert!(r.read(&mut Fake::new(&[CHARGING])).is_err(), "aucun niveau connu");
        r.read(&mut Fake::new(&[LOW])).unwrap();
        assert_eq!(r.read(&mut Fake::new(&[CHARGING])).unwrap(), Status::Battery { level: 30, charging: true });
    }

    #[test]
    fn headset_off() {
        assert_eq!(reader().read(&mut Fake::new(&[HEADSET_OFF])).unwrap(), Status::Off);
        let no_voltage = "ff 0f 05 fe 12 04 1f 08 05 03 05 01 00 00 50";
        assert_eq!(reader().read(&mut Fake::new(&[no_voltage])).unwrap(), Status::Off);
    }

    #[test]
    fn unreadable_replies_are_errors() {
        assert!(reader().read(&mut Fake::new(&["01 02 03"])).is_err());
        let bad_level = "ff 0f 05 fe 12 04 1f 08 05 03 05 01 0e c0 c8";
        assert!(reader().read(&mut Fake::new(&[bad_level])).is_err());
    }

    #[test]
    fn unknown_power_state_counts_as_not_charging() {
        let odd = "ff 0f 05 fe 12 04 1f 08 05 03 05 07 0e c0 50";
        assert_eq!(reader().read(&mut Fake::new(&[odd])).unwrap(), Status::Battery { level: 80, charging: false });
    }

    #[test]
    fn probe_vendor_collection_of_the_2020_receiver_only() {
        let d = HidInfo { vid: 0x1532, pid: PID, usage_page: USAGE_PAGE, usage: USAGE, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("Razer BlackShark V2 Pro (2020)"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 2, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x052E, ..d }), None, "câble de charge");
    }
}
