//! Mise en veille et reprise de Windows (`PowerRegisterSuspendResumeNotification`) : rappel
//! système sans fenêtre, valable aussi pour la veille moderne.

use crossbeam_channel::Sender;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Suspend,
    Resume,
}

unsafe extern "system" fn callback(
    context: *const core::ffi::c_void,
    kind: u32,
    _setting: *const core::ffi::c_void,
) -> u32 {
    // SAFETY : `context` est le `Sender` fuité par `watch`, valable jusqu'à la fin du processus.
    let tx = unsafe { &*(context as *const Sender<PowerEvent>) };
    let ev = match kind {
        PBT_APMSUSPEND => Some(PowerEvent::Suspend),
        PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => Some(PowerEvent::Resume),
        _ => None,
    };
    if let Some(ev) = ev {
        let _ = tx.send(ev);
    }
    ERROR_SUCCESS.0
}

/// Envoie sur `tx` les mises en veille et les reprises, pour toute la durée du processus.
pub fn watch(tx: Sender<PowerEvent>) -> anyhow::Result<()> {
    // Les paramètres et le canal doivent survivre à l'abonnement (jamais résilié) : fuités.
    let context: &'static Sender<PowerEvent> = Box::leak(Box::new(tx));
    let params: &'static mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS =
        Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(callback),
            Context: context as *const Sender<PowerEvent> as *mut core::ffi::c_void,
        }));
    let mut handle = std::ptr::null_mut();
    // SAFETY : `params` vit jusqu'à la fin du processus ; `handle` reçoit l'abonnement.
    let err = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut core::ffi::c_void),
            &mut handle,
        )
    };
    anyhow::ensure!(err == ERROR_SUCCESS, "abonnement aux reprises de veille refusé ({})", err.0);
    Ok(())
}
