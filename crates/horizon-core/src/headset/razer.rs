//! Razer : casques lus par le rapport de fonction de 90 octets de Synapse.
//!
//! Protocole repris de Halo Battery (`providers/razer.py`, licence MIT), lui-même tiré d'OpenRazer
//! (https://github.com/openrazer/openrazer, pilote `razercommon`). Halo y lit aussi des souris et
//! des claviers sans fil : seuls les casques sont gardés ici. Les autres casques Razer de Halo ont
//! leur module : BlackShark V2 Pro 2020 (`razer_blackshark2020`), BlackShark V2 Pro 2023 sur les
//! récepteurs 0555/0556 (`razer_blackshark`, protocole « PA »), Barracuda Pro (`razer_barracuda`).
//!
//! Rapport de fonction d'identifiant 0, 90 octets :
//!  * `[0]` état (0 dans une requête ; 2 = OK, 1 = occupé, 3 = échec, 4 = le casque ne répond
//!    pas, 5 = non pris en charge) ;
//!  * `[1]` identifiant de transaction (0x3F pour les casques, 0x1F, 0xFF… pour d'autres) ;
//!  * `[5]` taille des données, `[6]` classe de commande (0x07 = alimentation), `[7]` commande
//!    (0x80 = niveau, 0x84 = en charge), `[8..88]` arguments (`[9]` = valeur) ;
//!  * `[88]` CRC = XOR des octets 2 à 87.
//!
//! Le niveau est rendu sur 0–255, ramené en pourcentage. État 4 : le récepteur répond, pas le
//! casque (éteint). Une réponse à une autre commande vient d'un autre logiciel (Synapse, Chroma)
//! qui interroge la même collection : elle est ignorée, la requête renvoyée de temps en temps.
//! Halo essaie toutes les collections du récepteur et plusieurs identifiants de transaction : chaque
//! collection a ici son pilote, qui retient l'identifiant qui répond et met de côté pendant cinq
//! minutes une collection où aucun ne répond.

use super::{Driver, Family, HidInfo, Link, Status};
use std::thread::sleep;
use std::time::{Duration, Instant};

pub(super) const VID: u16 = 0x1532;
/// Casques lus par ce protocole : identifiant produit, nom affiché, identifiant de transaction.
/// Le 0557 se lit aussi par le protocole « PA » sur ses collections constructeur
/// (`razer_blackshark`, qui reprend alors ce module en premier) : ici, ses autres collections.
const PRODUCTS: &[(u16, &str, u8)] = &[(0x0557, "Razer BlackShark V2 Pro (2023)", 0x3F)];

pub const FAMILY: Family = Family {
    name: "Razer",
    vids: &[VID],
    probe: |d| product(d).map(|p| p.1),
    new: |d: &HidInfo| Box::new(Razer::new(product(d).map_or(0x3F, |p| p.2))),
};

fn product(d: &HidInfo) -> Option<&'static (u16, &'static str, u8)> {
    PRODUCTS.iter().find(|p| d.vid == VID && p.0 == d.pid)
}

/// Identifiants de transaction essayés après celui du modèle.
const TRANSACTION_IDS: [u8; 5] = [0x1F, 0x3F, 0xFF, 0x9F, 0x08];
const LEN: usize = 90;
const STATUS_OK: u8 = 0x02;
const STATUS_BUSY: u8 = 0x01;
const STATUS_TIMEOUT: u8 = 0x04;
const CLASS_POWER: u8 = 0x07;
const CMD_LEVEL: u8 = 0x80;
const CMD_CHARGING: u8 = 0x84;
/// Envois au plus d'une même requête quand les réponses sont pour un autre logiciel.
const SENDS: u32 = 3;
/// Mise de côté d'une collection muette.
const DEAD: Duration = Duration::from_secs(300);

/// Requête (identifiant de rapport 0 en tête, puis les 90 octets).
fn request(tid: u8, class: u8, id: u8) -> Vec<u8> {
    let mut m = vec![0u8; 1 + LEN];
    let r = &mut m[1..];
    r[1] = tid;
    r[5] = 2;
    r[6] = class;
    r[7] = id;
    r[88] = r[2..88].iter().fold(0, |a, b| a ^ b);
    m
}

/// Les 90 octets d'une réponse (hidapi rend l'identifiant de rapport en tête), `None` si courte.
fn normalize(resp: &[u8]) -> Option<&[u8]> {
    let d = if resp.len() > LEN { &resp[1..=LEN] } else { resp };
    (d.len() >= LEN).then_some(d)
}

/// État et valeur ; pas de valeur si la réponse est celle d'une autre commande.
fn parse(d: &[u8], class: u8, id: u8) -> (u8, Option<u8>) {
    (d[0], (d[6] == class && d[7] == id).then_some(d[9]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Status(u8, Option<u8>),
    /// Seulement des réponses à d'autres commandes : un autre logiciel occupe la collection.
    Foreign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Got {
    Battery(Status),
    /// État 4 : le casque ne répond pas.
    Timeout,
    Foreign,
    /// Autre état (occupé, échec, non pris en charge).
    Refused(u8),
}

/// Attentes du protocole (raccourcies dans les tests).
#[derive(Debug, Clone, Copy)]
pub(super) struct Pace {
    /// Avant la première lecture de la réponse.
    settle: Duration,
    /// Entre deux lectures quand le récepteur est occupé.
    retry: Duration,
    /// Attente maximale d'une réponse à soi.
    window: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace { settle: Duration::from_millis(60), retry: Duration::from_millis(80), window: Duration::from_secs(1) }
    }
}

/// Lecteur d'une collection ; retient l'identifiant de transaction qui répond.
pub(super) struct Razer {
    pace: Pace,
    preferred: u8,
    tid: Option<u8>,
    dead_until: Option<Instant>,
}

impl Razer {
    pub(super) fn new(preferred: u8) -> Razer {
        Razer::with_pace(preferred, Pace::default())
    }

    pub(super) fn with_pace(preferred: u8, pace: Pace) -> Razer {
        Razer { pace, preferred, tid: None, dead_until: None }
    }

    /// `None` : pas de réponse (envoi ou lecture refusés, réponse trop courte).
    fn query(&self, link: &mut dyn Link, tid: u8, class: u8, id: u8) -> Option<Answer> {
        let req = request(tid, class, id);
        link.send_feature(&req).ok()?;
        let (mut sends, mut foreign) = (1, 0);
        let end = Instant::now() + self.pace.window;
        sleep(self.pace.settle);
        loop {
            let resp = link.get_feature(0, LEN + 1).ok()?;
            let (status, value) = parse(normalize(&resp)?, class, id);
            let is_foreign = status == STATUS_OK && value.is_none();
            if Instant::now() < end && (status == STATUS_BUSY || is_foreign) {
                if is_foreign {
                    foreign += 1;
                    if foreign % 4 == 0 && sends < SENDS {
                        link.send_feature(&req).ok()?;
                        sends += 1;
                    }
                }
                sleep(self.pace.retry);
                continue;
            }
            return Some(if is_foreign { Answer::Foreign } else { Answer::Status(status, value) });
        }
    }

    fn read_with(&self, link: &mut dyn Link, tid: u8) -> Option<Got> {
        Some(match self.query(link, tid, CLASS_POWER, CMD_LEVEL)? {
            Answer::Status(STATUS_OK, Some(raw)) => {
                // Arrondi de raw / 255 × 100 (jamais à mi-chemin).
                let level = ((u32::from(raw) * 200 + 255) / 510) as u8;
                let charging = matches!(
                    self.query(link, tid, CLASS_POWER, CMD_CHARGING),
                    Some(Answer::Status(STATUS_OK, Some(c))) if c != 0
                );
                Got::Battery(Status::Battery { level, charging })
            }
            Answer::Status(STATUS_TIMEOUT, _) => Got::Timeout,
            Answer::Status(status, _) => Got::Refused(status),
            Answer::Foreign => Got::Foreign,
        })
    }

    /// Identifiants à essayer : celui du modèle d'abord.
    fn tids(&self) -> impl Iterator<Item = u8> + '_ {
        std::iter::once(self.preferred).chain(TRANSACTION_IDS.into_iter().filter(|&t| t != self.preferred))
    }
}

fn foreign() -> anyhow::Error {
    anyhow::anyhow!("réponses pour un autre logiciel seulement (Synapse, Chroma…) : collection occupée")
}

impl Driver for Razer {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        if let Some(tid) = self.tid {
            match self.read_with(link, tid) {
                Some(Got::Battery(s)) => return Ok(s),
                Some(Got::Timeout) => return Ok(Status::Off),
                Some(Got::Foreign) => return Err(foreign()),
                _ => self.tid = None,
            }
        }
        if self.dead_until.is_some_and(|t| Instant::now() < t) {
            anyhow::bail!("collection muette au protocole Razer (nouvel essai dans quelques minutes)");
        }
        let (mut answered, mut timeout, mut refused) = (false, false, None);
        for tid in self.tids().collect::<Vec<_>>() {
            let Some(got) = self.read_with(link, tid) else { continue };
            answered = true;
            match got {
                Got::Battery(s) => {
                    self.tid = Some(tid);
                    return Ok(s);
                }
                Got::Timeout => {
                    self.tid = Some(tid);
                    timeout = true;
                }
                Got::Foreign => {
                    self.tid = Some(tid);
                    return Err(foreign());
                }
                Got::Refused(status) => refused = Some(status),
            }
        }
        if !answered {
            self.dead_until = Some(Instant::now() + DEAD);
        }
        match (timeout, refused) {
            (true, _) => Ok(Status::Off),
            (false, Some(s)) => anyhow::bail!("le récepteur refuse la lecture de la batterie (état {s:#04x})"),
            (false, None) => anyhow::bail!("pas de réponse au protocole Razer"),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in super::super) fn pace() -> Pace {
        Pace { settle: Duration::ZERO, retry: Duration::ZERO, window: Duration::from_millis(20) }
    }

    /// Faux récepteur : répond à la dernière requête reçue.
    pub(in super::super) struct Fake {
        /// Niveau sur 0–255 ; `None` : casque éteint (état 4).
        pub level: Option<u8>,
        pub charging: bool,
        /// Seul identifiant de transaction accepté (état 5 pour les autres).
        pub tid: u8,
        /// Réponses pour un autre logiciel glissées avant chacune des nôtres.
        pub noise: usize,
        /// Réponses de 20 octets seulement.
        pub short: bool,
        last: Option<Vec<u8>>,
        pending_noise: usize,
        pub sends: usize,
    }

    impl Fake {
        pub fn new(level: Option<u8>) -> Fake {
            Fake { level, charging: false, tid: 0x3F, noise: 0, short: false, last: None, pending_noise: 0, sends: 0 }
        }

        fn reply(&self, req: &[u8]) -> Vec<u8> {
            let mut r = vec![0u8; 1 + LEN];
            r[1..].copy_from_slice(&req[1..]);
            let (tid, class, id) = (req[2], req[7], req[8]);
            r[1] = if tid != self.tid {
                0x05
            } else {
                match (self.level, class, id) {
                    (None, ..) => STATUS_TIMEOUT,
                    (Some(l), CLASS_POWER, CMD_LEVEL) => {
                        r[10] = l;
                        STATUS_OK
                    }
                    (Some(_), CLASS_POWER, CMD_CHARGING) => {
                        r[10] = u8::from(self.charging);
                        STATUS_OK
                    }
                    _ => 0x05,
                }
            };
            r
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
            assert_eq!(r.len(), 1 + LEN);
            assert_eq!(r[0], 0, "identifiant de rapport 0");
            assert_eq!(r[1 + 88], r[1 + 2..1 + 88].iter().fold(0, |a, b| a ^ b), "CRC");
            self.sends += 1;
            self.last = Some(r.to_vec());
            self.pending_noise = self.noise;
            Ok(())
        }
        fn get_feature(&mut self, id: u8, len: usize) -> anyhow::Result<Vec<u8>> {
            assert_eq!((id, len), (0, 1 + LEN));
            let Some(req) = self.last.clone() else { anyhow::bail!("aucune requête") };
            if self.short {
                return Ok(vec![0; 20]);
            }
            if self.pending_noise > 0 {
                self.pending_noise -= 1;
                let mut r = vec![0u8; 1 + LEN];
                (r[1], r[2], r[7], r[8], r[10]) = (STATUS_OK, req[2], 0x0F, 0x82, 0x55);
                return Ok(r);
            }
            Ok(self.reply(&req))
        }
    }

    fn reader() -> Razer {
        Razer::with_pace(0x3F, pace())
    }

    #[test]
    fn level_is_scaled_from_255() {
        for (raw, pct) in [(255, 100), (0, 0), (128, 50), (204, 80), (3, 1)] {
            let mut dev = Fake::new(Some(raw));
            assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: pct, charging: false });
        }
    }

    #[test]
    fn charging_flag() {
        let mut dev = Fake::new(Some(255));
        dev.charging = true;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 100, charging: true });
    }

    #[test]
    fn headset_off_is_status_4() {
        let mut dev = Fake::new(None);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn other_transaction_ids_are_tried_and_the_answering_one_kept() {
        let mut r = reader();
        let mut dev = Fake::new(Some(255));
        dev.tid = 0x1F;
        assert_eq!(r.read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
        assert_eq!(r.tid, Some(0x1F));
        dev.sends = 0;
        r.read(&mut dev).unwrap();
        assert_eq!(dev.sends, 2, "niveau et charge, sans nouvelle recherche");
    }

    #[test]
    fn replies_to_other_software_are_ignored() {
        let mut dev = Fake::new(Some(255));
        dev.noise = 2;
        // Trois lectures sans pause : la fenêtre n'est qu'un plafond. Celle de 20 ms de `pace()`
        // pouvait expirer avant la troisième sur une machine chargée (tests en parallèle) et la
        // réponse passait pour étrangère.
        let mut r = Razer::with_pace(0x3F, Pace { window: Duration::from_secs(10), ..pace() });
        assert_eq!(r.read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
    }

    #[test]
    fn only_foreign_replies_is_an_error() {
        let mut dev = Fake::new(Some(255));
        dev.noise = usize::MAX;
        assert!(reader().read(&mut dev).is_err());
        assert!(dev.sends <= SENDS as usize, "requête renvoyée au plus {SENDS} fois");
    }

    #[test]
    fn short_reply_is_an_error_and_the_collection_set_aside() {
        let mut r = reader();
        let mut dev = Fake::new(Some(255));
        dev.short = true;
        assert!(r.read(&mut dev).is_err());
        assert!(r.dead_until.is_some());
        dev.sends = 0;
        assert!(r.read(&mut dev).is_err());
        assert_eq!(dev.sends, 0, "collection mise de côté");
    }

    #[test]
    fn request_layout() {
        let r = request(0x3F, CLASS_POWER, CMD_LEVEL);
        assert_eq!(&r[..9], &[0, 0, 0x3F, 0, 0, 0, 2, 0x07, 0x80]);
        assert_eq!(r[1 + 88], 0x02 ^ 0x07 ^ 0x80);
    }

    #[test]
    fn probe_only_known_headsets() {
        let d = HidInfo { vid: VID, pid: 0x0557, usage_page: 0x000C, usage: 1, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("Razer BlackShark V2 Pro (2023)"));
        // Souris et claviers de la liste de Halo : non repris.
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x007C, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { vid: 0x046D, ..d }), None);
    }
}
