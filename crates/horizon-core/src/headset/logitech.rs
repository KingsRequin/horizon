//! Casques Logitech sans fil parlant HID++ 2.0 (G533, G535, G633, G635, G733, G933, G935,
//! G PRO, G PRO X, G PRO X 2 par leur dongle), à côté de G HUB.
//!
//! Protocole repris de Halo Battery (`providers/logitech.py`, licence MIT), lui-même appuyé sur
//! la documentation HID++ de Logitech, Solaar et HeadsetControl. Halo y lit aussi souris et
//! claviers (récepteurs Unifying, Bolt, Lightspeed) : seule la partie casques est portée ici.
//!  * collection : une seule collection « longue », appareil à l'emplacement 0xFF ; 0xFF43/0x0202
//!    pour la plupart des modèles, la collection grand public 0x000C/0x0001 pour le G535
//!    (HeadsetControl) ;
//!  * requête (rapport long, 20 octets) : `11 ff <index de fonction> <fonction << 4 | swid>
//!    <paramètres…>` ; réponse : `11 ff <index> <fonction | swid> <données…>` ;
//!  * erreur : `10 ff 8f <index> <fonction | swid> <code>` ou `11 ff ff <index> <fonction | swid>
//!    <code>` ; codes 0x04 / 0x09 : appareil appairé mais éteint ou hors de portée, 0x08 :
//!    emplacement vide, 0x01 : ancien HID++ 1.0 ;
//!  * fonction racine (index 0) : fonction 0 = index d'une fonctionnalité, fonction 1 = « ping »
//!    (un appareil qui dort ne répond pas du tout) ;
//!  * batterie, première fonctionnalité présente parmi :
//!    - 0x1004 (batterie unifiée), fonction 1 : `<pourcentage> <niveau approché> <état>` ;
//!    - 0x1000 (état de batterie), fonction 0 : `<pourcentage> <niveau suivant> <état>` ;
//!      état 1 à 4 (en charge, presque pleine, pleine, charge lente) = sur le chargeur ;
//!    - 0x1001 (tension), fonction 0 : `<mV fort> <mV faible> <drapeaux>`, bit 7 = en charge ;
//!    - 0x1F20 (mesure ADC des casques), fonction 0 : `<mV fort> <mV faible> <drapeaux>`, bit 0 =
//!      casque relié au dongle, bit 1 = en charge ; une réponse d'erreur = casque relié mais
//!      inactif. Tension → pourcentage par le modèle de batterie du casque (`curves`).
//!
//! Casque éteint (`Status::Off`) : ping sans réponse ou refusé (0x04, 0x08, 0x09), ou mesure ADC
//! qui dit le casque absent ou inactif. Seules des requêtes de lecture sont envoyées. Les
//! registres HID++ 1.0 de Halo ne servent qu'aux récepteurs (il faut la collection courte) : ils
//! ne concernent pas les casques. Le PRO X 2 LIGHTSPEED (0AF7) est lu par `centurion`.

mod curves;

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x046D;
/// Casques connus : identifiant produit et nom affiché.
const HEADSETS: &[(u16, &str)] = &[
    (0x0A66, "Logitech G533"),
    (0x0AC4, "Logitech G535"),
    (0x0A5C, "Logitech G633"),
    (0x0A89, "Logitech G635"),
    (0x0A5B, "Logitech G933"),
    (0x0A87, "Logitech G935"),
    (0x0AB5, "Logitech G733"),
    (0x0AFE, "Logitech G733"),
    (0x0B1F, "Logitech G733"),
    (0x0AA7, "Logitech G PRO"),
    (0x0AAA, "Logitech G PRO X"),
    (0x0ABA, "Logitech G PRO X"),
    (0x0AFB, "Logitech G PRO X 2"),
    (0x0AFC, "Logitech G PRO X 2"),
];
/// Collection HID++ (page d'usage, usage) des casques qui n'utilisent pas celle par défaut.
const HEADSET_COLLECTION: &[(u16, (u16, u16))] = &[(0x0AC4, (0x000C, 0x0001))];
const DEFAULT_COLLECTION: (u16, u16) = (0xFF43, 0x0202);

fn collection(pid: u16) -> (u16, u16) {
    HEADSET_COLLECTION.iter().find(|(p, _)| *p == pid).map_or(DEFAULT_COLLECTION, |(_, c)| *c)
}

pub const FAMILY: Family = Family {
    name: "Logitech HID++",
    vids: &[VID],
    probe: |d| {
        let name = HEADSETS.iter().find(|(pid, _)| d.vid == VID && *pid == d.pid)?.1;
        ((d.usage_page, d.usage) == collection(d.pid)).then_some(name)
    },
    new: |d: &HidInfo| Box::new(HidPP::new(d.pid)),
};

const LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
/// Emplacement de l'appareil relié directement (dongle de casque, câble).
const SLOT: u8 = 0xFF;
/// Identifiants logiciels utilisés tour à tour (Halo) : une réponse en retard à une requête
/// précédente, ou destinée à G HUB, n'est pas prise pour la nôtre.
const SWIDS: [u8; 6] = [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F];
const READ_MS: i32 = 20;

const F_ROOT: u8 = 0x00;
const F_UNIFIED: u16 = 0x1004;
const F_STATUS: u16 = 0x1000;
const F_VOLTAGE: u16 = 0x1001;
const F_ADC: u16 = 0x1F20;
/// Ordre d'essai des fonctionnalités de batterie.
const BATTERY_FEATURES: [u16; 4] = [F_UNIFIED, F_STATUS, F_VOLTAGE, F_ADC];

const ERR_OLD_PROTOCOL: u8 = 0x01;
const ERR_CONNECT_FAIL: u8 = 0x04;
const ERR_EMPTY_SLOT: u8 = 0x08;
const ERR_UNREACHABLE: u8 = 0x09;

/// État de batterie 1 à 4 : sur le chargeur (Solaar, `BatteryStatus`).
const ON_CHARGER: [u8; 4] = [1, 2, 3, 4];
/// Niveau approché de 0x1004 quand le pourcentage vaut 0 : drapeau → pourcentage.
const APPROX_LEVELS: [(u8, u8); 4] = [(8, 90), (4, 50), (2, 20), (1, 5)];

/// Ce que donne une fonctionnalité de batterie.
#[derive(Debug, PartialEq, Eq)]
enum Measure {
    Percent(u8),
    /// Tension de la batterie, à passer par le modèle du casque.
    Millivolts(u16),
}

/// Mesure et charge d'une réponse de fonctionnalité de batterie (`p` complétée de zéros).
fn parse_battery(feature: u16, p: &[u8]) -> Option<(Measure, bool)> {
    let on_charger = ON_CHARGER.contains(&p[2]);
    match feature {
        F_UNIFIED if (1..=100).contains(&p[0]) => Some((Measure::Percent(p[0]), on_charger)),
        F_UNIFIED if p[0] == 0 => {
            APPROX_LEVELS.iter().find(|(flag, _)| *flag == p[1]).map(|(_, l)| (Measure::Percent(*l), on_charger))
        }
        F_STATUS if (1..=100).contains(&p[0]) => Some((Measure::Percent(p[0]), on_charger)),
        F_VOLTAGE | F_ADC => {
            if feature == F_ADC && p[2] & 0x01 == 0 {
                return None;
            }
            let mv = u16::from_be_bytes([p[0], p[1]]);
            if mv < 2500 {
                return None;
            }
            let charging = if feature == F_VOLTAGE { p[2] & 0x80 != 0 } else { p[2] & 0x02 != 0 };
            Some((Measure::Millivolts(mv), charging))
        }
        _ => None,
    }
}

/// Réponse à une requête HID++.
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    /// Données (après l'index et la fonction), complétées de 16 zéros.
    Data(Vec<u8>),
    /// Réponse d'erreur et son code.
    Error(u8),
    /// Rien dans le délai.
    Silent,
}

/// Lecteur HID++ d'un casque ; garde les index de fonctionnalités entre deux lectures.
pub struct HidPP {
    timeout: Duration,
    ping_timeout: Duration,
    swid: usize,
    /// Dernier ping resté sans réponse (le suivant attend moins longtemps, comme Halo).
    asleep: bool,
    /// Index de chaque fonctionnalité de `BATTERY_FEATURES` (0 = absente), une fois connu.
    indexes: [Option<u8>; 4],
    /// Modèle de batterie du casque, pour ceux qui ne donnent qu'une tension.
    model: &'static curves::Model,
    /// Plus haute tension lue depuis le début de la charge en cours (`None` hors charge).
    charge_peak: Option<u16>,
}

impl HidPP {
    fn new(pid: u16) -> HidPP {
        HidPP::with_timeouts(pid, Duration::from_millis(600), Duration::from_millis(2000))
    }

    /// `timeout` : attente d'une réponse ; `ping_timeout` : celle du ping d'un appareil éveillé.
    fn with_timeouts(pid: u16, timeout: Duration, ping_timeout: Duration) -> HidPP {
        HidPP {
            timeout,
            ping_timeout,
            swid: 0,
            asleep: false,
            indexes: [None; 4],
            model: curves::model(pid),
            charge_peak: None,
        }
    }

    /// Pourcentage d'une mesure ; une tension passe par le modèle de batterie du casque.
    fn level(&mut self, measure: Measure, charging: bool) -> u8 {
        match measure {
            Measure::Percent(p) => p,
            Measure::Millivolts(mv) if charging => {
                let peak = self.charge_peak.map_or(mv, |p| p.max(mv));
                self.charge_peak = Some(peak);
                self.model.charging(mv, peak)
            }
            Measure::Millivolts(mv) => self.model.discharging(mv),
        }
    }

    fn request(&mut self, link: &mut dyn Link, index: u8, function: u8, params: &[u8], timeout: Duration) -> anyhow::Result<Reply> {
        self.swid = (self.swid + 1) % SWIDS.len();
        let f = (function << 4) | SWIDS[self.swid];
        let mut req = [&[LONG, SLOT, index, f][..], params].concat();
        req.resize(LONG_LEN, 0);
        link.write(&req)?;
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            let r = link.read(READ_MS)?;
            if r.len() < 4 || r[1] != SLOT {
                continue;
            }
            if matches!(r[2], 0x8F | 0xFF) && r.len() >= 6 && r[3] == index && r[4] == f {
                return Ok(Reply::Error(r[5]));
            }
            if r[2] == index && r[3] == f {
                return Ok(Reply::Data([&r[4..], &[0; 16]].concat()));
            }
        }
        Ok(Reply::Silent)
    }

    /// Index d'une fonctionnalité (0 si absente) ; `None` si la racine n'a pas répondu.
    fn feature_index(&mut self, link: &mut dyn Link, feature: u16) -> anyhow::Result<Option<u8>> {
        let t = self.timeout;
        Ok(match self.request(link, F_ROOT, 0, &feature.to_be_bytes(), t)? {
            Reply::Data(d) => Some(d[0]),
            _ => None,
        })
    }

    fn try_read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let t = if self.asleep { self.timeout } else { self.ping_timeout };
        match self.request(link, F_ROOT, 1, &[], t)? {
            Reply::Data(_) => self.asleep = false,
            Reply::Silent => {
                self.asleep = true;
                return Ok(Status::Off);
            }
            Reply::Error(code) => {
                self.asleep = false;
                return match code {
                    ERR_CONNECT_FAIL | ERR_EMPTY_SLOT | ERR_UNREACHABLE => Ok(Status::Off),
                    ERR_OLD_PROTOCOL => Err(anyhow::anyhow!("appareil HID++ 1.0, sans fonctionnalité de batterie")),
                    c => Err(anyhow::anyhow!("ping refusé (erreur {c:#04x})")),
                };
            }
        }
        let mut off = false;
        for (i, &feature) in BATTERY_FEATURES.iter().enumerate() {
            let index = match self.indexes[i] {
                Some(n) => n,
                None => match self.feature_index(link, feature)? {
                    Some(n) => {
                        self.indexes[i] = Some(n);
                        n
                    }
                    None => continue,
                },
            };
            if index == 0 {
                continue;
            }
            let function = if feature == F_UNIFIED { 1 } else { 0 };
            let t = self.timeout;
            match self.request(link, index, function, &[], t)? {
                Reply::Data(p) => {
                    if let Some((measure, charging)) = parse_battery(feature, &p) {
                        return Ok(Status::Battery { level: self.level(measure, charging), charging });
                    }
                    off |= feature == F_ADC && p[2] & 0x01 == 0;
                }
                // Casque relié au dongle mais inactif.
                Reply::Error(_) if feature == F_ADC => off = true,
                _ => {}
            }
        }
        if off { Ok(Status::Off) } else { Err(anyhow::anyhow!("aucune fonctionnalité de batterie lisible")) }
    }
}

impl Driver for HidPP {
    /// Lit la batterie. Une erreur efface les index : ils sont redécouverts la fois suivante.
    /// Le sommet de tension ne vaut que pour une charge suivie : oublié dès que le casque est lu
    /// hors charge ou éteint, gardé après une erreur (une réponse perdue est fréquente).
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let s = self.try_read(link);
        match s {
            Err(_) => self.indexes = [None; 4],
            Ok(Status::Battery { charging: true, .. }) => {}
            Ok(_) => self.charge_peak = None,
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Comportement du faux casque face au ping.
    #[derive(Clone, Copy)]
    enum Ping {
        Answer,
        Silent,
        Error(u8),
    }

    /// Faux dongle de casque : fonctionnalités `features` (index = position), réponse batterie
    /// `battery` (`None` = réponse d'erreur).
    struct Fake {
        ping: Ping,
        features: Vec<u16>,
        battery: Option<[u8; 3]>,
        queue: VecDeque<Vec<u8>>,
        writes: usize,
        /// Réponses d'un autre logiciel (swid 1) glissées avant les nôtres.
        noise: bool,
    }

    impl Fake {
        fn new(battery_feature: u16, battery: Option<[u8; 3]>) -> Fake {
            Fake {
                ping: Ping::Answer,
                features: vec![0x0000, 0x0001, 0x0003, 0x0005, battery_feature],
                battery,
                queue: VecDeque::new(),
                writes: 0,
                noise: false,
            }
        }
    }

    fn long(data: &[u8]) -> Vec<u8> {
        let mut r = [&[LONG, SLOT][..], data].concat();
        r.resize(LONG_LEN, 0);
        r
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), LONG_LEN);
            assert_eq!(&w[..2], &[LONG, SLOT]);
            self.writes += 1;
            let (index, f) = (w[2], w[3]);
            if self.noise {
                self.queue.push_back(long(&[index, (f & 0xF0) | 1, 0x63, 0x00, 0x00]));
            }
            let error = |code| vec![0x10, SLOT, 0x8F, index, f, code, 0];
            let reply = match (index, f >> 4) {
                (0, 1) => match self.ping {
                    Ping::Answer => Some(long(&[index, f, 4, 2])),
                    Ping::Silent => None,
                    Ping::Error(code) => Some(error(code)),
                },
                (0, 0) => {
                    let id = u16::from_be_bytes([w[4], w[5]]);
                    let n = self.features.iter().position(|&x| x == id).unwrap_or(0) as u8;
                    Some(long(&[index, f, n]))
                }
                (i, _) if i as usize == self.features.len() - 1 => Some(match self.battery {
                    Some(b) => long(&[&[index, f][..], &b].concat()),
                    None => long(&[0xFF, index, f, 0x05]),
                }),
                _ => Some(error(0x02)),
            };
            self.queue.extend(reply);
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    /// Lecteur d'un casque sans fiche de batterie (G733).
    fn reader() -> HidPP {
        reader_for(0x0AB5)
    }

    fn reader_for(pid: u16) -> HidPP {
        HidPP::with_timeouts(pid, Duration::from_millis(30), Duration::from_millis(30))
    }

    fn read(dev: &mut Fake) -> anyhow::Result<Status> {
        reader().read(dev)
    }

    #[test]
    fn unified_battery_level_and_state() {
        let mut dev = Fake::new(F_UNIFIED, Some([87, 0x04, 0]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 87, charging: false });
        for (state, charging) in [(1, true), (2, true), (3, true), (4, true), (5, false)] {
            let mut dev = Fake::new(F_UNIFIED, Some([87, 0x04, state]));
            assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 87, charging });
        }
    }

    #[test]
    fn unified_approximate_level_when_percent_is_zero() {
        let mut dev = Fake::new(F_UNIFIED, Some([0, 0x04, 1]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 50, charging: true });
    }

    #[test]
    fn battery_status_feature() {
        let mut dev = Fake::new(F_STATUS, Some([42, 30, 0]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 42, charging: false });
    }

    #[test]
    fn voltage_feature_bit_7_is_charging() {
        let [hi, lo] = 3811u16.to_be_bytes();
        let mut dev = Fake::new(F_VOLTAGE, Some([hi, lo, 0x80]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 50, charging: true });
    }

    #[test]
    fn adc_measurement_of_a_connected_headset() {
        let [hi, lo] = 3989u16.to_be_bytes();
        let mut dev = Fake::new(F_ADC, Some([hi, lo, 0x01]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 80, charging: false });
        let mut dev = Fake::new(F_ADC, Some([hi, lo, 0x03]));
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 80, charging: true });
    }

    #[test]
    fn headset_off() {
        let [hi, lo] = 3989u16.to_be_bytes();
        // Dongle : casque non relié (bit 0 à 0), ou relié mais inactif (erreur).
        assert_eq!(read(&mut Fake::new(F_ADC, Some([hi, lo, 0x00]))).unwrap(), Status::Off);
        assert_eq!(read(&mut Fake::new(F_ADC, None)).unwrap(), Status::Off);
        // Ping refusé (hors de portée, éteint, emplacement vide) ou sans réponse.
        for ping in [Ping::Error(0x09), Ping::Error(0x04), Ping::Error(0x08), Ping::Silent] {
            let mut dev = Fake::new(F_UNIFIED, Some([87, 0, 0]));
            dev.ping = ping;
            assert_eq!(read(&mut dev).unwrap(), Status::Off);
        }
    }

    #[test]
    fn unreadable_battery_is_an_error() {
        // Pourcentage hors bornes sans niveau approché, tension trop basse, erreur hors ADC.
        assert!(read(&mut Fake::new(F_UNIFIED, Some([0xFF, 0x03, 0]))).is_err());
        assert!(read(&mut Fake::new(F_VOLTAGE, Some([0x05, 0x00, 0]))).is_err());
        assert!(read(&mut Fake::new(F_STATUS, None)).is_err());
        let mut dev = Fake::new(F_UNIFIED, Some([87, 0, 0]));
        dev.ping = Ping::Error(ERR_OLD_PROTOCOL);
        assert!(read(&mut dev).is_err());
    }

    #[test]
    fn replies_to_other_software_are_ignored() {
        let mut dev = Fake::new(F_UNIFIED, Some([87, 0x04, 0]));
        dev.noise = true;
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 87, charging: false });
    }

    #[test]
    fn feature_indexes_are_kept_between_reads() {
        let mut h = reader();
        let mut dev = Fake::new(F_ADC, Some([0x0F, 0x95, 0x01]));
        h.read(&mut dev).unwrap();
        let before = dev.writes;
        assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level: 80, charging: false });
        assert_eq!(dev.writes - before, 2, "ping et lecture, sans redécouverte");
        // Casque relié mais tension illisible (sous 2,5 V) : erreur.
        dev.battery = Some([0x05, 0x00, 0x01]);
        assert!(h.read(&mut dev).is_err());
        assert_eq!(h.indexes, [None; 4], "index oubliés après une erreur");
    }

    #[test]
    fn pro_x_is_read_with_its_own_battery_model() {
        let adc = |mv: u16, flags: u8| {
            let [hi, lo] = mv.to_be_bytes();
            Some([hi, lo, flags])
        };
        let mut h = reader_for(0x0ABA);
        let mut dev = Fake::new(F_ADC, adc(3758, 0x01));
        assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level: 50, charging: false });
        // La même tension sur un casque sans fiche.
        assert_eq!(read(&mut dev).unwrap(), Status::Battery { level: 33, charging: false });
        // En charge : la tension monte jusqu'au sommet, puis redescend pendant que la charge finit.
        for (mv, level) in [(4139, 65), (4250, 84), (4275, 87), (4250, 91), (4200, 100)] {
            dev.battery = adc(mv, 0x03);
            assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level, charging: true }, "{mv} mV");
        }
        // Une lecture ratée ne fait pas oublier le sommet.
        dev.battery = adc(0x0500, 0x03);
        assert!(h.read(&mut dev).is_err());
        dev.battery = adc(4250, 0x03);
        assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level: 91, charging: true });
        // Débranché puis rebranché : une nouvelle charge repart de la courbe montante.
        dev.battery = adc(4100, 0x01);
        assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level: 100, charging: false });
        dev.battery = adc(4250, 0x03);
        assert_eq!(h.read(&mut dev).unwrap(), Status::Battery { level: 84, charging: true });
    }

    #[test]
    fn probe_only_the_headset_collection() {
        let g733 = HidInfo { vid: 0x046D, pid: 0x0AB5, usage_page: 0xFF43, usage: 0x0202, interface: 3 };
        assert_eq!((FAMILY.probe)(&g733), Some("Logitech G733"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0xFF00, usage: 0x0002, ..g733 }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0x000C, usage: 0x0001, ..g733 }), None);
        let g535 = HidInfo { pid: 0x0AC4, usage_page: 0x000C, usage: 0x0001, ..g733 };
        assert_eq!((FAMILY.probe)(&g535), Some("Logitech G535"));
        assert_eq!((FAMILY.probe)(&HidInfo { usage_page: 0xFF43, usage: 0x0202, ..g535 }), None);
        // PRO X 2 LIGHTSPEED (Centurion), souris et récepteurs : pas pour cette famille.
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0x0AF7, ..g733 }), None);
        assert_eq!((FAMILY.probe)(&HidInfo { pid: 0xC547, usage_page: 0xFF00, usage: 0x0002, ..g733 }), None);
    }
}
