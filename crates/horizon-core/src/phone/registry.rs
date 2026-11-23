//! Téléphones appairés (`phones.json`). Horizon ne garde du jeton d'un téléphone que son
//! empreinte : le fichier lu par un tiers ne permet pas de se faire passer pour lui.

use super::pairing::same;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Téléphones gardés au plus ; au-delà, celui vu le moins récemment est retiré.
const MAX_PHONES: usize = 16;
/// Longueur maximale du nom affiché d'un téléphone.
pub(super) const MAX_NAME: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phone {
    pub id: String,
    pub name: String,
    /// SHA-256 du jeton permanent, en hexadécimal.
    pub token_hash: String,
    pub paired_ms: u64,
    pub last_seen_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Registry {
    /// Identifiant de ce PC, créé une fois et envoyé aux téléphones.
    pub pc_id: String,
    pub phones: Vec<Phone>,
}

/// Empreinte d'un jeton permanent.
pub fn hash_token(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Nom borné à [`MAX_NAME`] caractères.
pub(super) fn short_name(name: &str) -> String {
    name.chars().take(MAX_NAME).collect()
}

impl Registry {
    /// Registre lu dans `text` ; vide si le texte est illisible.
    pub fn load(text: &str) -> Registry {
        serde_json::from_str(text).unwrap_or_default()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("registre sans clé non textuelle")
    }

    /// Appaire le téléphone `id` ; un téléphone déjà connu reçoit son nouveau jeton.
    pub fn add(&mut self, id: &str, name: &str, token: &str, now_ms: u64) {
        self.phones.retain(|p| p.id != id);
        self.phones.push(Phone { id: id.to_owned(), name: short_name(name), token_hash: hash_token(token), paired_ms: now_ms, last_seen_ms: now_ms });
        while self.phones.len() > MAX_PHONES {
            let oldest = self.phones.iter().enumerate().min_by_key(|(_, p)| p.last_seen_ms).map(|(i, _)| i).expect("liste non vide");
            self.phones.remove(oldest);
        }
    }

    /// Le téléphone `id` présente `token` ; la raison du refus est celle de `hello.rejected`.
    pub fn check(&mut self, id: &str, token: &str, now_ms: u64) -> Result<(), &'static str> {
        let phone = self.phones.iter_mut().find(|p| p.id == id).ok_or("unknown_device")?;
        if !same(&phone.token_hash, &hash_token(token)) {
            return Err("bad_token");
        }
        phone.last_seen_ms = now_ms;
        Ok(())
    }

    /// Retire le téléphone `id` ; faux s'il n'était pas connu.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.phones.len();
        self.phones.retain(|p| p.id != id);
        self.phones.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paired_phone_is_recognised_by_its_token_only() {
        let mut r = Registry::default();
        r.add("tel-1", "Pixel", "secret", 100);
        assert!(!r.to_json().contains("secret"), "le jeton n'est jamais écrit");
        assert_eq!(r.check("tel-1", "secret", 500), Ok(()));
        assert_eq!(r.phones[0].last_seen_ms, 500);
        assert_eq!(r.check("tel-1", "autre", 600), Err("bad_token"));
        assert_eq!(r.check("tel-2", "secret", 600), Err("unknown_device"));
        assert!(r.remove("tel-1") && !r.remove("tel-1"));
    }

    #[test]
    fn the_file_survives_a_round_trip_and_garbage() {
        let mut r = Registry { pc_id: "pc-1".into(), ..Registry::default() };
        r.add("tel-1", &"n".repeat(200), "s", 1);
        let back = Registry::load(&r.to_json());
        assert_eq!((back.pc_id.as_str(), back.phones[0].name.chars().count()), ("pc-1", 40));
        assert!(Registry::load("{{{").phones.is_empty());
        for i in 0..20 {
            r.add(&format!("t{i}"), "x", "s", i);
        }
        assert_eq!(r.phones.len(), 16);
    }

    #[test]
    fn pairing_again_replaces_the_old_token() {
        let mut r = Registry::default();
        r.add("tel-1", "Pixel", "ancien", 1);
        r.add("tel-1", "Pixel 9", "nouveau", 2);
        assert_eq!(r.phones.len(), 1);
        assert_eq!(r.phones[0].name, "Pixel 9");
        assert_eq!(r.check("tel-1", "ancien", 3), Err("bad_token"));
        assert_eq!(r.check("tel-1", "nouveau", 3), Ok(()));
    }
}
