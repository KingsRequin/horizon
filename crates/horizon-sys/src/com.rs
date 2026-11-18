//! Initialisation COM d'un fil, vérifiée, et défaite quand la garde est lâchée.

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize,
};

/// COM initialisé sur le fil courant tant que la garde vit. Un fil déjà initialisé dans
/// l'autre modèle (le fil principal de Tauri est en appartement mono-fil) peut quand même
/// utiliser COM : la garde ne défait alors rien.
#[must_use = "COM est libéré quand la garde est lâchée"]
pub struct ComGuard {
    owned: bool,
}

impl ComGuard {
    /// Appartement mono-fil (shell : raccourcis, icônes, fenêtres).
    pub fn sta() -> windows::core::Result<ComGuard> {
        Self::init(COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
    }

    /// Multi-fil (audio).
    pub fn mta() -> windows::core::Result<ComGuard> {
        Self::init(COINIT_MULTITHREADED)
    }

    fn init(model: COINIT) -> windows::core::Result<ComGuard> {
        // SAFETY : initialisation COM du fil courant ; défaite par `Drop` sur ce même fil.
        let hr = unsafe { CoInitializeEx(None, model) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(ComGuard { owned: false });
        }
        // S_OK comme S_FALSE (déjà fait) demandent un CoUninitialize.
        hr.ok().map(|()| ComGuard { owned: true })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY : appel apparié au CoInitializeEx réussi de ce fil.
            unsafe { CoUninitialize() };
        }
    }
}
