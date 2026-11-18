//! Batterie des casques et écouteurs Bluetooth, telle que Windows la connaît (logique dans
//! `horizon_core::headset::bluetooth`).
//!
//! Nœuds PnP présents des énumérateurs `BTHENUM`, `BTHLE` et `BTHLEDEVICE` lus par SetupAPI
//! (batterie, Class of Device, drapeau « connecté », nom), puis, pour les seuls appareils qui
//! ont une batterie et peuvent être un casque, l'état de connexion WinRT interrogé par adresse
//! (sans balayage radio : c'est ce qu'affiche Paramètres). Lectures seulement.

use horizon_core::headset::bluetooth::{self, Device, Kind, Node};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_ALLCLASSES, DIGCF_PRESENT, HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW, SetupDiGetDevicePropertyW,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_FriendlyName, DEVPROP_TYPE_BOOLEAN, DEVPROP_TYPE_BYTE, DEVPROP_TYPE_STRING, DEVPROP_TYPE_UINT32,
    DEVPROPTYPE,
};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::core::{GUID, HSTRING, PCWSTR};
use windows_future::{AsyncStatus, IAsyncOperation};

/// `DEVPKEY_Bluetooth_Battery` : niveau en %, octet.
const KEY_BATTERY: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5), pid: 2 };
/// Class of Device du Bluetooth classique (entier 32 bits).
const KEY_CLASS: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a), pid: 4 };
/// Drapeau « connecté » non documenté (booléen), repli quand WinRT ne répond pas.
const KEY_CONNECTED: DEVPROPKEY =
    DEVPROPKEY { fmtid: GUID::from_u128(0x83da6326_97a6_4088_9453_a1923f573b29), pid: 15 };

const ENUMERATORS: [&str; 3] = ["BTHENUM", "BTHLE", "BTHLEDEVICE"];
/// Une pile Bluetooth figée ne doit pas bloquer le fil du casque.
const WINRT_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq)]
pub struct BluetoothHeadset {
    pub name: String,
    pub level: u8,
}

/// Lecteur : garde le dernier niveau connu de chaque appareil (la propriété disparaît parfois
/// un instant).
#[derive(Default)]
pub struct Reader {
    last_levels: HashMap<String, u8>,
}

impl Reader {
    /// Premier casque Bluetooth connecté dont Windows connaît la batterie. Sans adaptateur
    /// Bluetooth ou sans réponse de WinRT, rien de plus grave qu'aucun casque.
    pub fn poll(&mut self) -> Option<BluetoothHeadset> {
        for mut d in devices() {
            let level = match d.level {
                Some(l) => {
                    self.last_levels.insert(d.mac.clone(), l);
                    l
                }
                None => match self.last_levels.get(&d.mac) {
                    Some(&l) => l,
                    None => continue,
                },
            };
            if d.kind == Kind::Other {
                continue;
            }
            let (connected, class) = match winrt_state(&d) {
                Ok(s) => s,
                Err(e) => {
                    log::debug!("casque Bluetooth {} : WinRT : {e:#}", d.mac);
                    (None, None)
                }
            };
            if d.kind == Kind::Unknown
                && let Some(c) = class.filter(|&c| c != 0)
            {
                d.kind = bluetooth::kind_from_class(c, d.le);
            }
            if d.is_headset() && bluetooth::is_connected(connected, d.pnp_connected) {
                return Some(BluetoothHeadset { name: d.name, level });
            }
        }
        None
    }
}

/// Appareils Bluetooth présents (appairés), tels que lus en PnP, sans interroger WinRT.
pub fn devices() -> Vec<Device> {
    bluetooth::group(&nodes())
}

/// Liste d'appareils SetupAPI, détruite en fin de portée.
struct DevInfo(HDEVINFO);

impl Drop for DevInfo {
    fn drop(&mut self) {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

/// Nœuds PnP Bluetooth présents.
fn nodes() -> Vec<Node> {
    let mut out = Vec::new();
    for e in ENUMERATORS {
        let e = HSTRING::from(e);
        // Énumérateur absent (pas d'adaptateur Bluetooth) : liste vide ou erreur, sans gravité.
        let Ok(set) = (unsafe { SetupDiGetClassDevsW(None, PCWSTR(e.as_ptr()), None, DIGCF_PRESENT | DIGCF_ALLCLASSES) })
        else {
            continue;
        };
        let set = DevInfo(set);
        for i in 0.. {
            let mut data = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if unsafe { SetupDiEnumDeviceInfo(set.0, i, &mut data) }.is_err() {
                break;
            }
            let Some(instance_id) = instance_id(&set, &data) else { continue };
            out.push(Node {
                instance_id,
                name: string(&set, &data, &DEVPKEY_Device_FriendlyName),
                battery: property(&set, &data, &KEY_BATTERY, DEVPROP_TYPE_BYTE).and_then(|b| b.first().copied()),
                class: property(&set, &data, &KEY_CLASS, DEVPROP_TYPE_UINT32)
                    .and_then(|b| Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))),
                connected: property(&set, &data, &KEY_CONNECTED, DEVPROP_TYPE_BOOLEAN)
                    .and_then(|b| b.first().map(|&v| v != 0)),
            });
        }
    }
    out
}

fn instance_id(set: &DevInfo, data: &SP_DEVINFO_DATA) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut len = 0u32;
    unsafe { SetupDiGetDeviceInstanceIdW(set.0, data, Some(&mut buf), Some(&mut len)) }.ok()?;
    Some(utf16(&buf[..(len as usize).min(buf.len())]))
}

/// Valeur brute d'une propriété, si elle existe et a le type attendu.
fn property(set: &DevInfo, data: &SP_DEVINFO_DATA, key: &DEVPROPKEY, ty: DEVPROPTYPE) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; 512];
    let mut kind = DEVPROPTYPE::default();
    let mut len = 0u32;
    unsafe { SetupDiGetDevicePropertyW(set.0, data, key, &mut kind, Some(&mut buf), Some(&mut len), 0) }.ok()?;
    if kind != ty {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}

fn string(set: &DevInfo, data: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let b = property(set, data, key, DEVPROP_TYPE_STRING)?;
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(utf16(&w)).filter(|s| !s.is_empty())
}

/// Chaîne UTF-16 jusqu'au premier zéro.
fn utf16(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// État WinRT de l'appareil, interrogé par adresse : connecté, et sa catégorie (Class of
/// Device ou Appearance). `None` si Windows ne le connaît pas.
fn winrt_state(d: &Device) -> anyhow::Result<(Option<bool>, Option<u32>)> {
    let connected = |s: BluetoothConnectionStatus| s == BluetoothConnectionStatus::Connected;
    if d.le {
        let Some(dev) = wait(&BluetoothLEDevice::FromBluetoothAddressAsync(d.address())?)? else {
            return Ok((None, None));
        };
        let class = dev.Appearance().and_then(|a| a.RawValue()).ok().map(u32::from);
        let state = (Some(connected(dev.ConnectionStatus()?)), class);
        let _ = dev.Close();
        Ok(state)
    } else {
        let Some(dev) = wait(&BluetoothDevice::FromBluetoothAddressAsync(d.address())?)? else {
            return Ok((None, None));
        };
        let class = dev.ClassOfDevice().and_then(|c| c.RawValue()).ok();
        let state = (Some(connected(dev.ConnectionStatus()?)), class);
        let _ = dev.Close();
        Ok(state)
    }
}

/// Résultat d'une opération WinRT ; `None` si elle rend un objet nul (appareil inconnu).
fn wait<T: windows::core::RuntimeType + 'static>(op: &IAsyncOperation<T>) -> anyhow::Result<Option<T>> {
    let start = Instant::now();
    loop {
        match op.Status()? {
            AsyncStatus::Completed => {
                return match op.GetResults() {
                    Ok(v) => Ok(Some(v)),
                    // Objet nul : `from_abi` le refuse.
                    Err(e) if e.code().is_ok() => Ok(None),
                    Err(e) => Err(e.into()),
                };
            }
            AsyncStatus::Started if start.elapsed() < WINRT_TIMEOUT => std::thread::sleep(Duration::from_millis(1)),
            AsyncStatus::Started => {
                let _ = op.Cancel();
                anyhow::bail!("délai dépassé");
            }
            s => anyhow::bail!("opération terminée avec l'état {}", s.0),
        }
    }
}
