//! HyperX Cloud II Wireless (03F0:0696 et 03F0:018B) par son dongle, sans NGENUITY.
//!
//! Protocole repris de Halo Battery (`providers/hyperx.py`, licence MIT), lui-même tiré de
//! HeadsetControl (Sapd/HeadsetControl, appareil `hyperx_cloud_2_wireless`, mêmes identifiants) :
//!  * collection constructeur 0xFF90/0x0303, à choisir par son usage : le dongle en porte quatre
//!    sur la même interface (000c, ff00, ff90, ffc0), et le numéro d'interface varie (0 chez
//!    HeadsetControl, 3 sur un dongle observé par Halo) ;
//!  * requête : 52 octets `06 ff bb <cmd> 00…` (0x06 = identifiant du rapport) ;
//!  * réponse : 20 octets dont les quatre premiers reprennent la requête (`06 ff bb <cmd>`) :
//!    commande 0x02, `réponse[7]` = pourcentage ; commande 0x03, `réponse[4] == 1` = en charge.
//!
//! Une réponse qui ne reprend pas la commande est ignorée ; un niveau au-delà de 100 est refusé
//! plutôt qu'affiché. Sans réponse, le dongle est là mais pas le casque : éteint.
//!
//! Ce module porte aussi ce que les quatre familles HyperX partagent : le fabricant (HP, qui
//! possède HyperX : 0x03F0), la recherche du nom par identifiant produit et l'attente d'une
//! réponse au milieu des autres rapports.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

/// HP Inc., propriétaire de HyperX : tous les casques HyperX lus par Halo sont sous ce fabricant.
pub(super) const HP_VID: u16 = 0x03F0;
const USAGE_PAGE: u16 = 0xFF90;
const USAGE: u16 = 0x0303;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x0696, "HyperX Cloud II Wireless"), (0x018B, "HyperX Cloud II Wireless")];

pub const FAMILY: Family = Family {
    name: "HyperX Cloud II Wireless",
    vids: &[HP_VID],
    probe: |d| {
        let name = product(PRODUCTS, d)?;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(CloudII::default()),
};

/// Nom du casque si la collection appartient à l'un des `products` (fabricant HP).
pub(super) fn product(products: &[(u16, &'static str)], d: &HidInfo) -> Option<&'static str> {
    if d.vid != HP_VID {
        return None;
    }
    products.iter().find(|(pid, _)| *pid == d.pid).map(|(_, name)| *name)
}

/// Lit les rapports d'entrée jusqu'à ce que `accept` en reconnaisse un (`Some`) ou que `window`
/// soit écoulée (`None`) ; les rapports qu'il ne reconnaît pas sont ignorés.
pub(super) fn wait_for<T>(
    link: &mut dyn Link,
    window: Duration,
    read_ms: i32,
    mut accept: impl FnMut(&[u8]) -> Option<T>,
) -> anyhow::Result<Option<T>> {
    let end = Instant::now() + window;
    while Instant::now() < end {
        let r = link.read(read_ms)?;
        if r.is_empty() {
            continue;
        }
        if let Some(t) = accept(&r) {
            return Ok(Some(t));
        }
    }
    Ok(None)
}

const REPORT_ID: u8 = 0x06;
const REQUEST_LEN: usize = 52;
const CMD_LEVEL: u8 = 0x02;
const CMD_CHARGING: u8 = 0x03;
const LEVEL_INDEX: usize = 7;
const CHARGING_INDEX: usize = 4;
const READ_MS: i32 = 100;

fn request(cmd: u8) -> Vec<u8> {
    let mut r = vec![0; REQUEST_LEN];
    r[..4].copy_from_slice(&[REPORT_ID, 0xFF, 0xBB, cmd]);
    r
}

/// Lecteur de batterie du Cloud II Wireless.
pub struct CloudII {
    /// Attente maximale d'une réponse.
    window: Duration,
}

impl Default for CloudII {
    fn default() -> Self {
        CloudII { window: Duration::from_millis(1000) }
    }
}

impl CloudII {
    /// Réponse à `cmd` (elle commence par la requête), `None` sans réponse.
    fn query(&self, link: &mut dyn Link, cmd: u8) -> anyhow::Result<Option<Vec<u8>>> {
        let req = request(cmd);
        link.write(&req)?;
        wait_for(link, self.window, READ_MS, |r| r.starts_with(&req[..4]).then(|| r.to_vec()))
    }
}

impl Driver for CloudII {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let Some(r) = self.query(link, CMD_LEVEL)? else { return Ok(Status::Off) };
        let level = match r.get(LEVEL_INDEX) {
            Some(&level) if level <= 100 => level,
            _ => anyhow::bail!("niveau de batterie illisible : {r:02x?}"),
        };
        // Comme Halo : sans réponse lisible à la seconde question, le casque n'est pas en charge.
        let charging = self
            .query(link, CMD_CHARGING)
            .ok()
            .flatten()
            .is_some_and(|r| r.get(CHARGING_INDEX) == Some(&1));
        Ok(Status::Battery { level, charging })
    }
}

/// Faux appareil commun aux tests des familles HyperX.
#[cfg(test)]
pub(super) mod fake {
    use super::Link;
    use std::collections::VecDeque;

    type Answer = Box<dyn FnMut(&[u8]) -> Vec<Vec<u8>>>;

    /// Rejoue `answer` : rapports d'entrée provoqués par chaque rapport envoyé (sortie ou fonction).
    pub struct Fake {
        answer: Answer,
        /// Rapports d'entrée en attente de lecture (peut être garni d'avance : bruit).
        pub queue: VecDeque<Vec<u8>>,
        pub writes: Vec<Vec<u8>>,
        pub features: Vec<Vec<u8>>,
        /// Erreur rendue par `write` : le rapport n'est alors pas pris.
        pub write_error: Option<&'static str>,
    }

    impl Fake {
        pub fn new(answer: impl FnMut(&[u8]) -> Vec<Vec<u8>> + 'static) -> Fake {
            Fake { answer: Box::new(answer), queue: VecDeque::new(), writes: vec![], features: vec![], write_error: None }
        }
    }

    impl Link for Fake {
        fn write(&mut self, report: &[u8]) -> anyhow::Result<()> {
            if let Some(e) = self.write_error {
                anyhow::bail!(e);
            }
            self.writes.push(report.to_vec());
            let replies = (self.answer)(report);
            self.queue.extend(replies);
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }

        fn send_feature(&mut self, report: &[u8]) -> anyhow::Result<()> {
            self.features.push(report.to_vec());
            let replies = (self.answer)(report);
            self.queue.extend(replies);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;

    fn reader() -> CloudII {
        CloudII { window: Duration::from_millis(30) }
    }

    /// Dongle avec casque allumé : `level` et état de charge `state` (octet 4, `None` : pas de
    /// réponse à cette question).
    fn dongle(level: u8, state: Option<u8>) -> Fake {
        Fake::new(move |w| {
            assert_eq!(w.len(), REQUEST_LEN);
            let mut r = vec![0; 20];
            r[..4].copy_from_slice(&w[..4]);
            match (w[3], state) {
                (CMD_LEVEL, _) => r[LEVEL_INDEX] = level,
                (CMD_CHARGING, Some(state)) => r[CHARGING_INDEX] = state,
                _ => return vec![],
            }
            // Un rapport étranger glissé avant la réponse.
            vec![vec![0x06, 0xFF, 0xBB, 0x7F, 0x01], r]
        })
    }

    #[test]
    fn reads_level_and_charging() {
        let mut dev = dongle(64, Some(0));
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 64, charging: false });
        assert_eq!(&dev.writes[0][..5], &[0x06, 0xFF, 0xBB, 0x02, 0x00]);
        assert_eq!(&dev.writes[1][..4], &[0x06, 0xFF, 0xBB, 0x03]);
        let mut dev = dongle(64, Some(1));
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 64, charging: true });
    }

    #[test]
    fn headset_off_when_the_dongle_is_silent() {
        let mut dev = Fake::new(|_| vec![]);
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn level_above_100_is_refused() {
        assert!(reader().read(&mut dongle(0xFF, Some(0))).is_err());
    }

    #[test]
    fn short_reply_is_refused() {
        let mut dev = Fake::new(|w| vec![w[..4].to_vec()]);
        assert!(reader().read(&mut dev).is_err());
    }

    #[test]
    fn unanswered_charging_question_means_not_charging() {
        assert_eq!(reader().read(&mut dongle(50, None)).unwrap(), Status::Battery { level: 50, charging: false });
    }

    #[test]
    fn probe_takes_the_ff90_collection_only() {
        let d = HidInfo { vid: HP_VID, pid: 0x018B, usage_page: 0xFF90, usage: 0x0303, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("HyperX Cloud II Wireless"));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0696, interface: 0, ..d }), Some("HyperX Cloud II Wireless"));
        for other in [(0x000C, 0x0001), (0xFF00, 0x0303), (0xFFC0, 0x0303), (0xFF90, 0x0001)] {
            assert_eq!((FAMILY.probe)(&HidInfo { usage_page: other.0, usage: other.1, ..d }), None);
        }
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x05B7, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { vid: 0x0951, ..d }), None);
        assert_eq!(super::super::find(&d).map(|(f, _)| f.name), Some(FAMILY.name));
    }
}
