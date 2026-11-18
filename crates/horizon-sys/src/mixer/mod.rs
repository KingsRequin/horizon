//! Panneau Audio : périphériques actifs et défauts, volume général, sessions audio par appli
//! (repris des plugins OpenDeck audio-switcher et app-volume).
//!
//! Un fil dédié (COM multi-fil) possède tous les objets audio. Il ne relit l'état que s'il est
//! « surveillé » (un panneau qui l'affiche est ouvert) : quand Windows signale un changement
//! (`watch`), juste après chaque commande, et par sécurité toutes les `SAFETY_POLL` ; panneau
//! fermé, les rappels sont retirés et il dort sur sa file de commandes.

mod index;
mod policy;
mod watch;

use crossbeam_channel::{Receiver, Sender, bounded, select, unbounded};
use horizon_core::mixer::{    AppAudio, Device, Flow, Level, MixerSnapshot, sort_apps,
};
use horizon_core::apps::{display_name, exe_key, packaged_name};
use index::AppIndex;
use std::collections::HashMap;
use std::time::Duration;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, AudioSessionStateExpired, DEVICE_STATE_ACTIVE, EDataFlow, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eCapture,
    eConsole, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::core::{GUID, Interface};

/// Identifiant d'instance de l'appareil qui porte un endpoint audio
/// (`USB\VID_046D&PID_0AF7&MI_00\…` pour un casque USB).
const PKEY_DEVICE_INSTANCE: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xb3f8fa53_0004_438e_9003_51a46e139bfc), pid: 2 };

/// Relecture de sécurité pendant qu'un panneau est ouvert (changement non signalé).
const SAFETY_POLL: Duration = Duration::from_secs(10);

/// Attente après un signal de Windows, pour relire une seule fois après une rafale (curseur de
/// volume d'une autre appli, sortie qui change : plusieurs rappels).
const SETTLE: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, PartialEq)]
pub enum MixerCommand {
    /// Ce périphérique devient le défaut (console, multimédia, communications).
    SetDefault(String),
    SetMasterVolume(f32),
    SetMasterMute(bool),
    SetAppVolume { key: String, volume: f32 },
    SetAppMute { key: String, muted: bool },
    /// Un panneau qui affiche l'état s'ouvre (`true`) ou se ferme.
    Watch(bool),
    /// Une relecture, même sans panneau ouvert (palette de commandes).
    Refresh,
}

pub struct MixerHandle {
    tx: Sender<MixerCommand>,
}

impl MixerHandle {
    pub fn send(&self, c: MixerCommand) {
        let _ = self.tx.send(c);
    }
}

/// Identifiant d'un périphérique (chaîne allouée par COM, libérée ici).
fn device_id(dev: &IMMDevice) -> Option<String> {
    // SAFETY : la chaîne renvoyée par GetId est libérée par CoTaskMemFree après copie.
    unsafe {
        let pw = dev.GetId().ok()?;
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const _));
        s
    }
}

/// Nom convivial d'un périphérique (« Haut-parleurs (Realtek…) »), vide s'il est illisible.
pub(crate) fn friendly_name(dev: &IMMDevice) -> String {
    // SAFETY : appareil COM valide ; la valeur de propriété est convertie en chaîne.
    unsafe {
        dev.OpenPropertyStore(STGM_READ)
            .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName))
            .map(|v| v.to_string())
            .unwrap_or_default()
    }
}

/// Sorties audio actives portées par l'appareil USB `vid:pid` (la sortie d'un casque et son
/// récepteur HID partagent le même appareil). Utilisable depuis n'importe quel fil.
pub fn outputs_of_usb(vid: u16, pid: u16) -> Vec<String> {
    let Ok(_com) = crate::com::ComGuard::mta() else { return Vec::new() };
    // SAFETY : objets COM utilisés sur ce fil, COM initialisé ; indices bornés par GetCount.
    unsafe {
        let Ok(e) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else {
            return Vec::new();
        };
        let Ok(coll) = e.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        (0..coll.GetCount().unwrap_or(0))
            .filter_map(|i| coll.Item(i).ok())
            .filter(|dev| {
                let instance = dev.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(&PKEY_DEVICE_INSTANCE));
                instance.is_ok_and(|v| horizon_core::mixer::usb_ids(&v.to_string()) == Some((vid, pid)))
            })
            .filter_map(|dev| device_id(&dev))
            .collect()
    }
}

/// Exe et AUMID (appli empaquetée) d'un processus.
#[derive(Clone)]
struct Proc {
    exe: String,
    aumid: Option<String>,
}

/// Possède les objets audio : ne quitte jamais le fil où COM a été initialisé.
struct Engine {
    enumerator: IMMDeviceEnumerator,
    index: Option<AppIndex>,
    /// Processus déjà résolus (vidé des processus disparus à chaque lecture).
    procs: HashMap<u32, Option<Proc>>,
    /// Rappels de Windows, tant qu'un panneau est ouvert.
    watch: Option<watch::Watchers>,
}

impl Engine {
    /// COM doit être initialisé sur ce fil.
    fn new() -> anyhow::Result<Engine> {
        // SAFETY : création d'un objet COM sur un fil initialisé.
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        Ok(Engine { enumerator, index: None, procs: HashMap::new(), watch: None })
    }

    /// Rappels de Windows enregistrés (`Some`) ou retirés : la sortie et les sessions suivies
    /// le sont à la relecture suivante.
    fn set_watch(&mut self, tx: Option<&Sender<()>>) {
        match tx {
            Some(tx) if self.watch.is_none() => match watch::Watchers::new(&self.enumerator, tx) {
                Ok(w) => self.watch = Some(w),
                Err(e) => log::warn!("suivi des périphériques audio : {e}"),
            },
            Some(_) => {}
            None => self.watch = None,
        }
    }

    fn devices(&self, flow: Flow) -> Vec<Device> {
        let edf: EDataFlow = if flow == Flow::Output { eRender } else { eCapture };
        // SAFETY : énumérateur valide ; indices bornés par GetCount.
        let Ok(coll) = (unsafe { self.enumerator.EnumAudioEndpoints(edf, DEVICE_STATE_ACTIVE) }) else {
            return Vec::new();
        };
        let n = unsafe { coll.GetCount() }.unwrap_or(0);
        (0..n)
            .filter_map(|i| unsafe { coll.Item(i) }.ok())
            .filter_map(|dev| {
                let id = device_id(&dev)?;
                let name = Some(friendly_name(&dev)).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
                Some(Device { id, name, flow, pinned: false })
            })
            .collect()
    }

    fn default_device(&self, edf: EDataFlow) -> Option<IMMDevice> {
        // SAFETY : énumérateur valide.
        unsafe { self.enumerator.GetDefaultAudioEndpoint(edf, eConsole) }.ok()
    }

    fn master_endpoint(&self) -> Option<IAudioEndpointVolume> {
        // SAFETY : appareil valide.
        self.default_device(eRender).and_then(|d| unsafe { d.Activate(CLSCTX_ALL, None) }.ok())
    }

    fn master(&mut self) -> Option<Level> {
        let dev = self.default_device(eRender)?;
        // SAFETY : appareil valide.
        let v: IAudioEndpointVolume = unsafe { dev.Activate(CLSCTX_ALL, None) }.ok()?;
        if let (Some(w), Some(id)) = (self.watch.as_mut(), device_id(&dev)) {
            w.follow_master(&id, &v);
        }
        // SAFETY : interface de volume valide.
        unsafe {
            Some(Level { volume: v.GetMasterVolumeLevelScalar().ok()?, muted: v.GetMute().ok()?.as_bool() })
        }
    }

    fn proc_info(&mut self, pid: u32) -> Option<Proc> {
        self.procs
            .entry(pid)
            .or_insert_with(|| {
                let exe = crate::foreground::exe_of_pid(pid)?;
                Some(Proc { exe, aumid: crate::foreground::process_aumid(pid) })
            })
            .clone()
    }

    /// Sessions de la sortie par défaut (hors sons système et sessions expirées), avec leur
    /// processus.
    fn sessions(&mut self) -> Vec<(IAudioSessionControl2, Proc)> {
        // SAFETY : objets COM valides le temps de la lecture ; indices bornés par GetCount.
        let list = unsafe {
            let Some(dev) = self.enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok() else {
                return Vec::new();
            };
            let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return Vec::new() };
            let Ok(list) = mgr.GetSessionEnumerator() else { return Vec::new() };
            if let (Some(w), Some(id)) = (self.watch.as_mut(), device_id(&dev)) {
                w.follow_manager(&id, &mgr);
            }
            list
        };
        let n = unsafe { list.GetCount() }.unwrap_or(0);
        let mut out = Vec::new();
        let mut seen = Vec::new();
        for i in 0..n {
            // SAFETY : session énumérée à l'instant.
            let Some(ctrl) = (unsafe { list.GetSession(i) }).ok().and_then(|c| c.cast::<IAudioSessionControl2>().ok())
            else {
                continue;
            };
            // IsSystemSoundsSession : S_OK pour les sons système, S_FALSE sinon.
            let (system, state, pid) = unsafe {
                (ctrl.IsSystemSoundsSession() == S_OK, ctrl.GetState().ok(), ctrl.GetProcessId().unwrap_or(0))
            };
            if system || pid == 0 || state == Some(AudioSessionStateExpired) {
                continue;
            }
            seen.push(pid);
            if let Some(p) = self.proc_info(pid) {
                out.push((ctrl, p));
            }
        }
        self.procs.retain(|pid, _| seen.contains(pid));
        if let Some(w) = self.watch.as_mut() {
            w.follow_sessions(out.iter().map(|(c, _)| c));
        }
        out
    }

    fn apps(&mut self) -> Vec<AppAudio> {
        let mut by_key: Vec<AppAudio> = Vec::new();
        for (ctrl, p) in self.sessions() {
            let key = exe_key(&p.exe);
            // SAFETY : session valide.
            let (level, active) = unsafe {
                let Ok(vol) = ctrl.cast::<ISimpleAudioVolume>() else { continue };
                let level = Level {
                    volume: vol.GetMasterVolume().unwrap_or(1.0),
                    muted: vol.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                };
                (level, ctrl.GetState().ok() == Some(AudioSessionStateActive))
            };
            if let Some(a) = by_key.iter_mut().find(|a| a.key == key) {
                a.active |= active;
                if let Some(m) = p.aumid.filter(|m| !a.aumids.contains(m)) {
                    a.aumids.push(m);
                }
                continue;
            }
            let idx = self.index.as_ref();
            let mut aumids: Vec<String> = idx.map(|i| i.aumids(&key).to_vec()).unwrap_or_default();
            if let Some(m) = p.aumid.as_ref().filter(|m| !aumids.contains(m)) {
                aumids.push(m.clone());
            }
            // Nom du raccourci du menu Démarrer, sinon du paquet, sinon de l'exe.
            let name = idx
                .and_then(|i| i.name(&key))
                .map(str::to_owned)
                .or_else(|| p.aumid.as_deref().and_then(packaged_name))
                .unwrap_or_else(|| display_name(&key, None));
            by_key.push(AppAudio { key, name, exe: p.exe, level, active, aumids });
        }
        sort_apps(&mut by_key);
        by_key
    }

    fn snapshot(&mut self) -> MixerSnapshot {
        let id_of = |e: EDataFlow| self.default_device(e).as_ref().and_then(device_id);
        MixerSnapshot {
            outputs: self.devices(Flow::Output),
            inputs: self.devices(Flow::Input),
            default_output: id_of(eRender),
            default_input: id_of(eCapture),
            master: self.master(),
            apps: self.apps(),
        }
    }

    fn for_each_session_of(&mut self, key: &str, f: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()>) {
        for (ctrl, p) in self.sessions() {
            if exe_key(&p.exe) == key
                && let Ok(v) = ctrl.cast::<ISimpleAudioVolume>()
                && let Err(e) = f(&v)
            {
                log::warn!("session audio de {key} : {e}");
            }
        }
    }

    fn apply(&mut self, c: MixerCommand) -> anyhow::Result<()> {
        match c {
            MixerCommand::SetDefault(id) => policy::set_default_all_roles(&id)?,
            MixerCommand::SetMasterVolume(v) => {
                let ep = self.master_endpoint().ok_or_else(|| anyhow::anyhow!("aucune sortie"))?;
                crate::volume::set_scalar(&ep, v)?;
            }
            MixerCommand::SetMasterMute(m) => {
                let ep = self.master_endpoint().ok_or_else(|| anyhow::anyhow!("aucune sortie"))?;
                crate::volume::set_mute(&ep, m)?;
            }
            MixerCommand::SetAppVolume { key, volume } => {
                let v = volume.clamp(0.0, 1.0);
                // SAFETY : interface de volume de session valide.
                self.for_each_session_of(&key, |s| unsafe { s.SetMasterVolume(v, &crate::volume::HORIZON_CONTEXT) });
            }
            MixerCommand::SetAppMute { key, muted } => {
                // SAFETY : interface de volume de session valide.
                self.for_each_session_of(&key, |s| unsafe { s.SetMute(muted, &crate::volume::HORIZON_CONTEXT) });
            }
            MixerCommand::Watch(_) | MixerCommand::Refresh => {}
        }
        Ok(())
    }
}

/// Applique les commandes et envoie l'état ; `Ok` quand le canal est fermé. `watching` (un
/// panneau est ouvert) survit à une relance.
fn run(rx: &Receiver<MixerCommand>, watching: &mut bool, on_update: &mut impl FnMut(MixerSnapshot)) -> anyhow::Result<()> {
    // COM du fil tenu jusqu'à la fin de cet essai (libéré après les objets audio).
    let _com = crate::com::ComGuard::mta()?;
    let mut e = Engine::new()?;
    // Signaux des rappels de Windows ; `changed_tx` gardé ici pour que le canal reste ouvert.
    let (changed_tx, changed) = bounded::<()>(1);
    if *watching {
        e.index = Some(index::build());
        e.set_watch(Some(&changed_tx));
        on_update(e.snapshot());
    }
    loop {
        let first = if *watching {
            select! {
                recv(rx) -> c => match c {
                    Ok(c) => Some(c),
                    Err(_) => return Ok(()),
                },
                recv(changed) -> _ => {
                    std::thread::sleep(SETTLE);
                    while changed.try_recv().is_ok() {}
                    None
                },
                default(SAFETY_POLL) => None,
            }
        } else {
            match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return Ok(()),
            }
        };
        let mut refresh = false;
        // Un curseur envoie des rafales : tout ce qui attend est appliqué avant de relire.
        for c in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            if let MixerCommand::Watch(on) = c {
                *watching = on;
                // Index construit à la première ouverture seulement (lecture du menu Démarrer).
                if on && e.index.is_none() {
                    e.index = Some(index::build());
                }
                e.set_watch(on.then_some(&changed_tx));
                if !on {
                    while changed.try_recv().is_ok() {}
                }
            } else if c == MixerCommand::Refresh {
                refresh = true;
            } else if let Err(err) = e.apply(c) {
                log::warn!("commande audio : {err:#}");
            }
        }
        if *watching || refresh {
            on_update(e.snapshot());
        }
    }
}

/// Démarre le fil audio ; `on_update` reçoit l'état (sur ce fil) à chaque relecture.
pub fn start(on_update: impl FnMut(MixerSnapshot) + Send + 'static) -> anyhow::Result<MixerHandle> {
    let (tx, rx) = unbounded();
    let mut on_update = on_update;
    std::thread::Builder::new().name("mixer".into()).spawn(move || {
        let mut watching = false;
        // Erreur (service audio redémarré…) : état vide envoyé, puis relance.
        let on_update = std::cell::RefCell::new(&mut on_update);
        crate::restart::with_restart(
            "panneau audio",
            || run(&rx, &mut watching, &mut *on_update.borrow_mut()),
            || {
                (on_update.borrow_mut())(MixerSnapshot::default());
                true
            },
        );
    })?;
    Ok(MixerHandle { tx })
}

/// Sorties actives et sortie par défaut, lues sur le fil appelant (sans sessions ni index des
/// applis : léger). Pour qui n'a pas le fil audio (bascule du casque d'Horizon natif).
pub fn outputs_once() -> anyhow::Result<(Vec<Device>, Option<String>)> {
    let _com = crate::com::ComGuard::mta()?;
    let e = Engine::new()?;
    let default = e.default_device(eRender).as_ref().and_then(device_id);
    Ok((e.devices(Flow::Output), default))
}

/// Ce périphérique devient le défaut (console, multimédia, communications), sur le fil appelant.
pub fn set_default_once(device_id: &str) -> anyhow::Result<()> {
    let _com = crate::com::ComGuard::mta()?;
    policy::set_default_all_roles(device_id)?;
    Ok(())
}

/// Lecture unique, sur le fil appelant (sonde) : index du menu Démarrer compris.
pub fn snapshot_once() -> anyhow::Result<MixerSnapshot> {
    let _com = crate::com::ComGuard::mta()?;
    let mut e = Engine::new()?;
    e.index = Some(index::build());
    Ok(e.snapshot())
}
