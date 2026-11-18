//! Astro A50 Gen 5 (046D:0B1C) par sa station d'accueil, sans G HUB.
//!
//! Protocole repris de Halo Battery (`providers/astro.py`, licence MIT), lui-même tiré du
//! pilote `logitech_astro_a50` de HeadsetControl (Sapd/HeadsetControl, rétro-ingénierie de
//! captures de G HUB). Ce n'est ni HID++ ni le protocole « Centurion » de l'A50 X :
//!  * collection HID constructeur 0xFF32/0x0074 (interface 8 sur la station, mais c'est la page
//!    d'usage qui l'identifie) ;
//!  * trames de 64 octets, identifiant de rapport 0x02 :
//!    `02 0c <len> 00 <commande> <poignée> <charge utile…>`, `len` = 3 + longueur de la charge ;
//!  * batterie : commande 0x06 sans charge utile ; réponse `02 0c 06 00 06 00 <niveau> <niveau 2>
//!    <station>` : octet 6 = pourcentage, octet 8 non nul = casque posé sur la station, ce qui
//!    tient lieu de « en charge ».
//!
//! La station envoie aussi d'elle-même des trames de même forme : seule une trame qui répond à
//! notre commande est retenue, et un niveau au-delà de 100 est refusé. Halo ne sait pas
//! distinguer un casque éteint : ce pilote non plus (pas de `Status::Off`).

use super::{Driver, Family, HidInfo, Link, Status};

const VID: u16 = 0x046D;
const USAGE_PAGE: u16 = 0xFF32;
const USAGE: u16 = 0x0074;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x0B1C, "Astro A50 Gen 5")];

pub const FAMILY: Family = Family {
    name: "Astro",
    vids: &[VID],
    probe: |d| {
        let name = PRODUCTS.iter().find(|(pid, _)| d.vid == VID && *pid == d.pid)?.1;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Astro),
};

const REPORT_ID: u8 = 0x02;
const MARKER: u8 = 0x0C;
const FRAME: usize = 64;
const CMD_BATTERY: u8 = 0x06;
/// Poignée de transaction choisie par Halo pour la batterie.
const HANDLE_BATTERY: u8 = 0x0C;
const LEVEL_INDEX: usize = 6;
const DOCK_INDEX: usize = 8;
/// Lectures tentées avant d'abandonner, chacune attendant au plus `READ_MS`.
const POLL_ATTEMPTS: usize = 8;
const READ_MS: i32 = 300;

/// Trame de requête de 64 octets.
fn request(cmd: u8, handle: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = [&[REPORT_ID, MARKER, 3 + payload.len() as u8, 0x00, cmd, handle][..], payload].concat();
    out.resize(FRAME, 0);
    out
}

/// La trame répond-elle à la commande `cmd` ?
fn answers(r: &[u8], cmd: u8) -> bool {
    r.len() > 4 && r[0] == REPORT_ID && r[1] == MARKER && r[4] == cmd
}

pub struct Astro;

impl Driver for Astro {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        link.write(&request(CMD_BATTERY, HANDLE_BATTERY, &[]))?;
        for _ in 0..POLL_ATTEMPTS {
            let r = link.read(READ_MS)?;
            if !answers(&r, CMD_BATTERY) {
                continue;
            }
            return match (r.get(LEVEL_INDEX), r.get(DOCK_INDEX)) {
                (Some(&level), Some(&dock)) if level <= 100 => Ok(Status::Battery { level, charging: dock != 0 }),
                _ => Err(anyhow::anyhow!("niveau de batterie illisible : {:02x?}", &r[..r.len().min(12)])),
            };
        }
        Err(anyhow::anyhow!("la station ne répond pas à la commande batterie"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Fausse station : répond à la commande batterie, après d'éventuelles trames spontanées.
    struct Fake {
        level: u8,
        dock: u8,
        unsolicited: usize,
        mute: bool,
        queue: VecDeque<Vec<u8>>,
    }

    impl Fake {
        fn new(level: u8, dock: u8) -> Fake {
            Fake { level, dock, unsolicited: 0, mute: false, queue: VecDeque::new() }
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), FRAME);
            assert_eq!(&w[..6], &[0x02, 0x0C, 0x03, 0x00, 0x06, 0x0C]);
            for _ in 0..self.unsolicited {
                self.queue.push_back(request(0x0B, 0, &[0x55, 0x55, 0x55]));
            }
            if !self.mute {
                let mut r = vec![0x02, 0x0C, 0x06, 0x00, 0x06, 0x00, self.level, self.level, self.dock];
                r.resize(FRAME, 0);
                self.queue.push_back(r);
            }
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn reads_level_and_dock() {
        assert_eq!(Astro.read(&mut Fake::new(73, 0)).unwrap(), Status::Battery { level: 73, charging: false });
        assert_eq!(Astro.read(&mut Fake::new(40, 1)).unwrap(), Status::Battery { level: 40, charging: true });
    }

    #[test]
    fn unsolicited_frames_are_skipped() {
        let mut dev = Fake::new(64, 0);
        dev.unsolicited = 3;
        assert_eq!(Astro.read(&mut dev).unwrap(), Status::Battery { level: 64, charging: false });
    }

    #[test]
    fn level_above_100_is_refused() {
        assert!(Astro.read(&mut Fake::new(0xFF, 0)).is_err());
    }

    #[test]
    fn silent_station_is_an_error() {
        let mut dev = Fake::new(50, 0);
        dev.mute = true;
        assert!(Astro.read(&mut dev).is_err());
    }

    #[test]
    fn too_many_unsolicited_frames_give_up() {
        let mut dev = Fake::new(50, 0);
        dev.unsolicited = POLL_ATTEMPTS;
        assert!(Astro.read(&mut dev).is_err());
    }

    #[test]
    fn probe_only_the_vendor_collection() {
        let d = HidInfo { vid: 0x046D, pid: 0x0B1C, usage_page: 0xFF32, usage: 0x0074, interface: 8 };
        assert_eq!((FAMILY.probe)(&d), Some("Astro A50 Gen 5"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, usage: 1, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 0x0001, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0AF7, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { vid: 0x9886, ..d }), None);
    }
}
