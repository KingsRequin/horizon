//! Couleur d'accent de Windows : lue et écrite dans le registre de l'utilisateur, comme le fait
//! Paramètres › Personnalisation › Couleurs, puis annoncée (`WM_SETTINGCHANGE`
//! « ImmersiveColorSet ») pour que le menu Démarrer, les paramètres et les applis la reprennent.

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_BINARY, REG_DWORD, REG_VALUE_TYPE,
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};
use windows::core::{PCWSTR, w};

const ACCENT: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");
const DWM: PCWSTR = w!("Software\\Microsoft\\Windows\\DWM");

/// Valeurs du registre qui font la couleur d'accent (pour la remettre telle quelle).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccentValues {
    /// `AccentPalette` : 8 × RGBA, du plus clair au plus sombre.
    pub palette: Vec<u8>,
    /// `AccentColorMenu` (ABGR) : l'accent.
    pub menu: u32,
    /// `StartColorMenu` (ABGR) : sombre 1.
    pub start: u32,
    /// DWM `AccentColor` (ABGR), `ColorizationColor` et `ColorizationAfterglow` (ARGB).
    pub dwm_accent: u32,
    pub colorization: u32,
    pub afterglow: u32,
}

impl AccentValues {
    /// Valeurs d'une palette (`horizon_core::accent::windows_palette`).
    pub fn from_palette(p: [[u8; 4]; 8]) -> AccentValues {
        let abgr = |[r, g, b, _]: [u8; 4]| 0xFF00_0000 | (b as u32) << 16 | (g as u32) << 8 | r as u32;
        let [r, g, b, _] = p[3];
        let argb = 0xC400_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32;
        AccentValues {
            palette: p.concat(),
            menu: abgr(p[3]),
            start: abgr(p[4]),
            dwm_accent: abgr(p[3]),
            colorization: argb,
            afterglow: argb,
        }
    }
}

struct Key(HKEY);

impl Key {
    fn open(path: PCWSTR, write: bool) -> anyhow::Result<Key> {
        let mut key = HKEY::default();
        let access = if write { KEY_READ | KEY_SET_VALUE } else { KEY_READ };
        // SAFETY : chemin constant, sortie HKEY valide.
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path, None, access, &mut key) }.ok()?;
        Ok(Key(key))
    }

    fn bytes(&self, name: PCWSTR) -> anyhow::Result<Vec<u8>> {
        let mut buf = vec![0u8; 64];
        let mut len = buf.len() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        // SAFETY : tampon de `len` octets.
        unsafe { RegQueryValueExW(self.0, name, None, Some(&mut kind), Some(buf.as_mut_ptr()), Some(&mut len)) }
            .ok()?;
        buf.truncate(len as usize);
        Ok(buf)
    }

    fn dword(&self, name: PCWSTR) -> anyhow::Result<u32> {
        let b = self.bytes(name)?;
        anyhow::ensure!(b.len() == 4, "valeur DWORD attendue");
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn set_bytes(&self, name: PCWSTR, data: &[u8]) -> anyhow::Result<()> {
        // SAFETY : clé ouverte en écriture, données valides le temps de l'appel.
        unsafe { RegSetValueExW(self.0, name, None, REG_BINARY, Some(data)) }.ok()?;
        Ok(())
    }

    fn set_dword(&self, name: PCWSTR, v: u32) -> anyhow::Result<()> {
        // SAFETY : clé ouverte en écriture, 4 octets.
        unsafe { RegSetValueExW(self.0, name, None, REG_DWORD, Some(&v.to_le_bytes())) }.ok()?;
        Ok(())
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY : clé ouverte par `open`.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// Couleur d'accent actuelle de Windows.
pub fn read() -> anyhow::Result<AccentValues> {
    let (a, d) = (Key::open(ACCENT, false)?, Key::open(DWM, false)?);
    Ok(AccentValues {
        palette: a.bytes(w!("AccentPalette"))?,
        menu: a.dword(w!("AccentColorMenu"))?,
        start: a.dword(w!("StartColorMenu"))?,
        dwm_accent: d.dword(w!("AccentColor"))?,
        colorization: d.dword(w!("ColorizationColor"))?,
        afterglow: d.dword(w!("ColorizationAfterglow"))?,
    })
}

/// Écrit la couleur d'accent et l'annonce à Windows (bloque jusqu'à 2 s si une appli tarde).
pub fn write(v: &AccentValues) -> anyhow::Result<()> {
    {
        let (a, d) = (Key::open(ACCENT, true)?, Key::open(DWM, true)?);
        a.set_bytes(w!("AccentPalette"), &v.palette)?;
        a.set_dword(w!("AccentColorMenu"), v.menu)?;
        a.set_dword(w!("StartColorMenu"), v.start)?;
        d.set_dword(w!("AccentColor"), v.dwm_accent)?;
        d.set_dword(w!("ColorizationColor"), v.colorization)?;
        d.set_dword(w!("ColorizationAfterglow"), v.afterglow)?;
    }
    let topic = w!("ImmersiveColorSet");
    // SAFETY : chaîne constante terminée par zéro, vivante pendant l'envoi synchrone.
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(topic.as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_follow_windows_byte_orders() {
        let mut p = [[0u8, 0, 0, 0xFF]; 8];
        p[3] = [0x00, 0x78, 0xD4, 0xFF];
        p[4] = [0x00, 0x67, 0xC0, 0xFF];
        let v = AccentValues::from_palette(p);
        // Valeurs relevées sur la palette bleue d'origine de Windows.
        assert_eq!(v.menu, 0xFFD4_7800);
        assert_eq!(v.start, 0xFFC0_6700);
        assert_eq!(v.colorization, 0xC400_78D4);
        assert_eq!(v.palette.len(), 32);
        assert_eq!(&v.palette[12..16], &[0x00, 0x78, 0xD4, 0xFF]);
    }
}
