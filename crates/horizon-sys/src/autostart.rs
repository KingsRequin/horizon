//! Démarrage avec Windows : valeur `Horizon` sous `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

use crate::registry::Key;
use windows::core::{PCWSTR, w};

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const NAME: PCWSTR = w!("Horizon");

/// Vrai si Horizon démarre avec Windows.
pub fn is_enabled() -> bool {
    Key::open(RUN, false).is_ok_and(|key| key.has(NAME))
}

/// Active (avec le chemin de l'exécutable courant) ou retire le démarrage automatique.
pub fn set(enabled: bool) -> anyhow::Result<()> {
    let key = Key::open(RUN, true)?;
    if enabled {
        let exe = std::env::current_exe()?;
        key.set_string(NAME, &format!("\"{}\"", exe.display()))
    } else {
        key.delete(NAME);
        Ok(())
    }
}
