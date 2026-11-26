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
    /// Ce que ce téléphone a le droit de faire ; absent d'un fichier de la 0.1 : les droits par
    /// défaut.
    #[serde(default = "default_granted")]
    pub granted: Vec<String>,
}

impl Phone {
    /// Ses droits, dans l'ordre de [`RIGHTS`].
    pub fn rights(&self) -> Vec<String> {
        RIGHTS.iter().filter(|r| self.granted.iter().any(|g| g == *r)).map(|r| (*r).to_owned()).collect()
    }
}

/// Droits qui existent. `battery` ne se retire pas ; `input` n'est jamais accordé d'office.
pub const RIGHTS: &[&str] = &["battery", "media", "sound", "input"];

/// Droits d'un téléphone qui vient d'être appairé.
pub fn default_granted() -> Vec<String> {
    ["battery", "media", "sound"].iter().map(|s| (*s).to_owned()).collect()
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
        self.phones.push(Phone { id: id.to_owned(), name: short_name(name), token_hash: hash_token(token), paired_ms: now_ms, last_seen_ms: now_ms, granted: default_granted() });
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

    /// Droits du téléphone `id`, dans l'ordre de [`RIGHTS`] ; vide s'il n'est pas connu.
    pub fn granted(&self, id: &str) -> Vec<String> {
        self.phones.iter().find(|p| p.id == id).map(Phone::rights).unwrap_or_default()
    }

    /// Accorde (`on`) ou retire le droit `right` au téléphone `id` ; vrai si quelque chose a
    /// changé. `battery` ne se retire pas, un droit inconnu n'existe pas.
    pub fn set_granted(&mut self, id: &str, right: &str, on: bool) -> bool {
        if !RIGHTS.contains(&right) || (right == "battery" && !on) {
            return false;
        }
        let Some(phone) = self.phones.iter_mut().find(|p| p.id == id) else { return false };
        let had = phone.granted.iter().any(|g| g == right);
        if had == on {
            return false;
        }
        if on {
            phone.granted.push(right.to_owned());
        } else {
            phone.granted.retain(|g| g != right);
        }
        true
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

    #[test]
    fn an_old_registry_gets_the_default_rights() {
        let old = r#"{"pc_id":"pc-1","phones":[{"id":"tel-1","name":"Pixel","token_hash":"ab","paired_ms":1,"last_seen_ms":2}]}"#;
        let r = Registry::load(old);
        assert_eq!(r.granted("tel-1"), vec!["battery", "media", "sound"]);
        assert!(r.granted("inconnu").is_empty());
    }

    #[test]
    fn input_is_never_granted_by_default() {
        let mut r = Registry::default();
        r.add("tel-1", "Pixel", "s", 1);
        assert!(!r.granted("tel-1").contains(&"input".to_owned()));
        assert!(r.set_granted("tel-1", "input", true));
        assert!(!r.set_granted("tel-1", "input", true), "rien de changé");
        assert!(Registry::load(&r.to_json()).granted("tel-1").contains(&"input".to_owned()), "enregistré");
        r.add("tel-1", "Pixel", "s2", 2);
        assert!(!r.granted("tel-1").contains(&"input".to_owned()), "un nouvel appairage repart des droits par défaut");
    }

    #[test]
    fn battery_cannot_be_withdrawn_and_unknown_rights_do_not_exist() {
        let mut r = Registry::default();
        r.add("tel-1", "Pixel", "s", 1);
        assert!(!r.set_granted("tel-1", "battery", false));
        assert!(!r.set_granted("tel-1", "tout", true));
        assert!(!r.set_granted("inconnu", "media", false));
        assert!(r.set_granted("tel-1", "media", false));
        assert_eq!(r.granted("tel-1"), vec!["battery", "sound"]);
    }
}
