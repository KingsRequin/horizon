//! Audeze Maxwell (et Maxwell 2) par le récepteur 2,4 GHz ou par le câble USB-C, sans Audeze HQ.
//!
//! Protocole repris de Halo Battery (`providers/audeze.py`, licence MIT), lui-même tiré de
//! HeadsetControl (Sapd/HeadsetControl, `lib/devices/audeze_maxwell.hpp`, `UNIQUE_REQUESTS`) :
//!  * collection constructeur 0xFF13/0x0001 ; requête = rapport de sortie 0x06 de 62 octets,
//!    réponse = rapport d'entrée 0x07 de 62 octets ;
//!  * la réponse est un tampon tournant des dernières réponses du récepteur : la marque de
//!    batterie `d6 0c 00 00 <pour cent>` se trouve dans une trame ou une autre, on prend la plus
//!    récente qui la porte ;
//!  * un seul paquet (attribut 0x0CD6) suffit d'ordinaire ; sans marque, la séquence de
//!    HeadsetControl sert de secours, et si le paquet seul échoue une fois, on passe directement
//!    à la séquence les fois suivantes ;
//!  * pas d'indication de charge : le casque charge toujours sur le câble USB-C, donc le câble
//!    qui répond vaut « en charge » ;
//!  * des réponses qui ne sont que l'écho des requêtes (cinq trames ou plus, vides après l'en-tête)
//!    deux lectures de suite : le récepteur ne transmet plus rien au casque (le débrancher et le
//!    rebrancher) ;
//!  * 0 % dans les 90 premières secondes après l'allumage : pas encore mesuré.
//!
//! Écarts avec Halo :
//!  * Halo lit la réponse par une requête GET_REPORT (`get_input_report`) ; `Link` n'offre que la
//!    lecture des rapports d'entrée qui arrivent, utilisée ici (seuls les rapports 0x07 comptent) ;
//!  * le paquet `06 07 00 05 5A 03 00 07 1C` de la séquence n'est pas envoyé : c'est le seul sans
//!    l'octet 0x80 des lectures, et Halo lui-même ne sait pas si c'est une lecture ;
//!  * Halo reconnaît le casque éteint au nom du produit (« Audeze Maxwell Dongle ») que
//!    `HidInfo` ne porte pas : le récepteur garde alors le dernier niveau, rendu tel quel.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x3329;
/// Modèles connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[
    (0x4B19, "Audeze Maxwell"),
    (0x4B18, "Audeze Maxwell (Xbox)"),
    (0x4B1A, "Audeze Maxwell (USB cable)"),
    (0x4B1E, "Audeze Maxwell (Xbox, USB cable)"),
    (0x4B29, "Audeze Maxwell 2"),
    (0x4B28, "Audeze Maxwell 2 (Xbox)"),
];
/// Câble USB-C : le casque y est toujours en charge.
const CABLE_PIDS: &[u16] = &[0x4B1A, 0x4B1E];
const USAGE_PAGE: u16 = 0xFF13;
const USAGE: u16 = 0x0001;

pub const FAMILY: Family = Family {
    name: "Audeze Maxwell",
    vids: &[VID],
    probe: |d| {
        let name = PRODUCTS.iter().find(|(pid, _)| d.vid == VID && *pid == d.pid)?.1;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |d: &HidInfo| Box::new(Maxwell::new(CABLE_PIDS.contains(&d.pid))),
};

const MSG_SIZE: usize = 62;
const REPORT_IN: u8 = 0x07;
const MARKER: [u8; 4] = [0xD6, 0x0C, 0x00, 0x00];
/// Paquet de la batterie seule (attribut 0x0CD6).
const BATTERY_QUERY: &[u8] = &[0x06, 0x07, 0x80, 0x05, 0x5A, 0x03, 0x00, 0xD6, 0x0C];
/// Séquence de HeadsetControl (initialisation puis état), moins le paquet à l'octet 0x00.
const REQUESTS: &[&[u8]] = &[
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x20],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x25],
    &[0x06, 0x07, 0x80, 0x05, 0x5A, 0x03, 0x00, 0x07, 0x1C],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x28],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x83, 0x2C, 0x01],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x83, 0x2C, 0x07],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x2D],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x2C],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x83, 0x2C, 0x0B],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x24],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x2F],
    BATTERY_QUERY,
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x22],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x83, 0x2C, 0x0B],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x24],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x01, 0x09, 0x2C],
    &[0x06, 0x08, 0x80, 0x05, 0x5A, 0x04, 0x00, 0x83, 0x2C, 0x07],
];
/// Lectures supplémentaires après la dernière requête.
const EXTRA_READS: usize = 2;
/// Requêtes sans réponse (avant toute réponse) au-delà desquelles on abandonne.
const UNANSWERED_MAX: usize = 3;
const ECHO_MIN_FRAMES: usize = 5;
const STUCK_POLLS: u32 = 2;
const READ_MS: i32 = 100;
/// Rapports lus au plus pour trouver un rapport 0x07 (les autres sont ignorés).
const READS_PER_FRAME: usize = 8;

fn packet(values: &[u8]) -> Vec<u8> {
    let mut data = vec![0; MSG_SIZE];
    data[..values.len()].copy_from_slice(values);
    data
}

/// Niveau qui suit la première marque valable de la trame.
fn find_level(frame: &[u8]) -> Option<u8> {
    let mut start = 0;
    loop {
        let i = start + frame.get(start..)?.windows(MARKER.len()).position(|w| w == MARKER)?;
        let level = *frame.get(i + 4)?;
        if level <= 100 {
            return Some(level);
        }
        start = i + 1;
    }
}

/// Niveau de la trame la plus récente qui en porte un.
fn newest_level(frames: &[Vec<u8>]) -> Option<u8> {
    frames.iter().rev().find_map(|f| find_level(f))
}

/// Toutes les réponses ne sont que l'écho vide des requêtes.
fn echo_only(frames: &[Vec<u8>]) -> bool {
    frames.len() >= ECHO_MIN_FRAMES
        && frames.iter().all(|f| f.len() > 3 && f[0] == REPORT_IN && f[3..].iter().all(|&b| b == 0))
}

/// Maxwell : un lecteur par collection (récepteur ou câble).
pub struct Maxwell {
    cable: bool,
    /// Attente entre une requête et la lecture de sa réponse.
    delay: Duration,
    /// Délai après l'allumage pendant lequel 0 % veut dire « pas encore mesuré ».
    grace: Duration,
    /// Le paquet seul n'a rien donné : la séquence complète directement.
    short_useless: bool,
    /// Lectures de suite qui n'ont rendu que des échos.
    echo_polls: u32,
    /// Premier niveau lu depuis l'allumage.
    up_since: Option<Instant>,
}

#[derive(Debug)]
enum Read {
    Level(u8),
    /// Réponses sans marque (`true` : rien que des échos).
    NoMarker(bool),
    Silent,
}

impl Maxwell {
    fn new(cable: bool) -> Maxwell {
        Maxwell {
            cable,
            delay: Duration::from_millis(60),
            grace: Duration::from_secs(90),
            short_useless: false,
            echo_polls: 0,
            up_since: None,
        }
    }

    /// Prochain rapport d'entrée 0x07, `None` s'il n'en vient pas.
    fn frame(link: &mut dyn Link) -> anyhow::Result<Option<Vec<u8>>> {
        for _ in 0..READS_PER_FRAME {
            let r = link.read(READ_MS)?;
            if r.is_empty() {
                return Ok(None);
            }
            if r.len() > 3 && r[0] == REPORT_IN {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// Envoie les requêtes et recueille les trames ; `false` si la collection ne répond pas.
    fn sequence(&self, link: &mut dyn Link, requests: &[&[u8]]) -> anyhow::Result<(Vec<Vec<u8>>, bool)> {
        let mut frames = Vec::new();
        let mut unanswered = 0;
        for req in requests {
            if link.write(&packet(req)).is_err() {
                break;
            }
            std::thread::sleep(self.delay);
            match Self::frame(link)? {
                Some(f) => frames.push(f),
                None if !frames.is_empty() => {}
                None => {
                    unanswered += 1;
                    if unanswered >= UNANSWERED_MAX {
                        return Ok((frames, false));
                    }
                }
            }
        }
        for _ in 0..EXTRA_READS {
            std::thread::sleep(self.delay);
            if let Some(f) = Self::frame(link)? {
                frames.push(f);
            }
        }
        Ok((frames, true))
    }

    fn query(&mut self, link: &mut dyn Link) -> anyhow::Result<Read> {
        let short = !self.short_useless;
        let (mut frames, mut answered, mut level) = (Vec::new(), true, None);
        if short {
            (frames, answered) = self.sequence(link, &[BATTERY_QUERY])?;
            level = newest_level(&frames);
        }
        if level.is_none() && answered {
            let (more, _) = self.sequence(link, REQUESTS)?;
            level = newest_level(&more);
            frames.extend(more);
        }
        if level.is_some() {
            self.short_useless = false;
        } else if short {
            self.short_useless = true;
        }
        Ok(match level {
            Some(l) => Read::Level(l),
            None if frames.is_empty() => Read::Silent,
            None => Read::NoMarker(echo_only(&frames)),
        })
    }
}

impl Driver for Maxwell {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let read = self.query(link)?;
        self.echo_polls = if matches!(read, Read::NoMarker(true)) { self.echo_polls + 1 } else { 0 };
        let level = match read {
            Read::Level(l) => l,
            _ if self.echo_polls >= STUCK_POLLS => {
                self.up_since = None;
                anyhow::bail!("le casque ne répond plus : débrancher le récepteur et le rebrancher")
            }
            Read::Silent => {
                self.up_since = None;
                anyhow::bail!("la collection ne répond pas")
            }
            Read::NoMarker(_) => {
                self.up_since = None;
                return Ok(Status::Off);
            }
        };
        let since = *self.up_since.get_or_insert_with(Instant::now);
        if level == 0 && since.elapsed() < self.grace {
            // Allumé depuis peu : niveau pas encore mesuré.
            return Ok(Status::Off);
        }
        Ok(Status::Battery { level, charging: self.cable })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux Maxwell : `battery` = la marque est dans la réponse au paquet seul ; `in_sequence` =
    /// elle n'arrive qu'avec la séquence ; `echo` = le récepteur ne renvoie que des échos.
    struct Fake {
        level: u8,
        battery: bool,
        in_sequence: bool,
        echo: bool,
        silent: bool,
        queue: VecDeque<Vec<u8>>,
        writes: Vec<Vec<u8>>,
    }

    impl Fake {
        fn new(level: u8) -> Fake {
            Fake { level, battery: true, in_sequence: false, echo: false, silent: false, queue: VecDeque::new(), writes: vec![] }
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), MSG_SIZE);
            assert_eq!(w[0], 0x06);
            assert_eq!(w[2], 0x80, "seulement les paquets de lecture");
            self.writes.push(w.to_vec());
            if self.silent {
                return Ok(());
            }
            let mut r = vec![0u8; MSG_SIZE];
            r[0] = REPORT_IN;
            let battery = w[7..9] == [0xD6, 0x0C];
            if !self.echo {
                r[1..4].copy_from_slice(&w[1..4]);
                // Le tampon tournant garde des réponses anciennes : un faux niveau plus loin.
                if battery && self.battery || self.in_sequence && self.writes.len() > 2 {
                    r[10..15].copy_from_slice(&[0xD6, 0x0C, 0x00, 0x00, self.level]);
                }
            }
            // Un autre rapport d'entrée (0x01) qui n'est pas le nôtre.
            self.queue.push_back(vec![0x01, 0xD6, 0x0C, 0x00, 0x00, 0x07]);
            self.queue.push_back(r);
            Ok(())
        }
        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn reader(cable: bool) -> Maxwell {
        Maxwell { delay: Duration::ZERO, grace: Duration::ZERO, ..Maxwell::new(cable) }
    }

    #[test]
    fn probe_takes_the_vendor_collection_of_known_models() {
        let d = HidInfo { vid: VID, pid: 0x4B19, usage_page: USAGE_PAGE, usage: USAGE, interface: 3 };
        assert_eq!((FAMILY.probe)(&d), Some("Audeze Maxwell"));
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x4B1A, ..d }), Some("Audeze Maxwell (USB cable)"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage: 2, ..d }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x4B00, ..d }), None);
    }

    #[test]
    fn single_packet_reads_the_level() {
        let mut dev = Fake::new(76);
        assert_eq!(reader(false).read(&mut dev).unwrap(), Status::Battery { level: 76, charging: false });
        assert_eq!(dev.writes.len(), 1);
    }

    #[test]
    fn cable_means_charging() {
        let mut dev = Fake::new(85);
        assert_eq!(reader(true).read(&mut dev).unwrap(), Status::Battery { level: 85, charging: true });
    }

    #[test]
    fn falls_back_to_the_sequence_and_then_skips_the_single_packet() {
        let mut m = reader(false);
        let mut dev = Fake::new(64);
        dev.battery = false;
        dev.in_sequence = true;
        assert_eq!(m.read(&mut dev).unwrap(), Status::Battery { level: 64, charging: false });
        assert_eq!(dev.writes.len(), 1 + REQUESTS.len());
        assert!(!m.short_useless, "le niveau est venu : le paquet seul sera retenté");
        // Sans niveau, la lecture suivante passe directement à la séquence.
        dev.in_sequence = false;
        dev.writes.clear();
        assert_eq!(m.read(&mut dev).unwrap(), Status::Off);
        assert!(m.short_useless);
        dev.writes.clear();
        let _ = m.read(&mut dev);
        assert_eq!(dev.writes.len(), REQUESTS.len());
    }

    #[test]
    fn zero_right_after_power_on_is_not_measured_yet() {
        let mut m = Maxwell { delay: Duration::ZERO, ..Maxwell::new(false) };
        let mut dev = Fake::new(0);
        assert_eq!(m.read(&mut dev).unwrap(), Status::Off);
        assert_eq!(reader(false).read(&mut dev).unwrap(), Status::Battery { level: 0, charging: false });
    }

    #[test]
    fn echoes_two_reads_in_a_row_mean_a_stuck_dongle() {
        let mut m = reader(false);
        let mut dev = Fake::new(50);
        dev.echo = true;
        assert_eq!(m.read(&mut dev).unwrap(), Status::Off);
        let e = m.read(&mut dev).unwrap_err();
        assert!(e.to_string().contains("débrancher"), "{e}");
    }

    #[test]
    fn silent_collection_is_an_error() {
        let mut dev = Fake::new(50);
        dev.silent = true;
        assert!(reader(false).read(&mut dev).is_err());
        assert_eq!(dev.writes.len(), 1 + UNANSWERED_MAX, "abandon après trois requêtes sans réponse");
    }

    #[test]
    fn marker_parsing() {
        assert_eq!(find_level(&[7, 0, 0xD6, 0x0C, 0, 0, 42]), Some(42));
        // Valeur hors échelle : la marque suivante.
        assert_eq!(find_level(&[0xD6, 0x0C, 0, 0, 200, 0xD6, 0x0C, 0, 0, 9]), Some(9));
        assert_eq!(find_level(&[7, 0xD6, 0x0C, 0, 0]), None);
        assert_eq!(find_level(&[]), None);
        assert_eq!(newest_level(&[vec![0xD6, 0x0C, 0, 0, 10], vec![7], vec![0xD6, 0x0C, 0, 0, 12]]), Some(12));
        assert_eq!(packet(BATTERY_QUERY).len(), MSG_SIZE);
        assert!(REQUESTS.iter().all(|r| r[2] == 0x80));
    }
}
