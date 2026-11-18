//! Démarrage avec Windows : valeur `Horizon` sous `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SAM_FLAGS, REG_SZ, RegCloseKey,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR, w};

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: PCWSTR = w!("Horizon");

fn open(access: REG_SAM_FLAGS) -> anyhow::Result<HKEY> {
    let mut key = HKEY::default();
    // SAFETY : chaînes constantes, sortie HKEY valide.
    unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN, None, access, &mut key) }.ok()?;
    Ok(key)
}

/// Vrai si Horizon démarre avec Windows.
pub fn is_enabled() -> bool {
    let Ok(key) = open(KEY_READ) else { return false };
    // SAFETY : interrogation de présence seulement (pas de tampon).
    let found = unsafe { RegQueryValueExW(key, NAME, None, None, None, None) }.is_ok();
    // SAFETY : clé ouverte ci-dessus.
    let _ = unsafe { RegCloseKey(key) };
    found
}

/// Active (avec le chemin de l'exécutable courant) ou retire le démarrage automatique.
pub fn set(enabled: bool) -> anyhow::Result<()> {
    let key = open(KEY_SET_VALUE)?;
    let result = if enabled {
        let exe = std::env::current_exe()?;
        let value = HSTRING::from(format!("\"{}\"", exe.display()));
        // SAFETY : vue en octets du tampon UTF-16 terminé par zéro de `value`.
        let bytes = unsafe {
            std::slice::from_raw_parts(value.as_ptr() as *const u8, (value.len() + 1) * 2)
        };
        // SAFETY : clé ouverte en écriture, données REG_SZ valides.
        unsafe { RegSetValueExW(key, NAME, None, REG_SZ, Some(bytes)) }.ok()
    } else {
        // SAFETY : clé ouverte en écriture ; valeur absente = erreur ignorée.
        let _ = unsafe { RegDeleteValueW(key, NAME) };
        Ok(())
    };
    // SAFETY : clé ouverte ci-dessus.
    let _ = unsafe { RegCloseKey(key) };
    result.map_err(Into::into)
}
