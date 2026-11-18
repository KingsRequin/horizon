//! Logitech G PRO X 2 LIGHTSPEED (046D:0AF7) par son récepteur, sans G HUB.
//!
//! Protocole « Centurion » repris de Halo Battery (`providers/logitech_centurion.py`, licence
//! MIT), lui-même tiré de Solaar et HeadsetControl :
//!  * collection HID constructeur 0xFFA0/0x0001, rapports de 64 octets, identifiant 0x51 ;
//!  * trame : `51 <len> <flags> <charge utile…>`, `len` = longueur de la charge utile + 1 ;
//!  * requête au récepteur : `<index de fonction> <fonction | id logiciel> <paramètres>` ; la
//!    réponse reprend les deux premiers octets, une erreur est `ff <index> <fonction> <code>` ;
//!  * le casque se joint par la fonction « pont » du récepteur (0x0003) : charge utile
//!    `<pont> 0x1n <taille sur 2 octets> 00 <index> <fonction | id> <paramètres>` ; le récepteur
//!    accuse réception (`<pont> 0x1n`, n = id logiciel), puis le casque répond
//!    (`<pont> 0x10 <taille> 00 <index> <fonction | id> <données>`, index 0xFF = refus) ;
//!  * batterie : fonction 0x0104 du casque, fonction 0 : `données[0]` = pourcentage,
//!    `données[2]` = 1, 2 ou 3 quand il est sur le câble ;
//!  * index des fonctions (pont sur le récepteur, batterie sur le casque) : fonction racine
//!    `getFeature` (index 0, fonction 0), une seule requête chacune (Halo énumère FeatureSet,
//!    ce qui mélange les réponses quand un autre lecteur interroge le casque en même temps).
//!
//! Casque éteint (`Status::Off`) : le récepteur refuse de relayer (`ff <pont> <fonction> 0b`,
//! réponse immédiate), ou accuse réception sans que le casque réponde. Le récepteur annonce
//! aussi de lui-même l'arrivée et le départ du casque (`<pont> 00 00 <01 | 00>`, `notice`) :
//! de quoi relire tout de suite au lieu d'attendre la lecture suivante.
//!
//! Seules des fonctions de lecture sont envoyées.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x046D;
const USAGE_PAGE: u16 = 0xFFA0;
const USAGE: u16 = 0x0001;
/// Casques connus : identifiant produit et nom affiché.
const PRODUCTS: &[(u16, &str)] = &[(0x0AF7, "Logitech PRO X 2 LIGHTSPEED")];

pub const FAMILY: Family = Family {
    name: "Logitech Centurion",
    vids: &[VID],
    probe: |d| {
        let name = PRODUCTS.iter().find(|(pid, _)| d.vid == VID && *pid == d.pid)?.1;
        (d.usage_page == USAGE_PAGE && d.usage == USAGE).then_some(name)
    },
    new: |_: &HidInfo| Box::new(Centurion::default()),
};

const REPORT_ID: u8 = 0x51;
const FRAME: usize = 64;
/// Identifiant logiciel des requêtes. Halo Battery utilise 1 : avec un autre, chacun ne
/// prend que ses propres réponses quand les deux interrogent le casque.
const SW: u8 = 0x0B;
const BRIDGE_SEND: u8 = 0x10;
const ROOT: u8 = 0x00;
const FEATURE_BRIDGE: u16 = 0x0003;
const FEATURE_BATTERY: u16 = 0x0104;
/// Refus du récepteur de relayer vers le casque : casque non relié (éteint, hors de portée).
const ERR_NOT_CONNECTED: u8 = 0x0B;
const READ_MS: i32 = 100;

#[derive(Debug)]
enum Fail {
    Off,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for Fail {
    fn from(e: anyhow::Error) -> Self {
        Fail::Other(e)
    }
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0; FRAME];
    out[0] = REPORT_ID;
    out[1] = payload.len() as u8 + 1;
    let n = payload.len().min(FRAME - 3);
    out[3..3 + n].copy_from_slice(&payload[..n]);
    out
}

fn payload_of(r: &[u8]) -> Option<&[u8]> {
    if r.len() < 4 || r[0] != REPORT_ID {
        return None;
    }
    let n = r[1] as usize;
    (n > 1 && n + 2 <= r.len()).then(|| &r[3..2 + n])
}

fn fn_sw(function: u8) -> u8 {
    (function & 0xF0) | SW
}

/// Avis spontané du récepteur (fonction 0, identifiant logiciel 0 : jamais une réponse), comme
/// l'arrivée ou le départ du casque.
fn notice(r: &[u8]) -> bool {
    payload_of(r).is_some_and(|p| p.len() >= 3 && p[0] != 0xFF && p[1] == 0x00)
}

/// Lecteur de batterie ; garde les index découverts tant qu'ils servent.
pub struct Centurion {
    window: Duration,
    /// Index du pont sur le récepteur.
    bridge: Option<u8>,
    /// Index de la batterie sur le casque.
    battery: Option<u8>,
}

impl Default for Centurion {
    fn default() -> Self {
        Centurion::with_window(Duration::from_millis(1500))
    }
}

impl Centurion {
    /// `window` : attente maximale d'une réponse.
    pub(crate) fn with_window(window: Duration) -> Centurion {
        Centurion { window, bridge: None, battery: None }
    }

    fn try_read(&mut self, link: &mut dyn Link) -> Result<Status, Fail> {
        let bridge = match self.bridge {
            Some(b) => b,
            None => *self.bridge.insert(self.feature_index(link, None, FEATURE_BRIDGE, "récepteur sans pont Centurion")?),
        };
        let battery = match self.battery {
            Some(b) => b,
            None => *self.battery.insert(self.feature_index(link, Some(bridge), FEATURE_BATTERY, "casque sans fonction batterie 0x0104")?),
        };
        let d = self.bridge(link, bridge, battery, 0x00, &[])?;
        match d.first() {
            Some(&level) if level <= 100 => {
                Ok(Status::Battery { level, charging: matches!(d.get(2), Some(1..=3)) })
            }
            _ => Err(anyhow::anyhow!("niveau de batterie illisible : {d:02x?}").into()),
        }
    }

    /// Index d'une fonction, du récepteur (`bridge` = `None`) ou du casque par le pont.
    fn feature_index(&self, link: &mut dyn Link, bridge: Option<u8>, feature: u16, missing: &str) -> Result<u8, Fail> {
        let id = feature.to_be_bytes();
        let d = match bridge {
            None => self.direct(link, ROOT, 0x00, &id)?,
            Some(b) => self.bridge(link, b, ROOT, 0x00, &id)?,
        };
        match d.first() {
            Some(&n) if n != 0 => Ok(n),
            _ => Err(anyhow::anyhow!("{missing}").into()),
        }
    }

    fn deadline(&self) -> Instant {
        Instant::now() + self.window
    }

    fn direct(&self, link: &mut dyn Link, index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Fail> {
        let f = fn_sw(function);
        link.write(&frame(&[&[index, f][..], params].concat()))?;
        let end = self.deadline();
        while Instant::now() < end {
            let r = link.read(READ_MS)?;
            let Some(p) = payload_of(&r) else { continue };
            if p.len() >= 3 && p[0] == 0xFF && p[1] == index && p[2] == f {
                return Err(anyhow::anyhow!("récepteur, fonction {index:#04x} : erreur").into());
            }
            if p.len() >= 2 && p[0] == index && p[1] == f {
                return Ok(p[2..].to_vec());
            }
        }
        Err(anyhow::anyhow!("le récepteur ne répond pas (fonction {index:#04x})").into())
    }

    fn bridge(&self, link: &mut dyn Link, bridge: u8, index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, Fail> {
        let f = fn_sw(function);
        let send = BRIDGE_SEND | SW;
        let sub = [&[0x00, index, f][..], params].concat();
        let head = [bridge, send, (sub.len() >> 8) as u8 & 0x0F, sub.len() as u8];
        link.write(&frame(&[&head[..], &sub].concat()))?;
        let mut acked = false;
        let end = self.deadline();
        while Instant::now() < end {
            let r = link.read(READ_MS)?;
            let Some(p) = payload_of(&r) else { continue };
            if p.len() >= 4 && p[0] == 0xFF && p[1] == bridge && p[2] == send {
                return Err(match p[3] {
                    ERR_NOT_CONNECTED => Fail::Off,
                    c => anyhow::anyhow!("le récepteur refuse de relayer (erreur {c:#04x})").into(),
                });
            }
            if p.len() < 2 || p[0] != bridge || p[1] >> 4 != BRIDGE_SEND >> 4 {
                continue;
            }
            match p[1] & 0x0F {
                SW => acked = true,
                0 if p.len() >= 7 && p[4] == 0x00 => {
                    if p[5] == 0xFF && p[6] == index {
                        return Err(anyhow::anyhow!("le casque refuse la fonction {index:#04x}").into());
                    }
                    if p[5] == index && p[6] == f {
                        return Ok(p[7..].to_vec());
                    }
                }
                _ => {}
            }
        }
        if acked { Err(Fail::Off) } else { Err(anyhow::anyhow!("pas d'accusé du récepteur").into()) }
    }
}

impl Driver for Centurion {
    /// Lit la batterie. Une erreur efface les index : ils sont redécouverts la fois suivante.
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        match self.try_read(link) {
            Ok(s) => Ok(s),
            Err(Fail::Off) => Ok(Status::Off),
            Err(Fail::Other(e)) => {
                self.bridge = None;
                self.battery = None;
                Err(e)
            }
        }
    }

    fn notice(&self) -> Option<fn(&[u8]) -> bool> {
        Some(notice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Faux récepteur : fonctions [Root, FeatureSet, 0x0100, pont] ; casque : [Root,
    /// FeatureSet, 0x0002, 0x0003, batterie]. `on` = casque allumé ; casque éteint, le
    /// récepteur refuse de relayer (`refuse`, comme le vrai) ou accuse réception sans suite.
    struct Fake {
        on: bool,
        refuse: bool,
        level: u8,
        state: u8,
        queue: VecDeque<Vec<u8>>,
        writes: usize,
        /// Réponses d'un autre logiciel (id 1) glissées avant les nôtres.
        noise: bool,
    }

    const RX: [u16; 4] = [0x0000, 0x0001, 0x0100, 0x0003];
    const HS: [u16; 5] = [0x0000, 0x0001, 0x0002, 0x0003, 0x0104];

    fn answer(features: &[u16], index: u8, function: u8, params: &[u8], battery: (u8, u8)) -> Option<Vec<u8>> {
        let f = function & 0xF0;
        Some(match (features.get(index as usize)?, f) {
            (0x0000, 0x00) => {
                let id = u16::from_be_bytes([params[0], params[1]]);
                vec![features.iter().position(|&x| x == id).unwrap_or(0) as u8]
            }
            (0x0001, 0x00) => vec![features.len() as u8],
            (0x0001, 0x10) => {
                let id = features[params[0] as usize];
                vec![0, (id >> 8) as u8, id as u8]
            }
            (0x0104, 0x00) => vec![battery.0, 0, battery.1],
            _ => return None,
        })
    }

    impl Fake {
        fn new(on: bool) -> Fake {
            Fake { on, refuse: true, level: 92, state: 0, queue: VecDeque::new(), writes: 0, noise: false }
        }
    }

    impl Link for Fake {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), FRAME);
            self.writes += 1;
            let p = payload_of(w).unwrap().to_vec();
            let (index, function) = (p[0], p[1]);
            if self.noise {
                self.queue.push_back(frame(&[index, (function & 0xF0) | 1, 0x63]));
            }
            if index == 3 && function & 0xF0 == BRIDGE_SEND {
                if !self.on && self.refuse {
                    self.queue.push_back(frame(&[0xFF, index, function, ERR_NOT_CONNECTED]));
                    return Ok(());
                }
                self.queue.push_back(frame(&[index, function]));
                if self.on {
                    let sub = &p[4..];
                    let data = answer(&HS, sub[1], sub[2], &sub[3..], (self.level, self.state))
                        .map(|d| [&[3, 0x10, 0, 0, 0x00, sub[1], sub[2]][..], &d].concat())
                        .unwrap_or_else(|| vec![3, 0x10, 0, 0, 0x00, 0xFF, sub[1], sub[2]]);
                    self.queue.push_back(frame(&data));
                }
            } else {
                let reply = answer(&RX, index, function, &p[2..], (0, 0))
                    .map(|d| [&[index, function][..], &d].concat())
                    .unwrap_or_else(|| vec![0xFF, index, function, 0x02]);
                self.queue.push_back(frame(&reply));
            }
            Ok(())
        }

        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn reader() -> Centurion {
        Centurion::with_window(Duration::from_millis(30))
    }

    #[test]
    fn reads_level_through_the_bridge() {
        let mut c = reader();
        let mut dev = Fake::new(true);
        assert_eq!(c.read(&mut dev).unwrap(), Status::Battery { level: 92, charging: false });
    }

    #[test]
    fn charging_states_1_to_3() {
        for (state, charging) in [(0, false), (1, true), (2, true), (3, true), (4, false)] {
            let mut dev = Fake::new(true);
            dev.state = state;
            assert_eq!(reader().read(&mut dev).unwrap(), Status::Battery { level: 92, charging });
        }
    }

    #[test]
    fn discovery_is_kept_between_reads() {
        let mut c = reader();
        let mut dev = Fake::new(true);
        c.read(&mut dev).unwrap();
        let before = dev.writes;
        dev.level = 91;
        assert_eq!(c.read(&mut dev).unwrap(), Status::Battery { level: 91, charging: false });
        assert_eq!(dev.writes - before, 1, "une seule requête une fois les index connus");
    }

    #[test]
    fn discovery_asks_the_root_once_per_feature() {
        let mut c = reader();
        let mut dev = Fake::new(true);
        c.read(&mut dev).unwrap();
        assert_eq!((c.bridge, c.battery), (Some(3), Some(4)));
        assert_eq!(dev.writes, 3, "pont, batterie, niveau");
    }

    #[test]
    fn headset_off_when_the_receiver_refuses_to_relay() {
        let mut c = Centurion::with_window(Duration::from_secs(5));
        let mut dev = Fake::new(false);
        let t = Instant::now();
        assert_eq!(c.read(&mut dev).unwrap(), Status::Off);
        assert!(t.elapsed() < Duration::from_secs(1), "refus immédiat, pas d'attente");
        assert_eq!(c.bridge, Some(3), "le pont reste connu");
        // Rallumé : batterie découverte puis lue.
        dev.on = true;
        assert_eq!(c.read(&mut dev).unwrap(), Status::Battery { level: 92, charging: false });
        dev.on = false;
        let before = dev.writes;
        assert_eq!(c.read(&mut dev).unwrap(), Status::Off);
        assert_eq!(dev.writes - before, 1);
    }

    #[test]
    fn headset_off_when_only_the_receiver_answers() {
        let mut dev = Fake::new(false);
        dev.refuse = false;
        assert_eq!(reader().read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn arrival_and_departure_notices() {
        // Relevés sur un vrai récepteur : départ, arrivée.
        assert!(notice(&frame(&[0x03, 0x00, 0x00, 0x00])));
        assert!(notice(&frame(&[0x03, 0x00, 0x00, 0x01])));
        // Réponses (à nous ou à un autre logiciel), accusés, refus : pas des avis.
        assert!(!notice(&frame(&[0x03, 0x1B])));
        assert!(!notice(&frame(&[0x03, 0x10, 0x00, 0x06, 0x00, 0x04, 0x0B, 0x4E])));
        assert!(!notice(&frame(&[0x03, 0x10, 0x00, 0x06, 0x00, 0x04, 0x01, 0x4E])));
        assert!(!notice(&frame(&[0x00, 0x0B, 0x01])));
        assert!(!notice(&frame(&[0xFF, 0x03, 0x1B, 0x0B])));
        assert!(!notice(&[]));
    }

    #[test]
    fn replies_to_other_software_are_ignored() {
        let mut dev = Fake::new(true);
        dev.noise = true;
        // La fenêtre n'est qu'un plafond : celle de 30 ms de `reader()` pouvait expirer avant la
        // vraie réponse sur une machine chargée (tests en parallèle).
        let mut c = Centurion::with_window(Duration::from_secs(10));
        assert_eq!(c.read(&mut dev).unwrap(), Status::Battery { level: 92, charging: false });
    }

    #[test]
    fn level_above_100_is_refused_and_discovery_forgotten() {
        let mut c = reader();
        let mut dev = Fake::new(true);
        dev.level = 0xFF;
        assert!(c.read(&mut dev).is_err());
        assert_eq!((c.bridge, c.battery), (None, None));
    }

    #[test]
    fn silent_receiver_is_an_error() {
        struct Mute;
        impl Link for Mute {
            fn write(&mut self, _: &[u8]) -> anyhow::Result<()> {
                Ok(())
            }
            fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
                Ok(vec![])
            }
        }
        assert!(reader().read(&mut Mute).is_err());
    }

    #[test]
    fn frame_layout() {
        let f = frame(&[0x03, 0x1B, 0x00, 0x03, 0x00, 0x04, 0x0B]);
        assert_eq!(&f[..10], &[0x51, 0x08, 0x00, 0x03, 0x1B, 0x00, 0x03, 0x00, 0x04, 0x0B]);
        assert_eq!(payload_of(&f), Some(&f[3..10]));
    }
}
