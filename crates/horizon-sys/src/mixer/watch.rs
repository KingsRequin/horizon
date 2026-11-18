//! Rappels audio de Windows pendant qu'un panneau Audio est ouvert : périphériques (ajout,
//! retrait, défaut, nom), volume général, sessions créées, et volume ou état de chaque session.
//! Chaque rappel ne fait que signaler au fil audio qu'une relecture est utile (`Sender<()>` de
//! capacité 1 : une rafale ne laisse qu'un signal). Les changements faits par Horizon
//! (`HORIZON_CONTEXT`) ne sont pas signalés : la commande est déjà suivie d'une relecture.

use crate::volume::HORIZON_CONTEXT;
use crossbeam_channel::Sender;
use std::collections::HashMap;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, AudioSessionDisconnectReason, AudioSessionState, DEVICE_STATE, EDataFlow,
    ERole, IAudioSessionControl, IAudioSessionControl2, IAudioSessionEvents, IAudioSessionEvents_Impl,
    IAudioSessionManager2, IAudioSessionNotification, IAudioSessionNotification_Impl, IMMDeviceEnumerator,
    IMMNotificationClient, IMMNotificationClient_Impl,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::core::{BOOL, GUID, PCWSTR, Ref, implement};

/// Signale une relecture (sans attendre : un signal en attente suffit).
fn ping(tx: &Sender<()>) {
    let _ = tx.try_send(());
}

/// Changement venu d'ailleurs qu'Horizon.
fn foreign(context: *const GUID) -> bool {
    // SAFETY : GUID fourni par Windows, valide pendant l'appel (ou nul).
    let context = unsafe { context.as_ref() };
    context != Some(&HORIZON_CONTEXT)
}

#[implement(IMMNotificationClient)]
struct Devices(Sender<()>);

impl IMMNotificationClient_Impl for Devices_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, _: EDataFlow, _: ERole, _: &PCWSTR) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
    /// Certains pilotes changent des propriétés sans cesse pendant la lecture : seul le nom
    /// affiché compte.
    fn OnPropertyValueChanged(&self, _: &PCWSTR, key: &PROPERTYKEY) -> windows::core::Result<()> {
        if *key == PKEY_Device_FriendlyName {
            ping(&self.0);
        }
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct Master(Sender<()>);

impl IAudioEndpointVolumeCallback_Impl for Master_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // SAFETY : structure fournie par Windows, valide pendant l'appel.
        if unsafe { data.as_ref() }.is_some_and(|d| d.guidEventContext != HORIZON_CONTEXT) {
            ping(&self.0);
        }
        Ok(())
    }
}

#[implement(IAudioSessionNotification)]
struct Created(Sender<()>);

impl IAudioSessionNotification_Impl for Created_Impl {
    fn OnSessionCreated(&self, _: Ref<IAudioSessionControl>) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
}

#[implement(IAudioSessionEvents)]
struct Session(Sender<()>);

impl IAudioSessionEvents_Impl for Session_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, context: *const GUID) -> windows::core::Result<()> {
        if foreign(context) {
            ping(&self.0);
        }
        Ok(())
    }
    fn OnChannelVolumeChanged(&self, _: u32, _: *const f32, _: u32, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    /// Lecture qui commence ou s'arrête (appli « active »), session expirée.
    fn OnStateChanged(&self, _: AudioSessionState) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> windows::core::Result<()> {
        ping(&self.0);
        Ok(())
    }
}

/// Identifiant unique d'une session (chaîne allouée par COM, libérée ici).
fn session_id(s: &IAudioSessionControl2) -> Option<String> {
    // SAFETY : session valide ; la chaîne rendue est libérée par CoTaskMemFree après copie.
    unsafe {
        let p = s.GetSessionInstanceIdentifier().ok()?;
        let id = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        id
    }
}

/// Rappels enregistrés ; tous retirés quand la valeur est lâchée (sur le fil audio).
pub(super) struct Watchers {
    enumerator: IMMDeviceEnumerator,
    devices: IMMNotificationClient,
    master_cb: IAudioEndpointVolumeCallback,
    created_cb: IAudioSessionNotification,
    session_cb: IAudioSessionEvents,
    /// Volume général suivi, par identifiant de sortie.
    master: Option<(String, IAudioEndpointVolume)>,
    /// Sessions de la sortie suivie, par identifiant de sortie.
    manager: Option<(String, IAudioSessionManager2)>,
    /// Sessions suivies, par identifiant de session.
    sessions: HashMap<String, IAudioSessionControl2>,
}

impl Watchers {
    pub(super) fn new(enumerator: &IMMDeviceEnumerator, tx: &Sender<()>) -> windows::core::Result<Watchers> {
        let devices: IMMNotificationClient = Devices(tx.clone()).into();
        // SAFETY : énumérateur valide ; le rappel est retiré dans `Drop`.
        unsafe { enumerator.RegisterEndpointNotificationCallback(&devices)? };
        Ok(Watchers {
            enumerator: enumerator.clone(),
            devices,
            master_cb: Master(tx.clone()).into(),
            created_cb: Created(tx.clone()).into(),
            session_cb: Session(tx.clone()).into(),
            master: None,
            manager: None,
            sessions: HashMap::new(),
        })
    }

    /// Suit le volume général de la sortie `id` (rien à faire si c'est déjà elle).
    pub(super) fn follow_master(&mut self, id: &str, ep: &IAudioEndpointVolume) {
        if self.master.as_ref().is_some_and(|(i, _)| i == id) {
            return;
        }
        if let Some((_, old)) = self.master.take() {
            // SAFETY : rappel enregistré sur cette interface.
            let _ = unsafe { old.UnregisterControlChangeNotify(&self.master_cb) };
        }
        // SAFETY : interface valide ; le rappel est retiré au changement de sortie ou dans `Drop`.
        match unsafe { ep.RegisterControlChangeNotify(&self.master_cb) } {
            Ok(()) => self.master = Some((id.to_owned(), ep.clone())),
            Err(e) => log::warn!("suivi du volume général : {e}"),
        }
    }

    /// Suit les sessions créées sur la sortie `id`. Windows ne les signale qu'après une
    /// énumération des sessions (faite à chaque relecture).
    pub(super) fn follow_manager(&mut self, id: &str, mgr: &IAudioSessionManager2) {
        if self.manager.as_ref().is_some_and(|(i, _)| i == id) {
            return;
        }
        if let Some((_, old)) = self.manager.take() {
            // SAFETY : rappel enregistré sur ce gestionnaire.
            let _ = unsafe { old.UnregisterSessionNotification(&self.created_cb) };
        }
        // SAFETY : gestionnaire valide ; le rappel est retiré au changement de sortie ou dans `Drop`.
        match unsafe { mgr.RegisterSessionNotification(&self.created_cb) } {
            Ok(()) => self.manager = Some((id.to_owned(), mgr.clone())),
            Err(e) => log::warn!("suivi des sessions audio : {e}"),
        }
    }

    /// Suit le volume et l'état de ces sessions, et plus celui des autres (disparues).
    pub(super) fn follow_sessions<'a>(&mut self, now: impl IntoIterator<Item = &'a IAudioSessionControl2>) {
        let mut kept = HashMap::new();
        for s in now {
            let Some(id) = session_id(s) else { continue };
            if kept.contains_key(&id) {
                continue;
            }
            if let Some(known) = self.sessions.remove(&id) {
                kept.insert(id, known);
                continue;
            }
            // SAFETY : session valide ; le rappel est retiré quand elle disparaît ou dans `Drop`.
            match unsafe { s.RegisterAudioSessionNotification(&self.session_cb) } {
                Ok(()) => {
                    kept.insert(id, s.clone());
                }
                Err(e) => log::debug!("suivi d'une session audio : {e}"),
            }
        }
        for gone in std::mem::replace(&mut self.sessions, kept).into_values() {
            // SAFETY : rappel enregistré sur cette session.
            let _ = unsafe { gone.UnregisterAudioSessionNotification(&self.session_cb) };
        }
    }
}

impl Drop for Watchers {
    fn drop(&mut self) {
        self.follow_sessions([]);
        // SAFETY : rappels enregistrés sur ces objets, retirés une seule fois.
        unsafe {
            if let Some((_, mgr)) = self.manager.take() {
                let _ = mgr.UnregisterSessionNotification(&self.created_cb);
            }
            if let Some((_, ep)) = self.master.take() {
                let _ = ep.UnregisterControlChangeNotify(&self.master_cb);
            }
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.devices);
        }
    }
}
