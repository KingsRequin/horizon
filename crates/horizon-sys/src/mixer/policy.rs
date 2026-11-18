#![allow(non_snake_case)]
//! `IPolicyConfig` : interface COM non documentée (stable de Vista à Windows 11) qu'utilisent
//! tous les outils de bascule audio (repris du plugin OpenDeck audio-switcher). Seul
//! `SetDefaultEndpoint` sert ; les autres méthodes ne sont déclarées que pour occuper les bons
//! emplacements de la table virtuelle : ne jamais les appeler.

use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

/// Classe COM `PolicyConfigClient`.
const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    // Emplacements 3 à 12 : signatures réduites (bon nombre d'arguments, types opaques).
    unsafe fn GetMixFormat(&self, a: PCWSTR, b: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(&self, a: PCWSTR, b: i32, c: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, a: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(&self, a: PCWSTR, b: *mut core::ffi::c_void, c: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetProcessingPeriod(&self, a: PCWSTR, b: i32, c: *mut i64, d: *mut i64) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, a: PCWSTR, b: *mut i64) -> HRESULT;
    unsafe fn GetShareMode(&self, a: PCWSTR, b: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetShareMode(&self, a: PCWSTR, b: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetPropertyValue(&self, a: PCWSTR, b: i32, c: *mut core::ffi::c_void, d: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetPropertyValue(&self, a: PCWSTR, b: i32, c: *mut core::ffi::c_void, d: *mut core::ffi::c_void) -> HRESULT;
    /// Emplacement 13 : périphérique par défaut pour un rôle.
    unsafe fn SetDefaultEndpoint(&self, device_id: PCWSTR, role: ERole) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, a: PCWSTR, b: i32) -> HRESULT;
}

/// `device_id` devient le périphérique par défaut pour les trois rôles (console, multimédia,
/// communications) : tout le son le suit. COM doit être initialisé sur le fil appelant.
pub(crate) fn set_default_all_roles(device_id: &str) -> windows::core::Result<()> {
    // SAFETY : objet COM créé ici ; chaîne UTF-16 terminée par zéro, vivante pendant les appels.
    unsafe {
        let pc: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
        let wide = crate::win32::to_wide(device_id);
        for role in [eConsole, eMultimedia, eCommunications] {
            pc.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role).ok()?;
        }
    }
    Ok(())
}
