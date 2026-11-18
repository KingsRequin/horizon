//! Micro par défaut de Windows : coupé ou ouvert, et son nom. Relu toutes les 500 ms (le
//! micro par défaut peut changer) : seuls l'identifiant du micro par défaut et son état coupé
//! sont lus ; le micro n'est rouvert (nom, volume) que s'il a changé.

use crossbeam_channel::Sender;
use serde::Serialize;
use std::time::Duration;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CoCreateInstance,
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MicState {
    /// `None` : aucun micro.
    pub muted: Option<bool>,
    pub device: String,
}

fn default_mic(e: &IMMDeviceEnumerator) -> Option<IMMDevice> {
    // SAFETY : énumérateur COM valide.
    unsafe { e.GetDefaultAudioEndpoint(eCapture, eConsole) }.ok()
}

/// Micro par défaut déjà ouvert : son identifiant, son nom et son volume (relus seulement
/// quand le micro par défaut change).
struct Opened {
    id: String,
    name: String,
    volume: Option<IAudioEndpointVolume>,
}

fn device_id(dev: &IMMDevice) -> Option<String> {
    // SAFETY : appareil COM valide ; chaîne libérée par CoTaskMemFree.
    unsafe {
        let p = dev.GetId().ok()?;
        let id = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        id
    }
}

fn read(e: &IMMDeviceEnumerator, opened: &mut Option<Opened>) -> MicState {
    let Some(dev) = default_mic(e) else {
        *opened = None;
        return unknown();
    };
    let id = device_id(&dev).unwrap_or_default();
    if opened.as_ref().is_none_or(|o| o.id != id || id.is_empty()) {
        // SAFETY : appareil COM valide.
        let volume = unsafe { dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }.ok();
        *opened = Some(Opened { id, name: crate::mixer::friendly_name(&dev), volume });
    }
    let Some(o) = opened.as_ref() else { return unknown() };
    // SAFETY : interface COM valide.
    let muted = o.volume.as_ref().and_then(|v| unsafe { v.GetMute() }.ok()).map(|b| b.as_bool());
    let state = MicState { muted, device: o.name.clone() };
    if muted.is_none() {
        // Micro débranché puis rebranché (même identifiant, interface périmée) : rouvert à la
        // prochaine lecture.
        *opened = None;
    }
    state
}

/// Coupe ou rouvre le micro par défaut.
pub fn set_muted(muted: bool) -> anyhow::Result<()> {
    let _com = crate::com::ComGuard::mta()?;
    // SAFETY : COM initialisé sur ce fil ; objets COM valides le temps de l'appel.
    unsafe {
        let e: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = default_mic(&e).ok_or_else(|| anyhow::anyhow!("aucun micro"))?;
        dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?
            .SetMute(muted, std::ptr::null())?;
    }
    Ok(())
}

/// Micro absent ou illisible.
fn unknown() -> MicState {
    MicState { muted: None, device: String::new() }
}

/// Envoie l'état puis chacun de ses changements ; `Ok` quand plus personne n'écoute.
fn run(sink: &Sender<MicState>) -> anyhow::Result<()> {
    // COM du fil tenu jusqu'à la fin (relance comprise : chaque essai l'initialise).
    let _com = crate::com::ComGuard::mta()?;
    // SAFETY : COM initialisé pour ce fil.
    let e: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let mut last: Option<MicState> = None;
    let mut opened: Option<Opened> = None;
    loop {
        // Session verrouillée ou écran éteint : personne ne voit le voyant.
        crate::presence::wait_while_away();
        let now = read(&e, &mut opened);
        if last.as_ref() != Some(&now) {
            if sink.send(now.clone()).is_err() {
                return Ok(());
            }
            last = Some(now);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Surveille le micro par défaut ; chaque changement d'état est envoyé sur `sink`. Si
/// l'énumérateur audio est indisponible, « aucun micro » est envoyé et la lecture reprend
/// plus tard.
pub fn watch(sink: Sender<MicState>) -> anyhow::Result<()> {
    std::thread::Builder::new().name("mic".into()).spawn(move || {
        crate::restart::with_restart("voyant micro", || run(&sink), || sink.send(unknown()).is_ok());
    })?;
    Ok(())
}
