//! Adresses du PC sur le réseau local et son nom : ce que le QR d'appairage du module
//! « Téléphone » donne au téléphone pour le joindre.

use std::net::Ipv4Addr;
use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS};
use windows::Win32::NetworkManagement::IpHelper::{
    GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
    IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};
use windows::Win32::System::SystemInformation::{ComputerNameDnsHostname, GetComputerNameExW};
use windows::core::PWSTR;

/// Adresses IPv4 privées des cartes actives, celles d'une carte qui a une passerelle d'abord
/// (le réseau de la maison avant un réseau virtuel sans sortie). Vide : pas de réseau local.
pub fn local_ipv4() -> Vec<Ipv4Addr> {
    order(adapters())
}

/// Garde les adresses privées, sans doublon, passerelle d'abord.
fn order(found: Vec<(Ipv4Addr, bool)>) -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for with_gateway in [true, false] {
        for (ip, gateway) in &found {
            if *gateway == with_gateway && ip.is_private() && !out.contains(ip) {
                out.push(*ip);
            }
        }
    }
    out
}

/// Adresses IPv4 des cartes actives, avec « cette carte a une passerelle ».
fn adapters() -> Vec<(Ipv4Addr, bool)> {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    // Tampon de u64 : aligné pour IP_ADAPTER_ADDRESSES_LH. Windows conseille 15 Ko au départ.
    let mut buffer = vec![0u64; 2048];
    let mut found = Vec::new();
    for _ in 0..3 {
        let mut size = (buffer.len() * size_of::<u64>()) as u32;
        let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY : `first` désigne `size` octets alignés, que Windows remplit d'une liste
        // chaînée dont tous les pointeurs restent dans ce tampon.
        let code = unsafe { GetAdaptersAddresses(u32::from(AF_INET.0), flags, None, Some(first), &mut size) };
        if code == ERROR_BUFFER_OVERFLOW.0 {
            buffer.resize((size as usize).div_ceil(size_of::<u64>()), 0);
            continue;
        }
        if code != ERROR_SUCCESS.0 {
            log::warn!("réseau local : cartes illisibles (code {code})");
            break;
        }
        let mut adapter = first;
        // SAFETY : liste chaînée terminée par un pointeur nul, lue dans le tampon rempli
        // ci-dessus et encore vivant.
        unsafe {
            while let Some(a) = adapter.as_ref() {
                if a.OperStatus == IfOperStatusUp && a.IfType != IF_TYPE_SOFTWARE_LOOPBACK {
                    let gateway = !a.FirstGatewayAddress.is_null();
                    let mut unicast = a.FirstUnicastAddress;
                    while let Some(u) = unicast.as_ref() {
                        if let Some(sa) = u.Address.lpSockaddr.as_ref()
                            && sa.sa_family == AF_INET
                        {
                            let v4 = &*u.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            // S_addr est en ordre réseau : ses octets en mémoire sont l'adresse.
                            found.push((Ipv4Addr::from(v4.sin_addr.S_un.S_addr.to_ne_bytes()), gateway));
                        }
                        unicast = u.Next;
                    }
                }
                adapter = a.Next;
            }
        }
        break;
    }
    found
}

/// Nom du PC (celui que Windows annonce sur le réseau, `<nom>.local`) ; « PC » s'il est
/// illisible.
pub fn host_name() -> String {
    let mut buffer = [0u16; 256];
    let mut len = buffer.len() as u32;
    // SAFETY : `buffer` a `len` caractères ; Windows y écrit le nom et sa longueur dans `len`.
    let ok = unsafe { GetComputerNameExW(ComputerNameDnsHostname, Some(PWSTR(buffer.as_mut_ptr())), &mut len) }.is_ok();
    let name = if ok { String::from_utf16_lossy(&buffer[..(len as usize).min(buffer.len())]) } else { String::new() };
    if name.is_empty() { "PC".to_owned() } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_addresses_first_those_with_a_gateway() {
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        let found = vec![(ip("10.8.0.2"), false), (ip("8.8.8.8"), true), (ip("192.168.1.49"), true), (ip("169.254.1.2"), true), (ip("192.168.1.49"), true), (ip("127.0.0.1"), false)];
        assert_eq!(order(found), vec![ip("192.168.1.49"), ip("10.8.0.2")]);
        assert!(order(Vec::new()).is_empty());
    }

    #[test]
    fn this_pc_has_a_name() {
        let name = host_name();
        assert!(!name.is_empty() && !name.contains(' '), "{name}");
    }

    /// Contrôle à la main : `cargo test -p horizon-sys local_net -- --ignored --nocapture`.
    #[test]
    #[ignore = "lit les cartes réseau de ce PC"]
    fn show_the_addresses_of_this_pc() {
        println!("{} : {:?}", host_name(), local_ipv4());
    }
}
