//! Accès HID aux casques (protocoles dans `horizon_core::headset`), et à défaut casques et
//! écouteurs Bluetooth dont Windows connaît la batterie (`bluetooth_battery`).
//!
//! Collections dont le pilote reconnaît des avis spontanés (`Driver::notice`) : écoutées en
//! continu sur un fil, chaque avis appelle le réveil donné à `Reader::on_notice` (relecture
//! immédiate au lieu d'attendre la suivante).

use crate::bluetooth_battery;
use horizon_core::headset::{self, Driver, HidInfo, Link, Status};
use hidapi::{HidApi, HidDevice};
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Taille du tampon de lecture : les rapports les plus longs (Razer) font 91 octets.
const BUF: usize = 128;
/// Attente d'un rapport par le fil d'écoute, entre deux vérifications de son drapeau.
const LISTEN_MS: i32 = 500;

type Notify = Arc<dyn Fn() + Send + Sync>;

struct Hid(HidDevice);

impl Link for Hid {
    fn write(&mut self, report: &[u8]) -> anyhow::Result<()> {
        self.0.write(report)?;
        Ok(())
    }

    fn read(&mut self, timeout_ms: i32) -> anyhow::Result<Vec<u8>> {
        let mut buf = [0u8; BUF];
        let n = self.0.read_timeout(&mut buf, timeout_ms)?;
        Ok(buf[..n].to_vec())
    }

    fn send_feature(&mut self, report: &[u8]) -> anyhow::Result<()> {
        self.0.send_feature_report(report)?;
        Ok(())
    }

    fn get_feature(&mut self, report_id: u8, len: usize) -> anyhow::Result<Vec<u8>> {
        let mut buf = vec![0u8; len.max(1)];
        buf[0] = report_id;
        let n = self.0.get_feature_report(&mut buf)?;
        buf.truncate(n);
        Ok(buf)
    }
}

/// Casques branchés (leur récepteur) et leur batterie.
pub struct Reader {
    api: HidApi,
    /// Pilote de chaque collection suivie (état gardé entre deux lectures).
    drivers: HashMap<CString, Box<dyn Driver>>,
    bluetooth: bluetooth_battery::Reader,
    notify: Option<Notify>,
    /// Collections écoutées : drapeau « en vie » partagé avec leur fil (baissé par le fil si
    /// la collection disparaît, par le lecteur pour l'arrêter).
    listening: HashMap<CString, Arc<AtomicBool>>,
    /// Fabricant et produit USB des récepteurs reconnus à la dernière lecture.
    present: Vec<(u16, u16)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Headset {
    pub name: String,
    pub level: u8,
    pub charging: bool,
}

impl Reader {
    pub fn new() -> anyhow::Result<Reader> {
        Ok(Reader {
            api: HidApi::new()?,
            drivers: HashMap::new(),
            bluetooth: Default::default(),
            notify: None,
            listening: HashMap::new(),
            present: Vec::new(),
        })
    }

    /// Fabricant et produit USB des récepteurs (ou casques filaires) reconnus à la dernière
    /// lecture, allumés ou non : de quoi retrouver la sortie audio du casque
    /// (`mixer::outputs_of_usb`).
    pub fn present(&self) -> &[(u16, u16)] {
        &self.present
    }

    /// Réveil à appeler (depuis un autre fil) quand un casque annonce un changement d'état.
    pub fn on_notice(&mut self, notify: impl Fn() + Send + Sync + 'static) {
        self.notify = Some(Arc::new(notify));
    }

    /// Écoute la collection si son pilote reconnaît des avis et qu'elle ne l'est pas déjà.
    fn listen(&mut self, path: &CString, notice: Option<fn(&[u8]) -> bool>) {
        let (Some(notify), Some(notice)) = (&self.notify, notice) else { return };
        if self.listening.get(path).is_some_and(|alive| alive.load(Ordering::Relaxed)) {
            return;
        }
        let Ok(dev) = self.api.open_path(path) else { return };
        let alive = Arc::new(AtomicBool::new(true));
        self.listening.insert(path.clone(), alive.clone());
        let notify = notify.clone();
        let spawned = std::thread::Builder::new().name("casque : avis".into()).spawn(move || {
            let mut buf = [0u8; BUF];
            while alive.load(Ordering::Relaxed) {
                match dev.read_timeout(&mut buf, LISTEN_MS) {
                    Ok(n) if n > 0 && notice(&buf[..n]) => notify(),
                    Ok(_) => {}
                    // Collection perdue (débranchée, veille) : rouverte à la lecture suivante.
                    Err(_) => alive.store(false, Ordering::Relaxed),
                }
            }
        });
        if spawned.is_err() {
            self.listening.remove(path);
        }
    }

    /// Batterie du premier casque allumé ; `None` sans casque reconnu ou casques éteints.
    /// Un casque HID allumé passe avant un casque Bluetooth. Erreur seulement si aucun casque
    /// n'a pu être lu, qu'aucun n'a répondu « éteint » (un même récepteur a souvent plusieurs
    /// collections reconnues, dont certaines muettes) et qu'au moins un casque HID a échoué.
    pub fn poll(&mut self) -> anyhow::Result<Option<Headset>> {
        self.api.reset_devices()?;
        for vid in headset::vids() {
            self.api.add_devices(vid, 0)?;
        }
        let found: Vec<(CString, HidInfo, &'static headset::Family, &'static str)> = self
            .api
            .device_list()
            .filter_map(|d| {
                let info = HidInfo {
                    vid: d.vendor_id(),
                    pid: d.product_id(),
                    usage_page: d.usage_page(),
                    usage: d.usage(),
                    interface: d.interface_number(),
                };
                let (family, name) = headset::find(&info)?;
                Some((d.path().to_owned(), info, family, name))
            })
            .collect();
        self.drivers.retain(|p, _| found.iter().any(|(q, ..)| q == p));
        self.present = found.iter().map(|(_, info, ..)| (info.vid, info.pid)).collect();
        self.present.dedup();
        self.listening.retain(|p, alive| {
            let keep = found.iter().any(|(q, ..)| q == p);
            if !keep {
                alive.store(false, Ordering::Relaxed);
            }
            keep
        });
        let mut error = None;
        let mut off = false;
        for (path, info, family, name) in found {
            let notice = self.drivers.entry(path.clone()).or_insert_with(|| (family.new)(&info)).notice();
            self.listen(&path, notice);
            let driver = self.drivers.get_mut(&path).expect("pilote inséré juste avant");
            let read = self.api.open_path(&path).map_err(anyhow::Error::from).and_then(|d| driver.read(&mut Hid(d)));
            match read {
                Ok(Status::Battery { level, charging }) => {
                    return Ok(Some(Headset { name: name.to_owned(), level, charging }));
                }
                Ok(Status::Off) => off = true,
                Err(e) => {
                    error.get_or_insert(e.context(format!("{name} ({})", family.name)));
                }
            }
        }
        if let Some(b) = self.bluetooth.poll() {
            return Ok(Some(Headset { name: b.name, level: b.level, charging: false }));
        }
        match error {
            Some(e) if !off => Err(e),
            _ => Ok(None),
        }
    }
}
