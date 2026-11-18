//! Volume général (sortie par défaut) : réglage, et suivi de ses changements faits ailleurs
//! (touches de volume, autre appli) pour l'affichage du volume d'Horizon.

use std::time::Duration;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::core::{GUID, implement};

/// Marque des changements faits par Horizon (molette de la barre, panneau Audio) : le suivi
/// les ignore, la barre les montre déjà.
pub(crate) const HORIZON_CONTEXT: GUID = GUID::from_u128(0x6f1c_2d3e_8a47_4b5c_9e0d_7a21_c3b4_e5f6);

/// Délai entre deux vérifications de la sortie par défaut (changement de sortie).
const DEVICE_POLL: Duration = Duration::from_secs(2);

/// Volume de la sortie par défaut. COM doit être initialisé sur le fil appelant.
fn endpoint() -> anyhow::Result<IAudioEndpointVolume> {
    // SAFETY : COM initialisé par l'appelant ; objets COM valides le temps de l'appel.
    unsafe {
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        Ok(e.GetDefaultAudioEndpoint(eRender, eConsole)?.Activate(CLSCTX_ALL, None)?)
    }
}

/// Identifiant de la sortie par défaut (COM initialisé par l'appelant).
fn default_id() -> Option<String> {
    // SAFETY : COM initialisé ; la chaîne rendue par GetId est libérée ici.
    unsafe {
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let p = e.GetDefaultAudioEndpoint(eRender, eConsole).ok()?.GetId().ok()?;
        let id = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        id
    }
}

/// Règle le volume (0–1) ; monter le son le rétablit s'il était coupé (comme Windows).
pub(crate) fn set_scalar(v: &IAudioEndpointVolume, level: f32) -> windows::core::Result<()> {
    let level = level.clamp(0.0, 1.0);
    // SAFETY : interface de volume valide.
    unsafe {
        v.SetMasterVolumeLevelScalar(level, &HORIZON_CONTEXT)?;
        if level > 0.0 && v.GetMute()?.as_bool() {
            v.SetMute(false, &HORIZON_CONTEXT)?;
        }
    }
    Ok(())
}

/// Coupe ou rétablit le son (changement marqué comme venant d'Horizon).
pub(crate) fn set_mute(v: &IAudioEndpointVolume, muted: bool) -> windows::core::Result<()> {
    // SAFETY : interface de volume valide.
    unsafe { v.SetMute(muted, &HORIZON_CONTEXT) }
}

/// Change le volume général de `delta` points (sur 100) ; renvoie le nouveau volume.
pub fn step(delta: i32) -> anyhow::Result<u32> {
    let _com = crate::com::ComGuard::mta()?;
    let v = endpoint()?;
    // SAFETY : interface de volume valide.
    let now = unsafe { v.GetMasterVolumeLevelScalar()? };
    let next = (now + delta as f32 / 100.0).clamp(0.0, 1.0);
    set_scalar(&v, next)?;
    Ok((next * 100.0).round() as u32)
}

/// Volume général (0–100) et coupure, changés par autre chose qu'Horizon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeChange {
    pub level: u32,
    pub muted: bool,
}

#[implement(IAudioEndpointVolumeCallback)]
struct Callback(crossbeam_channel::Sender<VolumeChange>);

impl IAudioEndpointVolumeCallback_Impl for Callback_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY : structure fournie par Windows, valide pendant l'appel.
        let Some(d) = (unsafe { data.as_ref() }) else { return Ok(()) };
        if d.guidEventContext != HORIZON_CONTEXT {
            let level = (d.fMasterVolume * 100.0).round().clamp(0.0, 100.0) as u32;
            let _ = self.0.send(VolumeChange { level, muted: d.bMuted.as_bool() });
        }
        Ok(())
    }
}

/// Suit le volume de la sortie par défaut (celle qui change est reprise en moins de 2 s) ;
/// `on_change` est appelé sur un fil à lui.
pub fn watch(mut on_change: impl FnMut(VolumeChange) + Send + 'static) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded::<VolumeChange>();
    std::thread::Builder::new().name("volume-suivi".into()).spawn(move || {
        let Ok(_com) = crate::com::ComGuard::mta() else {
            return log::warn!("suivi du volume : COM indisponible");
        };
        let callback: IAudioEndpointVolumeCallback = Callback(tx).into();
        let mut current: Option<(String, IAudioEndpointVolume)> = None;
        loop {
            let id = default_id();
            if current.as_ref().map(|(i, _)| i) != id.as_ref() {
                if let Some((_, ep)) = current.take() {
                    // SAFETY : rappel enregistré sur cette interface.
                    let _ = unsafe { ep.UnregisterControlChangeNotify(&callback) };
                }
                if let Some(id) = id {
                    match endpoint() {
                        // SAFETY : interface valide ; le rappel vit aussi longtemps que le fil.
                        Ok(ep) => match unsafe { ep.RegisterControlChangeNotify(&callback) } {
                            Ok(()) => current = Some((id, ep)),
                            Err(e) => log::warn!("suivi du volume : {e}"),
                        },
                        Err(e) => log::warn!("suivi du volume : {e:#}"),
                    }
                }
            }
            std::thread::sleep(DEVICE_POLL);
        }
    })?;
    std::thread::Builder::new().name("volume-annonce".into()).spawn(move || {
        for change in rx {
            on_change(change);
        }
    })?;
    Ok(())
}
