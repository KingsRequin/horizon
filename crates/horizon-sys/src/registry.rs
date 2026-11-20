//! Clé du registre de l'utilisateur (`HKEY_CURRENT_USER`), fermée en sortie de portée.

use crate::win32::{from_wide, to_wide};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD, REG_SZ, REG_VALUE_TYPE,
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

pub(crate) struct Key(HKEY);

impl Key {
    /// Ouvre une clé qui existe, en lecture ou en lecture et écriture.
    pub(crate) fn open(path: &str, write: bool) -> anyhow::Result<Key> {
        let path = to_wide(path);
        let mut key = HKEY::default();
        let access = if write { KEY_READ | KEY_SET_VALUE } else { KEY_READ };
        // SAFETY : chemin terminé par zéro, vivant pendant l'appel ; sortie HKEY valide.
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, access, &mut key) }.ok()?;
        Ok(Key(key))
    }

    /// Vrai si la valeur existe.
    pub(crate) fn has(&self, name: PCWSTR) -> bool {
        // SAFETY : interrogation de présence seulement (pas de tampon).
        unsafe { RegQueryValueExW(self.0, name, None, None, None, None) }.is_ok()
    }

    pub(crate) fn bytes(&self, name: PCWSTR) -> anyhow::Result<Vec<u8>> {
        let mut len = 0u32;
        // SAFETY : sans tampon, seule la taille est rendue.
        unsafe { RegQueryValueExW(self.0, name, None, None, None, Some(&mut len)) }.ok()?;
        let mut buf = vec![0u8; len as usize];
        // SAFETY : tampon de `len` octets.
        unsafe { RegQueryValueExW(self.0, name, None, None, Some(buf.as_mut_ptr()), Some(&mut len)) }.ok()?;
        buf.truncate(len as usize);
        Ok(buf)
    }

    pub(crate) fn dword(&self, name: PCWSTR) -> anyhow::Result<u32> {
        let b = self.bytes(name)?;
        anyhow::ensure!(b.len() == 4, "valeur DWORD attendue");
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Texte d'une valeur `REG_SZ` (jusqu'au premier zéro).
    pub(crate) fn string(&self, name: PCWSTR) -> anyhow::Result<String> {
        let wide: Vec<u16> = self.bytes(name)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Ok(from_wide(&wide))
    }

    fn set(&self, name: PCWSTR, kind: REG_VALUE_TYPE, data: &[u8]) -> anyhow::Result<()> {
        // SAFETY : clé ouverte en écriture, données valides le temps de l'appel.
        unsafe { RegSetValueExW(self.0, name, None, kind, Some(data)) }.ok()?;
        Ok(())
    }

    pub(crate) fn set_bytes(&self, name: PCWSTR, data: &[u8]) -> anyhow::Result<()> {
        self.set(name, REG_BINARY, data)
    }

    pub(crate) fn set_dword(&self, name: PCWSTR, v: u32) -> anyhow::Result<()> {
        self.set(name, REG_DWORD, &v.to_le_bytes())
    }

    pub(crate) fn set_string(&self, name: PCWSTR, v: &str) -> anyhow::Result<()> {
        let data: Vec<u8> = to_wide(v).into_iter().flat_map(u16::to_le_bytes).collect();
        self.set(name, REG_SZ, &data)
    }

    /// Retire une valeur ; absente, rien ne se passe.
    pub(crate) fn delete(&self, name: PCWSTR) {
        // SAFETY : clé ouverte en écriture.
        let _ = unsafe { RegDeleteValueW(self.0, name) };
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY : clé ouverte par `open`.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Clé d'essai créée pour un test, supprimée en sortie de portée.
#[cfg(test)]
pub(crate) struct Scratch(pub(crate) String);

#[cfg(test)]
impl Scratch {
    pub(crate) fn new(name: &str) -> Scratch {
        use windows::Win32::System::Registry::RegCreateKeyW;
        let path = format!("Software\\Horizon-essai-{name}-{}", std::process::id());
        let wide = to_wide(&path);
        let mut key = HKEY::default();
        // SAFETY : chemin terminé par zéro, sortie HKEY valide, refermée aussitôt.
        unsafe {
            RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), &mut key).ok().expect("clé d'essai");
            let _ = RegCloseKey(key);
        }
        Scratch(path)
    }
}

#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        use windows::Win32::System::Registry::RegDeleteKeyW;
        let wide = to_wide(&self.0);
        // SAFETY : chemin terminé par zéro, vivant pendant l'appel.
        let _ = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::w;

    #[test]
    fn values_round_trip() {
        let scratch = Scratch::new("valeurs");
        let key = Key::open(&scratch.0, true).unwrap();
        assert!(!key.has(w!("Texte")));
        key.set_string(w!("Texte"), "C:\\Applis\\Écran 2\\").unwrap();
        key.set_dword(w!("Nombre"), 0xC400_78D4).unwrap();
        key.set_bytes(w!("Octets"), &[7u8; 100]).unwrap();
        assert!(key.has(w!("Texte")));
        assert_eq!(key.string(w!("Texte")).unwrap(), "C:\\Applis\\Écran 2\\");
        assert_eq!(key.dword(w!("Nombre")).unwrap(), 0xC400_78D4);
        assert_eq!(key.bytes(w!("Octets")).unwrap(), vec![7u8; 100], "plus long que tout tampon fixe");
        assert!(key.dword(w!("Texte")).is_err());
        key.delete(w!("Texte"));
        key.delete(w!("Texte"));
        assert!(!key.has(w!("Texte")));
    }

    #[test]
    fn a_missing_key_does_not_open() {
        assert!(Key::open("Software\\Horizon-essai-absente", false).is_err());
    }
}
