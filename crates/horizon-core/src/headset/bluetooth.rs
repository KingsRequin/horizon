//! Casques et écouteurs Bluetooth dont Windows connaît la batterie (ceux qu'affiche
//! Paramètres > Bluetooth et appareils) : logique pure, l'accès au système est dans
//! `horizon_sys::bluetooth_battery`.
//!
//! Méthode reprise de Halo Battery (licence MIT) : un appareil Bluetooth est plusieurs nœuds
//! PnP (la racine `BTHENUM\DEV_<MAC>` ou `BTHLE\DEV_<MAC>`, puis un nœud par service), tous
//! rattachés au même appareil par l'adresse MAC trouvée dans leur identifiant d'instance. La
//! batterie est la propriété `DEVPKEY_Bluetooth_Battery` (en %) de l'un d'eux ; le genre de
//! l'appareil vient de sa « Class of Device » (Bluetooth classique) ou de son « Appearance »
//! (Bluetooth basse consommation), à défaut de ses services audio.

/// Nœud PnP Bluetooth tel que lu par l'énumération.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Node {
    /// Identifiant d'instance (`BTHENUM\…`, `BTHLE\…`, `BTHLEDEVICE\…`).
    pub instance_id: String,
    /// Nom convivial du nœud.
    pub name: Option<String>,
    /// `DEVPKEY_Bluetooth_Battery`, en %.
    pub battery: Option<u8>,
    /// Class of Device (Bluetooth classique), propriété PnP `{2BD67D8B-…} 4`.
    pub class: Option<u32>,
    /// Drapeau « connecté » non documenté `{83DA6326-…} 15` : peu fiable (certains casques
    /// le disent faux pendant la lecture audio), seulement en dernier recours.
    pub connected: Option<bool>,
}

/// Genre d'un appareil, d'après sa catégorie Bluetooth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Casque, écouteurs, kit mains libres.
    Headset,
    /// Autre chose (clavier, souris, manette, enceinte, téléphone…).
    Other,
    /// Catégorie absente ou non classée.
    Unknown,
}

/// Appareil physique : ses nœuds regroupés par adresse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// Adresse MAC, 12 chiffres hexadécimaux en majuscules.
    pub mac: String,
    /// Bluetooth basse consommation (racine `BTHLE`).
    pub le: bool,
    pub name: String,
    pub level: Option<u8>,
    pub kind: Kind,
    /// Un de ses nœuds est un service audio de casque (A2DP, mains libres, profil casque).
    pub audio: bool,
    /// Drapeau PnP « connecté » : vrai si un des nœuds le dit.
    pub pnp_connected: Option<bool>,
}

/// Services audio d'un casque : récepteur A2DP, mains libres, profil casque (et sa variante).
const AUDIO_SERVICES: [&str; 4] = ["{0000110B-", "{0000111E-", "{00001108-", "{00001131-"];

impl Device {
    /// Adresse en nombre, pour l'interroger par WinRT.
    pub fn address(&self) -> u64 {
        u64::from_str_radix(&self.mac, 16).unwrap_or(0)
    }

    /// Casque ou écouteurs : d'après sa catégorie, à défaut d'après ses services audio.
    pub fn is_headset(&self) -> bool {
        match self.kind {
            Kind::Headset => true,
            Kind::Other => false,
            Kind::Unknown => self.audio,
        }
    }
}

fn is_hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// Adresse MAC d'un identifiant d'instance : celle qui suit `DEV_`, sinon la dernière suite
/// d'exactement 12 chiffres hexadécimaux (les nœuds de service la portent en fin d'identifiant).
pub fn mac_of(instance_id: &str) -> Option<String> {
    let s = instance_id.to_ascii_uppercase();
    let b = s.as_bytes();
    let exactly12 = |i: usize| {
        i + 12 <= b.len()
            && b[i..i + 12].iter().all(|&c| is_hex(c))
            && b.get(i + 12).is_none_or(|&c| !is_hex(c))
    };
    if let Some(i) = s.find("DEV_").map(|i| i + 4)
        && exactly12(i)
    {
        return Some(s[i..i + 12].to_owned());
    }
    let mut last = None;
    let mut i = 0;
    while i < b.len() {
        if !is_hex(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && is_hex(b[i]) {
            i += 1;
        }
        if i - start == 12 {
            last = Some(&s[start..i]);
        }
    }
    last.map(str::to_owned)
}

/// Racine d'un appareil (`BTHENUM\DEV_<MAC>` ou `BTHLE\DEV_<MAC>`) ; `Some(true)` pour BLE.
fn root_kind(instance_id: &str) -> Option<bool> {
    let s = instance_id.to_ascii_uppercase();
    let (le, rest) = if let Some(r) = s.strip_prefix("BTHENUM\\DEV_") {
        (false, r)
    } else {
        (true, s.strip_prefix("BTHLE\\DEV_")?)
    };
    (rest.len() >= 12 && rest.as_bytes()[..12].iter().all(|&c| is_hex(c))).then_some(le)
}

/// Genre d'après la catégorie : Class of Device (classique) ou Appearance (BLE).
pub fn kind_from_class(raw: u32, le: bool) -> Kind {
    if le {
        // Appearance : catégorie sur les 10 bits de poids fort (16 bits en tout).
        return match (raw >> 6) & 0x3FF {
            0 => Kind::Unknown,
            // « Wearable Audio Device » : écouteurs, casque, tour de cou.
            37 => Kind::Headset,
            _ => Kind::Other,
        };
    }
    // Class of Device : classe principale (bits 8-12) et secondaire (bits 2-7).
    let major = (raw >> 8) & 0x1F;
    let minor = (raw >> 2) & 0x3F;
    match (major, minor) {
        // Non classé.
        (0, _) | (0x1F, _) => Kind::Unknown,
        // Audio/vidéo : casque porté, mains libres, casque à écouteurs.
        (4, 1 | 2 | 6) => Kind::Headset,
        // Audio/vidéo non classé ou réservé : on s'en remet aux services.
        (4, 0 | 3) => Kind::Unknown,
        _ => Kind::Other,
    }
}

/// Regroupe les nœuds par appareil (ordre des adresses).
pub fn group(nodes: &[Node]) -> Vec<Device> {
    struct Slot {
        le: Option<bool>,
        root_name: Option<String>,
        battery_name: Option<String>,
        level: Option<u8>,
        class: Option<u32>,
        audio: bool,
        connected: Option<bool>,
    }
    let mut by_mac: std::collections::BTreeMap<String, Slot> = Default::default();
    for n in nodes {
        // Adresse nulle : services de l'adaptateur lui-même (ports série locaux).
        let Some(mac) = mac_of(&n.instance_id).filter(|m| m != "000000000000") else { continue };
        let g = by_mac.entry(mac).or_insert(Slot {
            le: None,
            root_name: None,
            battery_name: None,
            level: None,
            class: None,
            audio: false,
            connected: None,
        });
        let name = n.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(le) = root_kind(&n.instance_id) {
            g.le = Some(le);
            if let Some(name) = name {
                g.root_name.get_or_insert_with(|| name.to_owned());
            }
        } else if g.le.is_none() && n.instance_id.to_ascii_uppercase().starts_with("BTHLEDEVICE\\") {
            g.le = Some(true);
        }
        if let Some(level) = n.battery {
            g.level = Some(level.min(100));
            if let Some(name) = name {
                g.battery_name.get_or_insert_with(|| name.to_owned());
            }
        }
        if let Some(c) = n.class.filter(|&c| c != 0) {
            g.class.get_or_insert(c);
        }
        let id = n.instance_id.to_ascii_uppercase();
        g.audio |= AUDIO_SERVICES.iter().any(|u| id.contains(u));
        if let Some(c) = n.connected {
            g.connected = Some(g.connected.unwrap_or(false) || c);
        }
    }
    by_mac
        .into_iter()
        .map(|(mac, g)| {
            let le = g.le.unwrap_or(false);
            Device {
                name: g.root_name.or(g.battery_name).unwrap_or_else(|| format!("Bluetooth {mac}")),
                le,
                level: g.level,
                kind: g.class.map_or(Kind::Unknown, |c| kind_from_class(c, le)),
                audio: g.audio,
                pnp_connected: g.connected,
                mac,
            }
        })
        .collect()
}

/// Connecté maintenant : l'état WinRT (celui de Paramètres) s'il est connu, sinon le drapeau
/// PnP ; sans l'un ni l'autre, considéré comme déconnecté (la batterie lue peut être ancienne).
pub fn is_connected(winrt: Option<bool>, pnp: Option<bool>) -> bool {
    winrt.or(pnp).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Node {
        Node { instance_id: id.to_owned(), ..Node::default() }
    }

    const ROOT: &str = r"BTHENUM\DEV_00BB438DD420\7&2B830FC&0&BLUETOOTHDEVICE_00BB438DD420";
    const A2DP: &str = r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&00010094_PID&0004\7&2B830FC&0&00BB438DD420_C00000000";
    const HFP: &str = r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&00010094_PID&0004\7&2B830FC&0&00BB438DD420_C00000000";
    const PAD_ROOT: &str = r"BTHENUM\DEV_8D03BB5BDCCD\7&2B830FC&0&BLUETOOTHDEVICE_8D03BB5BDCCD";
    const PAD_HID: &str = r"BTHENUM\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&05C4\7&2B830FC&0&8D03BB5BDCCD_C00000000";

    #[test]
    fn mac_is_found_in_roots_and_service_nodes() {
        assert_eq!(mac_of(ROOT).as_deref(), Some("00BB438DD420"));
        // Le GUID du service finit aussi par 12 chiffres hexadécimaux : c'est la dernière suite qui compte.
        assert_eq!(mac_of(A2DP).as_deref(), Some("00BB438DD420"));
        assert_eq!(mac_of(r"bthle\dev_f1e2d3c4b5a6\8&1").as_deref(), Some("F1E2D3C4B5A6"));
        assert_eq!(
            mac_of(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&02046D_PID&B023_REV&0013_F1E2D3C4B5A6\8&2&0&000C")
                .as_deref(),
            Some("F1E2D3C4B5A6")
        );
        assert_eq!(mac_of("USB\\VID_046D"), None);
    }

    #[test]
    fn class_of_device_is_decoded() {
        // Redmi Buds 6 Pro : audio/vidéo, casque porté.
        assert_eq!(kind_from_class(0x0024_0404, false), Kind::Headset);
        // Casque à écouteurs, mains libres.
        assert_eq!(kind_from_class(0x0004_0418, false), Kind::Headset);
        assert_eq!(kind_from_class(0x0020_0408, false), Kind::Headset);
        // Manette DualShock 4 (périphérique), enceinte (audio/vidéo, haut-parleur), téléphone.
        assert_eq!(kind_from_class(0x2508, false), Kind::Other);
        assert_eq!(kind_from_class(0x0024_0414, false), Kind::Other);
        assert_eq!(kind_from_class(0x005A_020C, false), Kind::Other);
        // Non classé.
        assert_eq!(kind_from_class(0x1F00, false), Kind::Unknown);
        assert_eq!(kind_from_class(0x0400, false), Kind::Unknown);
    }

    #[test]
    fn appearance_is_decoded() {
        assert_eq!(kind_from_class(0x0941, true), Kind::Headset); // écouteurs
        assert_eq!(kind_from_class(0x0942, true), Kind::Headset); // casque
        assert_eq!(kind_from_class(0x03C2, true), Kind::Other); // souris
        assert_eq!(kind_from_class(0x03C1, true), Kind::Other); // clavier
        assert_eq!(kind_from_class(0, true), Kind::Unknown);
    }

    #[test]
    fn nodes_are_grouped_by_address() {
        let nodes = vec![
            Node { name: Some("Transport Avrcp Redmi".into()), battery: Some(80), connected: Some(false), ..node(HFP) },
            Node { name: Some("Redmi Buds 6 Pro".into()), class: Some(0x0024_0404), connected: Some(true), ..node(ROOT) },
            Node { name: Some("Redmi Buds 6 Pro".into()), ..node(A2DP) },
            Node { name: Some("Wireless Controller".into()), class: Some(0x2508), battery: Some(40), ..node(PAD_ROOT) },
            node(PAD_HID),
        ];
        let devices = group(&nodes);
        assert_eq!(devices.len(), 2);
        let buds = &devices[0];
        assert_eq!(buds.mac, "00BB438DD420");
        assert_eq!(buds.name, "Redmi Buds 6 Pro"); // la racine avant le nœud qui porte la batterie
        assert_eq!(buds.level, Some(80));
        assert_eq!(buds.kind, Kind::Headset);
        assert!(buds.audio && !buds.le && buds.is_headset());
        assert_eq!(buds.pnp_connected, Some(true));
        assert_eq!(buds.address(), 0x00BB_438D_D420);
        let pad = &devices[1];
        assert_eq!(pad.kind, Kind::Other);
        assert!(!pad.is_headset());
    }

    #[test]
    fn local_adapter_nodes_are_ignored() {
        let local = r"BTHENUM\{00001101-0000-1000-8000-00805F9B34FB}_LOCALMFG&0000&33DAABF9&0&000000000000_00000000";
        assert!(group(&[node(local)]).is_empty());
    }

    #[test]
    fn audio_services_decide_without_a_class() {
        let devices = group(&[Node { battery: Some(55), ..node(A2DP) }]);
        assert_eq!(devices[0].kind, Kind::Unknown);
        assert!(devices[0].is_headset());
        // Sans racine ni nom : nom tiré de l'adresse.
        assert_eq!(devices[0].name, "Bluetooth 00BB438DD420");
        // Une catégorie connue l'emporte sur les services (enceinte avec A2DP).
        let speaker = group(&[Node { class: Some(0x0024_0414), ..node(A2DP) }]);
        assert!(!speaker[0].is_headset());
    }

    #[test]
    fn low_energy_devices_are_flagged() {
        let devices = group(&[
            Node { battery: Some(30), ..node(r"BTHLEDEVICE\{0000180F-0000-1000-8000-00805F9B34FB}_DEV_VID&02046D_PID&B023_REV&0013_F1E2D3C4B5A6\8&2&0&000C") },
            Node { name: Some("MX Master".into()), ..node(r"BTHLE\DEV_F1E2D3C4B5A6\8&1") },
        ]);
        assert_eq!(devices.len(), 1);
        assert!(devices[0].le);
        assert_eq!(devices[0].name, "MX Master");
        assert_eq!(devices[0].kind, Kind::Unknown);
        assert!(!devices[0].is_headset());
    }

    #[test]
    fn connection_prefers_winrt() {
        assert!(is_connected(Some(true), Some(false)));
        assert!(!is_connected(Some(false), Some(true)));
        assert!(is_connected(None, Some(true)));
        assert!(!is_connected(None, None));
    }
}
