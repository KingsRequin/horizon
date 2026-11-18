//! Casques Corsair sans fil par leur récepteur, sans iCUE.
//!
//! Protocoles repris de Halo Battery (`providers/corsair.py`, licence MIT), qui s'appuie sur
//! HeadsetControl (Sapd/HeadsetControl : `corsair_void_v2w`, Virtuoso XT/SE), ckb-next
//! (« bragi » : `bragi_proto.h`, `device_bragi.c`) et OpenLinkHub (« slipstream »). Les souris
//! (Dark Core RGB Pro SE, 1B1C:1B7F) ne sont pas reprises ici.
//!
//! Ancien protocole (Void v2 Wireless, Virtuoso Max, HS80 Max) :
//!  * collection de l'interface USB 4 du récepteur (HeadsetControl ne donne ni page ni usage) ;
//!  * requête de 65 octets : `00 02 <destinataire> 02 <commande>`, destinataire 0x08 = récepteur,
//!    0x09 = casque ; réponses de 64 octets ;
//!  * poignée de main minimale (micrologiciel du récepteur, battement du récepteur, battement du
//!    casque), sans passer le casque en mode logiciel ; pas de réponse au battement du casque :
//!    casque éteint ou endormi ;
//!  * batterie : commande 0x0F au casque, `r[4] | r[5] << 8` = dixièmes de pour cent ; 0 ou plus
//!    de 1000 (le récepteur répond parfois l'identifiant du casque) : réessayé puis refusé ;
//!  * pas d'indication de charge.
//!
//! Protocole « bragi » (Virtuoso RGB Wireless SE et XT, récepteurs et câbles) :
//!  * collection constructeur 0xFF42/0x0001 ;
//!  * requête nue de 64 octets `02 <route> 02 <propriété>` (Windows refuse la forme précédée de
//!    l'identifiant 0), route 0x08 = récepteur, 0x09 = casque ; propriétés : 0x0F batterie,
//!    0x10 charge (1 en charge, 2 sur batterie), 0x12 battement, 0x13 micrologiciel ;
//!  * réponse `01 02 00 <valeur sur 2 octets>` (ou `01 01 02 00 <valeur>`), dixièmes de pour cent ;
//!  * la réponse reprend la commande, pas la propriété : un autre logiciel sur le même récepteur
//!    peut glisser la réponse à une autre propriété. La batterie est donc demandée deux fois et
//!    deux réponses différentes sont refusées ;
//!  * le récepteur 0A40 veut la séance complète (micrologiciel, battements) avant ; les câbles
//!    s'interrogent d'abord sur la route 0x08, les récepteurs sur 0x09, l'autre en secours.
//!
//! Seules des lectures (« get ») sont envoyées.

use super::{Driver, Family, HidInfo, Link, Status};
use std::time::{Duration, Instant};

const VID: u16 = 0x1B1C;

/// Ancien protocole : identifiant produit et nom affiché.
const LEGACY: &[(u16, &str)] = &[
    (0x2A08, "Corsair Void v2 Wireless"),
    (0x2A02, "Corsair Virtuoso Max Wireless"),
    (0x0A97, "Corsair HS80 Max Wireless"),
];
/// Interface USB de la collection de commande (ancien protocole).
const CONTROL_INTERFACE: i32 = 4;

/// Protocole bragi : identifiant produit et nom affiché.
const BRAGI: &[(u16, &str)] = &[
    (0x0A40, "Corsair Virtuoso RGB Wireless SE"),
    (0x0A3E, "Corsair Virtuoso RGB Wireless SE"),
    (0x0A3D, "Corsair Virtuoso RGB Wireless SE"),
    (0x0A64, "Corsair Virtuoso RGB Wireless XT"),
    (0x0A62, "Corsair Virtuoso RGB Wireless XT"),
];
const BRAGI_PAGE: u16 = 0xFF42;
const BRAGI_USAGE: u16 = 0x0001;
/// Récepteur qui veut la séance complète avant les propriétés.
const SESSION_PID: u16 = 0x0A40;
/// Câbles : route 0x08 d'abord.
const WIRED_PIDS: &[u16] = &[0x0A3D, 0x0A62];
/// Récepteurs SE et XT : route 0x09 d'abord, 0x08 en secours.
const BOTH_ROUTES_PIDS: &[u16] = &[0x0A3E, 0x0A64];

fn name(table: &[(u16, &'static str)], pid: u16) -> Option<&'static str> {
    table.iter().find(|(p, _)| *p == pid).map(|(_, n)| *n)
}

pub const FAMILY: Family = Family {
    name: "Corsair",
    vids: &[VID],
    probe: |d| {
        if d.vid != VID {
            return None;
        }
        if let Some(n) = name(LEGACY, d.pid) {
            return (d.interface == CONTROL_INTERFACE).then_some(n);
        }
        let n = name(BRAGI, d.pid)?;
        (d.usage_page == BRAGI_PAGE && d.usage == BRAGI_USAGE).then_some(n)
    },
    new: |d: &HidInfo| {
        if name(LEGACY, d.pid).is_some() {
            Box::new(Legacy)
        } else {
            Box::new(Bragi::new(d.pid))
        }
    },
};

/// Destinataires, communs aux deux protocoles.
const RECEIVER: u8 = 0x08;
const HEADSET: u8 = 0x09;
/// Valeur maximale de la batterie (dixièmes de pour cent).
const LEVEL_MAX: u16 = 1000;
const READ_MS: i32 = 500;
const FLUSH_MS: i32 = 30;

/// Vide les réponses en attente (quatre lectures courtes au plus).
fn drain(link: &mut dyn Link) -> anyhow::Result<()> {
    for _ in 0..4 {
        if link.read(FLUSH_MS)?.is_empty() {
            break;
        }
    }
    Ok(())
}

/// Dixièmes de pour cent en pour cent ; 0 et les valeurs hors échelle sont refusés.
fn percent(tenths: u16) -> Option<u8> {
    (tenths != 0 && tenths <= LEVEL_MAX).then_some((tenths / 10) as u8)
}

// --- Ancien protocole -------------------------------------------------------------------------

const CMD_FIRMWARE: u8 = 0x13;
const CMD_HEARTBEAT: u8 = 0x12;
const CMD_BATTERY: u8 = 0x0F;
const ATTEMPTS: usize = 3;

fn legacy_request(endpoint: u8, command: u8) -> Vec<u8> {
    let mut f = vec![0; 65];
    f[1] = 0x02;
    f[2] = endpoint;
    f[3] = 0x02;
    f[4] = command;
    f
}

fn legacy_level(r: &[u8]) -> Option<u8> {
    percent(u16::from_le_bytes([*r.get(4)?, *r.get(5)?]))
}

/// Void v2 Wireless et apparentés.
struct Legacy;

impl Driver for Legacy {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        link.write(&legacy_request(RECEIVER, CMD_FIRMWARE))?;
        link.write(&legacy_request(RECEIVER, CMD_HEARTBEAT))?;
        drain(link)?;
        link.write(&legacy_request(HEADSET, CMD_HEARTBEAT))?;
        if link.read(READ_MS)?.is_empty() {
            return Ok(Status::Off);
        }
        drain(link)?;
        for _ in 0..ATTEMPTS {
            link.write(&legacy_request(HEADSET, CMD_BATTERY))?;
            if let Some(level) = legacy_level(&link.read(READ_MS)?) {
                return Ok(Status::Battery { level, charging: false });
            }
        }
        anyhow::bail!("aucun niveau de batterie lisible dans les réponses")
    }
}

// --- Protocole bragi --------------------------------------------------------------------------

const GET: u8 = 0x02;
const PROP_BATTERY: u8 = 0x0F;
const PROP_CHARGE: u8 = 0x10;
const PROP_HEARTBEAT: u8 = 0x12;
const PROP_FIRMWARE: u8 = 0x13;
const FRAME: usize = 64;
/// Lectures au plus par attente (une trame d'un autre logiciel écourte une lecture).
const READ_ATTEMPTS: usize = 96;

fn bragi_request(prop: u8, route: u8) -> Vec<u8> {
    let mut f = vec![0; FRAME];
    f[..4].copy_from_slice(&[0x02, route, GET, prop]);
    f
}

/// Valeur d'une réponse « get » sans erreur, sinon `None` (trame d'autre chose).
fn bragi_value(reply: &[u8]) -> Option<u16> {
    let d = if reply.len() > FRAME { &reply[1..] } else { reply };
    if d.len() < 5 || d[0] != 0x01 {
        return None;
    }
    if d[1] == GET && d[2] == 0 {
        return Some(u16::from_le_bytes([d[3], d[4]]));
    }
    if d.len() >= 6 && d[1] == 0x01 && d[2] == GET && d[3] == 0 {
        return Some(u16::from_le_bytes([d[4], d[5]]));
    }
    None
}

fn bragi_level(reply: &[u8]) -> Option<u8> {
    percent(bragi_value(reply)?)
}

fn bragi_charge(reply: &[u8]) -> Option<bool> {
    match bragi_value(reply)? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Virtuoso RGB Wireless SE / XT.
struct Bragi {
    pid: u16,
    /// Temps total accordé à une lecture.
    budget: Duration,
}

impl Bragi {
    fn new(pid: u16) -> Bragi {
        Bragi { pid, budget: Duration::from_secs(7) }
    }

    fn routes(&self) -> &'static [u8] {
        if WIRED_PIDS.contains(&self.pid) {
            &[RECEIVER, HEADSET]
        } else if BOTH_ROUTES_PIDS.contains(&self.pid) {
            &[HEADSET, RECEIVER]
        } else {
            &[HEADSET]
        }
    }
}

fn left(deadline: Instant, cap: Duration) -> Duration {
    deadline.saturating_duration_since(Instant::now()).min(cap)
}

/// Première réponse que `parse` accepte dans la fenêtre.
fn ask<T>(link: &mut dyn Link, window: Duration, parse: fn(&[u8]) -> Option<T>) -> anyhow::Result<Option<T>> {
    let end = Instant::now() + window;
    for _ in 0..READ_ATTEMPTS {
        if Instant::now() >= end {
            break;
        }
        let r = link.read(READ_MS)?;
        if let Some(v) = parse(&r) {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

impl Driver for Bragi {
    fn read(&mut self, link: &mut dyn Link) -> anyhow::Result<Status> {
        let deadline = Instant::now() + self.budget;
        drain(link)?;
        if self.pid == SESSION_PID {
            for (prop, route) in [(PROP_FIRMWARE, RECEIVER), (PROP_HEARTBEAT, RECEIVER), (PROP_HEARTBEAT, HEADSET)] {
                if Instant::now() >= deadline || link.write(&bragi_request(prop, route)).is_err() {
                    break;
                }
                ask(link, left(deadline, Duration::from_millis(800)), |_| None::<()>)?;
            }
        }
        let mut found = None;
        for &route in self.routes() {
            link.write(&bragi_request(PROP_BATTERY, route))?;
            if let Some(level) = ask(link, left(deadline, Duration::from_secs(6)), bragi_level)? {
                found = Some((level, route));
                break;
            }
        }
        // Aucune réponse du casque : éteint ou hors de portée.
        let Some((level, route)) = found else { return Ok(Status::Off) };
        link.write(&bragi_request(PROP_BATTERY, route))?;
        let confirm = left(deadline, Duration::from_secs(3));
        if let Some(second) = ask(link, confirm, bragi_level)?
            && second != level
        {
            anyhow::bail!("deux réponses différentes ({level} % / {second} %) : refusées");
        }
        let mut charging = false;
        if Instant::now() < deadline && link.write(&bragi_request(PROP_CHARGE, route)).is_ok() {
            charging = ask(link, left(deadline, Duration::from_secs(3)), bragi_charge)?.unwrap_or(false);
        }
        Ok(Status::Battery { level, charging })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn info(pid: u16, usage_page: u16, usage: u16, interface: i32) -> HidInfo {
        HidInfo { vid: VID, pid, usage_page, usage, interface }
    }

    #[test]
    fn probe_picks_the_right_collection() {
        let probe = FAMILY.probe;
        assert_eq!(probe(&info(0x2A08, 0xFF00, 1, 4)), Some("Corsair Void v2 Wireless"));
        assert_eq!(probe(&info(0x2A08, 0xFF00, 1, 3)), None);
        assert_eq!(probe(&info(0x0A40, BRAGI_PAGE, 1, 3)), Some("Corsair Virtuoso RGB Wireless SE"));
        assert_eq!(probe(&info(0x0A64, BRAGI_PAGE, 1, 3)), Some("Corsair Virtuoso RGB Wireless XT"));
        assert_eq!(probe(&info(0x0A40, BRAGI_PAGE, 2, 3)), None);
        assert_eq!(probe(&info(0x0A40, 0x000C, 1, 3)), None);
        // Souris et inconnus.
        assert_eq!(probe(&info(0x1B7F, BRAGI_PAGE, 1, 1)), None);
        assert_eq!(probe(&HidInfo { vid: 0x046D, ..info(0x2A08, 0xFF00, 1, 4) }), None);
    }

    // --- Ancien protocole ---

    /// Faux récepteur Void : répond à chaque requête ; `on` = casque allumé.
    struct Void {
        on: bool,
        tenths: u16,
        queue: VecDeque<Vec<u8>>,
        writes: Vec<Vec<u8>>,
    }

    impl Void {
        fn new(on: bool, tenths: u16) -> Void {
            Void { on, tenths, queue: VecDeque::new(), writes: vec![] }
        }
    }

    impl Link for Void {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), 65);
            assert_eq!((w[0], w[1], w[3]), (0, 2, 2));
            self.writes.push(w.to_vec());
            let mut r = vec![0u8; 64];
            r[..3].copy_from_slice(&[0x01, w[2], w[4]]);
            match (w[2], w[4]) {
                (HEADSET, _) if !self.on => return Ok(()),
                (HEADSET, CMD_BATTERY) => r[4..6].copy_from_slice(&self.tenths.to_le_bytes()),
                _ => {}
            }
            self.queue.push_back(r);
            Ok(())
        }
        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    #[test]
    fn legacy_reads_level_without_charge() {
        let mut dev = Void::new(true, 873);
        assert_eq!(Legacy.read(&mut dev).unwrap(), Status::Battery { level: 87, charging: false });
        let cmds: Vec<_> = dev.writes.iter().map(|w| (w[2], w[4])).collect();
        assert_eq!(cmds, [(8, 0x13), (8, 0x12), (9, 0x12), (9, 0x0F)]);
    }

    #[test]
    fn legacy_headset_off_when_its_heartbeat_is_silent() {
        let mut dev = Void::new(false, 500);
        assert_eq!(Legacy.read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn legacy_unreadable_level_is_an_error_after_three_tries() {
        for tenths in [0, 1001, 0x2A08] {
            let mut dev = Void::new(true, tenths);
            assert!(Legacy.read(&mut dev).is_err());
            assert_eq!(dev.writes.iter().filter(|w| w[4] == CMD_BATTERY).count(), ATTEMPTS);
        }
    }

    // --- Protocole bragi ---

    /// Faux récepteur bragi : répond sur `answers_on` seulement (casque) ; `noise` glisse la
    /// réponse à une autre propriété, comme un autre logiciel ; `refused` = écritures refusées.
    struct Virtuoso {
        answers_on: Option<u8>,
        tenths: u16,
        charge: u16,
        noise: Option<u16>,
        queue: VecDeque<Vec<u8>>,
        writes: Vec<(u8, u8)>,
        battery_asks: usize,
    }

    impl Virtuoso {
        fn new(answers_on: Option<u8>) -> Virtuoso {
            Virtuoso { answers_on, tenths: 550, charge: 2, noise: None, queue: VecDeque::new(), writes: vec![], battery_asks: 0 }
        }
    }

    fn answer(v: u16) -> Vec<u8> {
        let mut r = vec![0u8; FRAME];
        r[..5].copy_from_slice(&[0x01, GET, 0x00, v as u8, (v >> 8) as u8]);
        r
    }

    impl Link for Virtuoso {
        fn write(&mut self, w: &[u8]) -> anyhow::Result<()> {
            assert_eq!(w.len(), FRAME);
            assert_eq!((w[0], w[2]), (0x02, GET), "requêtes « get » seulement");
            let (route, prop) = (w[1], w[3]);
            self.writes.push((route, prop));
            // Le récepteur accuse ses propres requêtes ; une erreur (`err` ≠ 0) n'est pas un niveau.
            if route == RECEIVER && Some(RECEIVER) != self.answers_on {
                let mut r = answer(0);
                r[2] = 0x05;
                self.queue.push_back(r);
                return Ok(());
            }
            if Some(route) != self.answers_on {
                return Ok(());
            }
            if let Some(n) = self.noise.take() {
                self.queue.push_back(answer(n));
            }
            match prop {
                PROP_BATTERY => {
                    self.battery_asks += 1;
                    self.queue.push_back(answer(self.tenths));
                }
                PROP_CHARGE => self.queue.push_back(answer(self.charge)),
                _ => self.queue.push_back(answer(0x0100)),
            }
            Ok(())
        }
        fn read(&mut self, _: i32) -> anyhow::Result<Vec<u8>> {
            Ok(self.queue.pop_front().unwrap_or_default())
        }
    }

    fn bragi(pid: u16) -> Bragi {
        Bragi { pid, budget: Duration::from_millis(200) }
    }

    #[test]
    fn bragi_reads_level_twice_and_charge() {
        let mut dev = Virtuoso::new(Some(HEADSET));
        assert_eq!(bragi(0x0A40).read(&mut dev).unwrap(), Status::Battery { level: 55, charging: false });
        assert_eq!(dev.battery_asks, 2);
        assert_eq!(&dev.writes[..3], &[(8, 0x13), (8, 0x12), (9, 0x12)], "séance du 0A40");
        dev = Virtuoso::new(Some(HEADSET));
        dev.charge = 1;
        assert_eq!(bragi(0x0A3E).read(&mut dev).unwrap(), Status::Battery { level: 55, charging: true });
        assert_eq!(dev.writes[0], (9, PROP_BATTERY), "pas de séance hors 0A40");
    }

    #[test]
    fn bragi_cable_tries_the_receiver_route_first_then_the_headset() {
        let mut dev = Virtuoso::new(Some(HEADSET));
        assert_eq!(bragi(0x0A62).read(&mut dev).unwrap(), Status::Battery { level: 55, charging: false });
        assert_eq!(&dev.writes[..2], &[(8, PROP_BATTERY), (9, PROP_BATTERY)]);
        let mut dev = Virtuoso::new(Some(RECEIVER));
        assert_eq!(bragi(0x0A3D).read(&mut dev).unwrap(), Status::Battery { level: 55, charging: false });
    }

    #[test]
    fn bragi_headset_off_when_nobody_answers_the_battery() {
        let mut dev = Virtuoso::new(None);
        assert_eq!(bragi(0x0A40).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn bragi_disagreeing_answers_are_refused() {
        let mut dev = Virtuoso::new(Some(HEADSET));
        dev.noise = Some(0x0300);
        assert!(bragi(0x0A64).read(&mut dev).is_err());
    }

    #[test]
    fn bragi_unreadable_level_means_off() {
        let mut dev = Virtuoso::new(Some(HEADSET));
        dev.tenths = 0;
        assert_eq!(bragi(0x0A40).read(&mut dev).unwrap(), Status::Off);
    }

    #[test]
    fn bragi_reply_layouts() {
        assert_eq!(bragi_value(&answer(0x0226)), Some(0x0226));
        assert_eq!(bragi_level(&answer(0x0226)), Some(55));
        // Variante `01 01 02 00 <valeur>`, et trame précédée de l'identifiant 0 (65 octets).
        let mut alt = vec![0u8; FRAME];
        alt[..6].copy_from_slice(&[0x01, 0x01, GET, 0x00, 0x26, 0x02]);
        assert_eq!(bragi_value(&alt), Some(0x0226));
        assert_eq!(bragi_value(&[&[0][..], &answer(0x0226)].concat()), Some(0x0226));
        // Erreur, autre en-tête, trame courte.
        let mut err = answer(0x0226);
        err[2] = 1;
        assert_eq!(bragi_value(&err), None);
        assert_eq!(bragi_value(&[0x02, GET, 0, 0x26, 0x02]), None);
        assert_eq!(bragi_value(&[0x01, GET, 0]), None);
        assert_eq!(bragi_charge(&answer(3)), None);
    }
}
