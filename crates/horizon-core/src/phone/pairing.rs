//! Appairage d'un téléphone : un jeton à usage unique, affiché dans un QR code, valable cinq
//! minutes. Les essais faux ne sont pas comptés : le jeton fait 128 bits, et une limite
//! laisserait n'importe quel appareil du réseau local faire tomber le code affiché.

use std::net::{IpAddr, Ipv4Addr};

/// Durée de vie d'une offre d'appairage.
pub const OFFER_MS: u64 = 300_000;

/// Refus d'un jeton d'appairage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Le bon jeton, mais trop tard.
    Expired,
    /// Le jeton a déjà servi.
    Used,
    /// Aucune offre en cours, ou mauvais jeton.
    Unknown,
}

impl Reject {
    /// Raison envoyée au téléphone (`pair.rejected`).
    pub fn reason(&self) -> &'static str {
        match self {
            Reject::Expired => "expired",
            Reject::Used => "used",
            Reject::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
struct Offer {
    token: String,
    at_ms: u64,
}

/// L'offre d'appairage en cours (une seule à la fois) et le dernier jeton qui a servi.
#[derive(Debug, Default)]
pub struct Pairing {
    offer: Option<Offer>,
    used: Option<String>,
}

impl Pairing {
    /// Nouvelle offre ; remplace celle en cours.
    pub fn offer(&mut self, token: String, now_ms: u64) {
        self.offer = Some(Offer { token, at_ms: now_ms });
    }

    /// Le téléphone présente `token`. Un succès consomme l'offre.
    pub fn redeem(&mut self, token: &str, now_ms: u64) -> Result<(), Reject> {
        // Le bon jeton consomme l'offre, même trop tard ; un mauvais la laisse en place.
        let Some(offer) = self.offer.take_if(|offer| same(&offer.token, token)) else {
            let used = self.offer.is_none() && self.used.as_deref().is_some_and(|u| same(u, token));
            return Err(if used { Reject::Used } else { Reject::Unknown });
        };
        if now_ms.saturating_sub(offer.at_ms) > OFFER_MS {
            return Err(Reject::Expired);
        }
        self.used = Some(offer.token);
        Ok(())
    }

    /// Millisecondes restantes de l'offre en cours.
    pub fn pending(&self, now_ms: u64) -> Option<u64> {
        let offer = self.offer.as_ref()?;
        OFFER_MS.checked_sub(now_ms.saturating_sub(offer.at_ms))
    }

    /// Retire l'offre en cours (page fermée, module désactivé).
    pub fn cancel(&mut self) {
        self.offer = None;
    }
}

/// Comparaison en temps constant : le temps de réponse ne dit rien du jeton attendu.
pub(super) fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Adresse du réseau local : seules celles-là peuvent se connecter.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(|v4| is_private(IpAddr::V4(v4))),
    }
}

/// Contenu du QR code d'appairage (`docs/PHONE.md`).
pub fn qr_payload(addrs: &[Ipv4Addr], host: &str, port: u16, fingerprint: &str, token: &str) -> String {
    let hosts = addrs.iter().map(Ipv4Addr::to_string).collect::<Vec<_>>().join(",");
    // `byte_serialize` écrit l'espace « + » (formulaires) ; un « + » du nom, lui, sort en %2B.
    let name = url::form_urlencoded::byte_serialize(host.as_bytes()).collect::<String>().replace('+', "%20");
    format!("horizon://pair?v=1&h={hosts}&n={name}&p={port}&fp={fingerprint}&t={token}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_works_once_within_five_minutes() {
        let mut p = Pairing::default();
        p.offer("abc".into(), 1_000);
        assert_eq!(p.pending(1_000), Some(OFFER_MS));
        assert_eq!(p.redeem("abc", 1_000 + OFFER_MS), Ok(()));
        assert_eq!(p.redeem("abc", 2_000), Err(Reject::Used));
        p.offer("def".into(), 10_000);
        assert_eq!(p.redeem("def", 10_001 + OFFER_MS), Err(Reject::Expired));
        assert_eq!(Pairing::default().redeem("abc", 0), Err(Reject::Unknown));
    }

    #[test]
    fn wrong_tokens_never_cancel_the_offer() {
        // Un appareil quelconque du réseau local ne doit pas pouvoir faire tomber le code
        // affiché : le jeton fait 128 bits (hors de portée d'une recherche), l'offre expire seule.
        let mut p = Pairing::default();
        p.offer("abc".into(), 0);
        for _ in 0..1_000 {
            assert_eq!(p.redeem("non", 1), Err(Reject::Unknown));
        }
        assert_eq!(p.pending(1), Some(OFFER_MS - 1));
        assert_eq!(p.redeem("abc", 2), Ok(()));
    }

    #[test]
    fn only_private_addresses_are_local() {
        for ok in ["10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.49", "169.254.3.4", "127.0.0.1"] {
            assert!(is_private(ok.parse().unwrap()), "{ok}");
        }
        for no in ["8.8.8.8", "172.32.0.1", "192.169.1.1", "100.64.0.1", "2001:db8::1"] {
            assert!(!is_private(no.parse().unwrap()), "{no}");
        }
    }

    #[test]
    fn the_qr_carries_everything_the_phone_needs() {
        let q = qr_payload(&["192.168.1.49".parse().unwrap(), "10.0.0.5".parse().unwrap()], "PC Été", 28680, "AbC-_9", "tok");
        assert_eq!(q, "horizon://pair?v=1&h=192.168.1.49,10.0.0.5&n=PC%20%C3%89t%C3%A9&p=28680&fp=AbC-_9&t=tok");
    }
}
