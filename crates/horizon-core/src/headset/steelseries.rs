//! Casques sans fil SteelSeries (Arctis Nova 7, Nova 5, 7+, GameBuds, anciens Arctis, bases
//! Nova Pro Wireless) par leur récepteur USB, sans SteelSeries GG (qui peut tourner en même temps).
//!
//! Protocole repris de Halo Battery (`providers/steelseries.py`, licence MIT), lui-même tiré de
//! Sapd/HeadsetControl (liste des modèles, mises en page des réponses) et, pour les GameBuds X,
//! de la lecture de SteelSeries GG par Arctis-Sound-Manager. Les souris du même fichier (Rival 3,
//! Aerox) ne sont pas portées : ce module ne lit que des casques.
//!
//! Récepteurs « b0 » (`MODELS`) :
//!  * collection constructeur 0xFFC0/0x0001 (interface 3 du récepteur) ; requête `00 b0` ;
//!  * seule une réponse qui commence par `b0` est retenue : un autre rapport lu comme un niveau
//!    donnerait n'importe quoi (`01 00 63 02` lu comme 99 %) ;
//!  * Nova 7 : `b0 <liaison> <niveau> <état>` ; liaison 03 = casque relié (02 = non relié, le
//!    récepteur répète alors le dernier niveau) ; état 00 = éteint ou hors de portée, 01 / 02 =
//!    en charge, 03 = sur batterie ; niveau 0..100, ou 0..4 sur le micrologiciel d'origine ;
//!  * Nova 5 / Nova 3P : `b0 <état> <?> <niveau 0..100> <charge>` ; état 02 = éteint, charge 01 ;
//!  * Arctis 7+ : `b0 <état> <niveau 0..4> <charge>` ; état 01 = éteint, charge 01 ;
//!  * GameBuds : `b0 <?> <?> <état g> <état d> <% g> <% d>` ; état 03 = écouteur sorti du boîtier
//!    (un écouteur rangé annonce 0 % : il est ignoré).
//!
//! Anciens Arctis et bases Nova Pro (`CLASSIC`), chacun sa requête, sa collection et l'écho
//! attendu en tête de réponse :
//!  * Arctis 1 / 7X / 7P : interface 3, page 0xFF43, `06 12` → `06 12 <éteint=01> <niveau>` ;
//!  * Arctis 7 (2018) : interface 5, `06 14` → `06 14 <03 = relié>` puis `06 18` → `06 18 <niveau>` ;
//!  * Arctis 7 (2017) / Pro Wireless 2019 : interface 5, `06 18` → `06 18 <niveau, 0 = éteint>` ;
//!  * Arctis 9 : interface 0, `00 20` → `aa <01 = relié> <?> <tension 100..154> <charge>` ;
//!  * Arctis Pro Wireless : interface 0, `41 aa` → `<02 éteint | 04 relié>` puis `40 aa` →
//!    `<niveau 0..4>` ;
//!  * Nova Pro Wireless (et X) : interfaces 3 ou 4, `06 b0` → octet 6 = niveau en neuf paliers
//!    (0..8), octet 15 = état (01 éteint, 02 en charge sur le câble, 08 sur batterie). Halo
//!    affiche ces paliers comme « environ NN % » ; ici le pourcentage arrondi est rendu tel quel.
//!
//! Interfaces sans page imposée : toute page constructeur (>= 0xFF00). Seules des requêtes de
//! lecture sont envoyées.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x1038;
/// Collection des récepteurs « b0 ».
const VENDOR_PAGE: u16 = 0xFFC0;
const VENDOR_USAGE: u16 = 0x0001;
/// Début des pages constructeur.
const ANY_VENDOR_PAGE: u16 = 0xFF00;
const REPLY: u8 = 0xB0;
const READ_MS: i32 = 100;
/// Attente maximale d'une réponse (une seconde, comme Halo).
const WINDOW: Duration = Duration::from_secs(1);

/// Mise en page de la réponse `b0` d'un récepteur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Nova 7 : niveau en pourcentage.
    Nova7,
    /// Nova 7, micrologiciel d'origine : niveau en quatre paliers.
    Nova7Steps,
    Nova5,
    Arctis7Plus,
    GameBuds,
}

/// Casques sur récepteur « b0 » : identifiant produit, nom affiché, mise en page.
const MODELS: &[(u16, &str, Layout)] = &[
    (0x22A1, "SteelSeries Arctis Nova 7", Layout::Nova7),
    (0x2202, "SteelSeries Arctis Nova 7", Layout::Nova7Steps),
    (0x227E, "SteelSeries Arctis Nova 7 Gen 2", Layout::Nova7),
    (0x2206, "SteelSeries Arctis Nova 7x", Layout::Nova7Steps),
    (0x2258, "SteelSeries Arctis Nova 7x", Layout::Nova7),
    (0x229E, "SteelSeries Arctis Nova 7x", Layout::Nova7),
    (0x22AD, "SteelSeries Arctis Nova 7x", Layout::Nova7),
    (0x22A4, "SteelSeries Arctis Nova 7X", Layout::Nova7Steps),
    (0x22A5, "SteelSeries Arctis Nova 7X", Layout::Nova7),
    (0x223A, "SteelSeries Arctis Nova 7 Diablo IV", Layout::Nova7Steps),
    (0x22A9, "SteelSeries Arctis Nova 7 Diablo IV", Layout::Nova7),
    (0x227A, "SteelSeries Arctis Nova 7 WoW Edition", Layout::Nova7Steps),
    (0x2232, "SteelSeries Arctis Nova 5", Layout::Nova5),
    (0x2253, "SteelSeries Arctis Nova 5X", Layout::Nova5),
    (0x220A, "SteelSeries Arctis Nova 7P", Layout::Nova7Steps),
    (0x22A7, "SteelSeries Arctis Nova 7P", Layout::Nova7),
    (0x2298, "SteelSeries Arctis Nova 7P", Layout::Nova7),
    (0x2269, "SteelSeries Arctis Nova 3P Wireless", Layout::Nova5),
    (0x226D, "SteelSeries Arctis Nova 3X Wireless", Layout::Nova5),
    (0x220E, "SteelSeries Arctis 7+", Layout::Arctis7Plus),
    (0x2212, "SteelSeries Arctis 7+ PS5", Layout::Arctis7Plus),
    (0x2216, "SteelSeries Arctis 7+ Xbox", Layout::Arctis7Plus),
    (0x2236, "SteelSeries Arctis 7+ Destiny", Layout::Arctis7Plus),
    (0x230A, "SteelSeries Arctis GameBuds", Layout::GameBuds),
    (0x2317, "SteelSeries Arctis GameBuds X", Layout::GameBuds),
];

/// Échange propre à un ancien Arctis ou à une base Nova Pro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Classic {
    Arctis1,
    Arctis7Of2018,
    Arctis7,
    Arctis9,
    ProWireless,
    NovaPro,
}

/// Casque à échange propre.
struct ClassicModel {
    pid: u16,
    name: &'static str,
    /// Interfaces USB acceptées.
    interfaces: &'static [i32],
    /// Page d'usage imposée ; `None` : toute page constructeur.
    page: Option<u16>,
    exchange: Classic,
}

const fn classic(pid: u16, name: &'static str, interfaces: &'static [i32], page: Option<u16>, exchange: Classic) -> ClassicModel {
    ClassicModel { pid, name, interfaces, page, exchange }
}

const CLASSIC: &[ClassicModel] = &[
    classic(0x12B3, "SteelSeries Arctis 1 Wireless", &[3], Some(0xFF43), Classic::Arctis1),
    classic(0x12B6, "SteelSeries Arctis 1 Wireless Xbox", &[3], Some(0xFF43), Classic::Arctis1),
    classic(0x12D7, "SteelSeries Arctis 7X", &[3], Some(0xFF43), Classic::Arctis1),
    classic(0x12D5, "SteelSeries Arctis 7P", &[3], Some(0xFF43), Classic::Arctis1),
    classic(0x12AD, "SteelSeries Arctis 7", &[5], None, Classic::Arctis7Of2018),
    classic(0x1260, "SteelSeries Arctis 7", &[5], None, Classic::Arctis7),
    classic(0x1252, "SteelSeries Arctis Pro Wireless 2019", &[5], None, Classic::Arctis7),
    classic(0x12C2, "SteelSeries Arctis 9", &[0], None, Classic::Arctis9),
    classic(0x1290, "SteelSeries Arctis Pro Wireless", &[0], None, Classic::ProWireless),
    classic(0x12E0, "SteelSeries Arctis Nova Pro Wireless", &[3, 4], None, Classic::NovaPro),
    classic(0x12E5, "SteelSeries Arctis Nova Pro Wireless X", &[3, 4], None, Classic::NovaPro),
];

/// Ce qu'il faut demander au casque de cet identifiant produit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exchange {
    Dongle(Layout),
    Classic(Classic),
}

fn probe(d: &HidInfo) -> Option<&'static str> {
    if d.vid != VID {
        return None;
    }
    if let Some(&(_, name, _)) = MODELS.iter().find(|m| m.0 == d.pid) {
        return (d.usage_page == VENDOR_PAGE && d.usage == VENDOR_USAGE).then_some(name);
    }
    let c = CLASSIC.iter().find(|c| c.pid == d.pid)?;
    let page_ok = match c.page {
        Some(p) => d.usage_page == p,
        None => d.usage_page >= ANY_VENDOR_PAGE,
    };
    (c.interfaces.contains(&d.interface) && page_ok).then_some(c.name)
}

fn exchange_of(pid: u16) -> Option<Exchange> {
    MODELS
        .iter()
        .find(|m| m.0 == pid)
        .map(|m| Exchange::Dongle(m.2))
        .or_else(|| CLASSIC.iter().find(|c| c.pid == pid).map(|c| Exchange::Classic(c.exchange)))
}

pub const FAMILY: Family = Family {
    name: "SteelSeries Arctis",
    vids: &[VID],
    probe,
    new: |d: &HidInfo| {
        let exchange = exchange_of(d.pid).expect("identifiant produit reconnu par probe");
        Box::new(SteelSeries { exchange, window: WINDOW })
    },
};

/// Décode la réponse `b0` d'un récepteur ; `None` si elle est illisible.
fn parse(layout: Layout, r: &[u8]) -> Option<Status> {
    if r.first() != Some(&REPLY) {
        return None;
    }
    let battery = |level: u8, charging: bool| Some(Status::Battery { level, charging });
    match layout {
        Layout::Nova7 | Layout::Nova7Steps => {
            if r.len() < 4 {
                return None;
            }
            if r[1] != 0x03 || r[3] == 0x00 {
                return Some(Status::Off);
            }
            let level = if layout == Layout::Nova7 { r[2].min(100) } else { r[2].min(4) * 25 };
            battery(level, matches!(r[3], 1 | 2))
        }
        Layout::Nova5 => {
            if r.len() < 5 {
                return None;
            }
            if r[1] == 0x02 {
                return Some(Status::Off);
            }
            battery(r[3].min(100), r[4] == 0x01)
        }
        Layout::Arctis7Plus => {
            if r.len() < 4 {
                return None;
            }
            if r[1] == 0x01 {
                return Some(Status::Off);
            }
            battery(r[2].min(4) * 25, r[3] == 0x01)
        }
        Layout::GameBuds => {
            if r.len() < 7 {
                return None;
            }
            // Écouteurs sortis du boîtier ; tous deux rangés : rien à montrer.
            let out = (0..2).filter(|&i| r[3 + i] == 0x03).map(|i| r[5 + i]).min();
            match out {
                Some(level) => battery(level.min(100), false),
                None => Some(Status::Off),
            }
        }
    }
}

/// Lecteur de batterie d'un casque SteelSeries.
pub struct SteelSeries {
    exchange: Exchange,
    window: Duration,
}

impl SteelSeries {
    /// Envoie `request` et rend la première réponse que `accept` reconnaît, `None` après
    /// `window` sans elle. Les autres rapports (réponses à SteelSeries GG…) sont ignorés.
    fn ask(&self, link: &mut dyn Link, request: &[u8], accept: impl Fn(&[u8]) -> bool) -> anyhow::Result<Option<Vec<u8>>> {
        link.write(request)?;
        let end = Instant::now() + self.window;
        while Instant::now() < end {
            let r = link.read(READ_MS)?;
            if !r.is_empty() && accept(&r) {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// Comme `ask`, mais une absence de réponse est une erreur.
    fn ask_or_fail(&self, link: &mut dyn Link, request: &[u8], accept: impl Fn(&[u8]) -> bool) -> anyhow::Result<Vec<u8>> {
        self.ask(link, request, accept)?
            .ok_or_else(|| anyhow::anyhow!("pas de réponse à la requête {:02x?}", &request[..request.len().min(2)]))
    }

    fn dongle(&self, link: &mut dyn Link, layout: Layout) -> anyhow::Result<Status> {
        let r = self.ask_or_fail(link, &[0x00, REPLY], |r| r[0] == REPLY)?;
        parse(layout, &r).ok_or_else(|| anyhow::anyhow!("réponse b0 illisible : {r:02x?}"))
    }

    fn classic(&self, link: &mut dyn Link, kind: Classic) -> anyhow::Result<Status> {
        let echo = |a: u8, b: u8| move |r: &[u8]| r.len() >= 4 && r[0] == a && r[1] == b;
        let battery = |level: u8, charging: bool| Ok(Status::Battery { level, charging });
        match kind {
            Classic::Arctis1 => {
                let r = self.ask_or_fail(link, &[0x06, 0x12], echo(0x06, 0x12))?;
                if r[2] == 0x01 {
                    return Ok(Status::Off);
                }
                battery(r[3].min(100), false)
            }
            Classic::Arctis7Of2018 => {
                let r = self.ask_or_fail(link, &[0x06, 0x14], echo(0x06, 0x14))?;
                if r[2] != 0x03 {
                    return Ok(Status::Off);
                }
                let r = self.ask_or_fail(link, &[0x06, 0x18], echo(0x06, 0x18))?;
                battery(r[2].min(100), false)
            }
            Classic::Arctis7 => {
                let r = self.ask_or_fail(link, &[0x06, 0x18], echo(0x06, 0x18))?;
                if r[2] == 0x00 {
                    return Ok(Status::Off);
                }
                battery(r[2].min(100), false)
            }
            Classic::Arctis9 => {
                let r = self.ask_or_fail(link, &[0x00, 0x20], |r| r.len() >= 5 && matches!(r[0], 0xAA | 0x55))?;
                if r[0] != 0xAA || r[1] != 0x01 {
                    return Ok(Status::Off);
                }
                // Tension brute de 100 à 154 ramenée à un pourcentage.
                let raw = u16::from(r[3].clamp(100, 154));
                battery(((raw - 100) * 100 / 54) as u8, r[4] == 0x01)
            }
            Classic::ProWireless => {
                let request = |op: u8| [&[op, 0xAA][..], &[0; 29]].concat();
                let r = self.ask_or_fail(link, &request(0x41), |r| matches!(r[0], 0x02 | 0x04))?;
                if r[0] == 0x02 {
                    return Ok(Status::Off);
                }
                let r = self.ask_or_fail(link, &request(0x40), |r| r[0] <= 4)?;
                battery(r[0] * 25, false)
            }
            Classic::NovaPro => {
                let r = self.ask_or_fail(link, &[0x06, REPLY], |r| r.len() >= 16 && matches!(r[15], 0x01 | 0x02 | 0x08))?;
                if r[15] == 0x01 {
                    return Ok(Status::Off);
                }
                if r[6] > 8 {
                    anyhow::bail!("palier de batterie illisible : {r:02x?}");
                }
                battery((u16::from(r[6]) * 100 / 8) as u8, r[15] == 0x02)
            }
        }
    }
}

impl Driver for SteelSeries {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        match self.exchange {
            Exchange::Dongle(layout) => self.dongle(link, layout),
            Exchange::Classic(kind) => self.classic(link, kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux récepteur : chaque requête reçue met en file les rapports que `answer` rend.
    type Answer = Box<dyn Fn(&[u8]) -> Vec<Vec<u8>>>;

    struct Fake {
        answer: Answer,
        queue: VecDeque<Vec<u8>>,
        writes: Vec<Vec<u8>>,
    }

    impl Fake {
        fn new(answer: impl Fn(&[u8]) -> Vec<Vec<u8>> + 'static) -> Fake {
            Fake { answer: Box::new(answer), queue: VecDeque::new(), writes: Vec::new() }
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            self.writes.push(w.to_vec());
            self.queue.extend((self.answer)(w));
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn reader(pid: u16) -> SteelSeries {
        SteelSeries { exchange: exchange_of(pid).unwrap(), window: Duration::from_millis(30) }
    }

    fn info(pid: u16, usage_page: u16, usage: u16, interface: i32) -> HidInfo {
        HidInfo { vid: VID, pid, usage_page, usage, interface }
    }

    /// Lecture d'un récepteur « b0 » qui répond `reply` (précédé d'un rapport étranger).
    fn dongle(pid: u16, reply: &'static [u8]) -> anyhow::Result<Status> {
        let mut dev = Fake::new(move |w| {
            assert_eq!(w, [0x00, 0xB0]);
            vec![vec![0x01, 0x00, 0x63, 0x02], reply.to_vec()]
        });
        reader(pid).read(&mut dev)
    }

    fn battery(level: u8, charging: bool) -> Status {
        Status::Battery { level, charging }
    }

    #[test]
    fn nova7_level_and_charging() {
        assert_eq!(dongle(0x22A1, &[0xB0, 0x03, 0x5C, 0x03]).unwrap(), battery(92, false));
        assert_eq!(dongle(0x22A1, &[0xB0, 0x03, 0x5C, 0x01]).unwrap(), battery(92, true));
        assert_eq!(dongle(0x22A1, &[0xB0, 0x03, 0x5C, 0x02]).unwrap(), battery(92, true));
        assert_eq!(dongle(0x22A1, &[0xB0, 0x03, 0xC8, 0x03]).unwrap(), battery(100, false));
    }

    #[test]
    fn nova7_off_when_unlinked_or_state_zero() {
        assert_eq!(dongle(0x22A1, &[0xB0, 0x02, 0x5C, 0x03]).unwrap(), Status::Off);
        assert_eq!(dongle(0x22A1, &[0xB0, 0x03, 0x5C, 0x00]).unwrap(), Status::Off);
    }

    #[test]
    fn nova7_original_firmware_in_four_steps() {
        assert_eq!(dongle(0x2202, &[0xB0, 0x03, 0x03, 0x03]).unwrap(), battery(75, false));
        assert_eq!(dongle(0x2202, &[0xB0, 0x03, 0x09, 0x01]).unwrap(), battery(100, true));
    }

    #[test]
    fn nova5_layout() {
        assert_eq!(dongle(0x2232, &[0xB0, 0x01, 0x00, 0x4B, 0x01]).unwrap(), battery(75, true));
        assert_eq!(dongle(0x2269, &[0xB0, 0x01, 0x00, 0x4B, 0x00]).unwrap(), battery(75, false));
        assert_eq!(dongle(0x2232, &[0xB0, 0x02, 0x00, 0x4B, 0x00]).unwrap(), Status::Off);
    }

    #[test]
    fn arctis7_plus_layout() {
        assert_eq!(dongle(0x220E, &[0xB0, 0x00, 0x02, 0x01]).unwrap(), battery(50, true));
        assert_eq!(dongle(0x220E, &[0xB0, 0x01, 0x02, 0x00]).unwrap(), Status::Off);
    }

    #[test]
    fn gamebuds_lowest_bud_out_of_the_case() {
        assert_eq!(dongle(0x230A, &[0xB0, 0, 0, 0x03, 0x03, 0x50, 0x40]).unwrap(), battery(64, false));
        assert_eq!(dongle(0x2317, &[0xB0, 0, 0, 0x03, 0x01, 0x50, 0x00]).unwrap(), battery(80, false));
        assert_eq!(dongle(0x230A, &[0xB0, 0, 0, 0x01, 0x01, 0x00, 0x00]).unwrap(), Status::Off);
    }

    #[test]
    fn short_b0_reply_is_unreadable() {
        assert!(dongle(0x22A1, &[0xB0, 0x03]).is_err());
        assert!(dongle(0x2232, &[0xB0, 0x01, 0x00, 0x4B]).is_err());
        assert!(dongle(0x230A, &[0xB0, 0, 0, 0x03, 0x03, 0x50]).is_err());
    }

    #[test]
    fn foreign_reports_alone_give_no_reading() {
        let mut dev = Fake::new(|_| vec![vec![0x01, 0x00, 0x63, 0x02], vec![0xAA, 0x50]]);
        assert!(reader(0x22A1).read(&mut dev).is_err());
        assert_eq!(dev.writes, vec![vec![0x00, 0xB0]]);
    }

    #[test]
    fn silent_receiver_is_an_error() {
        let mut dev = Fake::new(|_| vec![]);
        assert!(reader(0x22A1).read(&mut dev).is_err());
        assert!(reader(0x12E0).read(&mut dev).is_err());
    }

    #[test]
    fn arctis1_exchange() {
        let mut dev = Fake::new(|w| {
            assert_eq!(w, [0x06, 0x12]);
            vec![vec![0x06, 0x13, 0x00, 0x10], vec![0x06, 0x12, 0x00, 0x50]]
        });
        assert_eq!(reader(0x12B3).read(&mut dev).unwrap(), battery(80, false));
        let mut dev = Fake::new(|_| vec![vec![0x06, 0x12, 0x01, 0x50]]);
        assert_eq!(reader(0x12D5).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn arctis7_2018_asks_link_then_level() {
        let mut dev = Fake::new(|w| match w {
            [0x06, 0x14] => vec![vec![0x06, 0x14, 0x03, 0x00]],
            [0x06, 0x18] => vec![vec![0x06, 0x18, 0x2A, 0x00]],
            _ => panic!("requête inattendue {w:02x?}"),
        });
        assert_eq!(reader(0x12AD).read(&mut dev).unwrap(), battery(42, false));
        assert_eq!(dev.writes.len(), 2);
        let mut dev = Fake::new(|_| vec![vec![0x06, 0x14, 0x01, 0x00]]);
        assert_eq!(reader(0x12AD).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn arctis7_level_zero_is_off() {
        let mut dev = Fake::new(|_| vec![vec![0x06, 0x18, 0x37, 0x00]]);
        assert_eq!(reader(0x1260).read(&mut dev).unwrap(), battery(55, false));
        let mut dev = Fake::new(|_| vec![vec![0x06, 0x18, 0x00, 0x00]]);
        assert_eq!(reader(0x1252).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn arctis9_voltage_scale() {
        let mut dev = Fake::new(|w| {
            assert_eq!(w, [0x00, 0x20]);
            vec![vec![0xAA, 0x01, 0x00, 127, 0x01]]
        });
        assert_eq!(reader(0x12C2).read(&mut dev).unwrap(), battery(50, true));
        let mut dev = Fake::new(|_| vec![vec![0xAA, 0x01, 0x00, 200, 0x00]]);
        assert_eq!(reader(0x12C2).read(&mut dev).unwrap(), battery(100, false));
        let mut dev = Fake::new(|_| vec![vec![0xAA, 0x03, 0x00, 127, 0x00]]);
        assert_eq!(reader(0x12C2).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn pro_wireless_two_requests() {
        let mut dev = Fake::new(|w| {
            assert_eq!(w.len(), 31);
            match w[..2] {
                [0x41, 0xAA] => vec![vec![0x04]],
                [0x40, 0xAA] => vec![vec![0x03]],
                _ => panic!("requête inattendue"),
            }
        });
        assert_eq!(reader(0x1290).read(&mut dev).unwrap(), battery(75, false));
        let mut dev = Fake::new(|_| vec![vec![0x02]]);
        assert_eq!(reader(0x1290).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn nova_pro_nine_steps_and_state() {
        fn reply(step: u8, state: u8) -> Vec<u8> {
            let mut r = vec![0u8; 16];
            r[6] = step;
            r[15] = state;
            r
        }
        let mut dev = Fake::new(|w| {
            assert_eq!(w, [0x06, 0xB0]);
            vec![reply(4, 0x05), reply(4, 0x08)]
        });
        assert_eq!(reader(0x12E0).read(&mut dev).unwrap(), battery(50, false));
        let mut dev = Fake::new(|_| vec![reply(7, 0x02)]);
        assert_eq!(reader(0x12E5).read(&mut dev).unwrap(), battery(87, true));
        let mut dev = Fake::new(|_| vec![reply(0, 0x01)]);
        assert_eq!(reader(0x12E0).read(&mut dev).unwrap(), Status::Off);
        let mut dev = Fake::new(|_| vec![reply(9, 0x08)]);
        assert!(reader(0x12E0).read(&mut dev).is_err());
    }

    #[test]
    fn probe_b0_receivers_on_their_vendor_collection_only() {
        let d = info(0x22A1, 0xFFC0, 0x0001, 3);
        assert_eq!(probe(&d), Some("SteelSeries Arctis Nova 7"));
        assert_eq!(
            super::super::find(&d).map(|(f, n)| (f.name, n)),
            Some(("SteelSeries Arctis", "SteelSeries Arctis Nova 7"))
        );
        assert_eq!(probe(&HidInfo { usage: 0x0002, ..d }), None);
        assert_eq!(probe(&HidInfo { usage_page: 0x000C, ..d }), None);
        assert_eq!(probe(&HidInfo { vid: 0x046D, ..d }), None);
        // Souris (Aerox 3) et Nova Elite : pas pour ce module.
        assert_eq!(probe(&info(0x1838, 0xFFC0, 0x0001, 3)), None);
        assert_eq!(probe(&info(0x2244, 0xFFC0, 0x0001, 3)), None);
    }

    #[test]
    fn probe_classic_models_on_their_interface_and_page() {
        assert_eq!(probe(&info(0x12B3, 0xFF43, 0x0202, 3)), Some("SteelSeries Arctis 1 Wireless"));
        assert_eq!(probe(&info(0x12B3, 0xFF00, 0x0001, 3)), None);
        assert_eq!(probe(&info(0x12B3, 0xFF43, 0x0202, 0)), None);
        assert_eq!(probe(&info(0x12AD, 0xFF00, 0x0001, 5)), Some("SteelSeries Arctis 7"));
        assert_eq!(probe(&info(0x12AD, 0x000C, 0x0001, 5)), None);
        assert_eq!(probe(&info(0x12C2, 0xFF00, 0x0001, 0)), Some("SteelSeries Arctis 9"));
        assert_eq!(probe(&info(0x12E0, 0xFFC0, 0x0001, 4)), Some("SteelSeries Arctis Nova Pro Wireless"));
        assert_eq!(probe(&info(0x12E5, 0xFF00, 0x0001, 3)), Some("SteelSeries Arctis Nova Pro Wireless X"));
        assert_eq!(probe(&info(0x12E0, 0xFFC0, 0x0001, 5)), None);
    }

    #[test]
    fn every_model_has_an_exchange_and_a_unique_id() {
        let mut ids: Vec<u16> = MODELS.iter().map(|m| m.0).chain(CLASSIC.iter().map(|c| c.pid)).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "identifiant produit en double");
        for id in ids {
            assert!(exchange_of(id).is_some());
        }
        for (_, name, _) in MODELS {
            assert!(name.starts_with("SteelSeries "));
        }
        for c in CLASSIC {
            assert!(c.name.starts_with("SteelSeries "));
        }
    }
}
