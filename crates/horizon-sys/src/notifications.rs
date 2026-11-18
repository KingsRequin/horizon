//! Lecture des notifications Windows des autres applis (`UserNotificationListener`).
//!
//! L'API exige une identité de paquet : Horizon est enregistré avec un paquet « sparse »
//! (`packaging/`). L'événement `NotificationChanged` n'est pas disponible pour une appli de
//! bureau : la liste est relue et comparée à la précédente. Chaque lecture fait analyser par
//! Windows (wpnapps, msxml) le XML de toutes les notifications présentes : c'était le premier
//! poste de processeur d'Horizon au repos. Elle est donc espacée quand l'utilisateur ne touche
//! plus à rien (`horizon_core::notifs::poll_period_ms`) et suspendue pendant une absence
//! (session verrouillée, écran éteint).

use crossbeam_channel::Sender;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};

/// Nouvel essai quand l'accès est refusé.
const DENIED_RETRY: Duration = Duration::from_secs(10);
/// Avec les avis de changement : relecture de sécurité (avis perdu).
const EVENT_SAFETY: Duration = Duration::from_secs(30);

fn poll_period() -> Duration {
    Duration::from_millis(horizon_core::notifs::poll_period_ms(crate::win32::input_idle_ms()))
}

pub use horizon_core::notifs::{NotifEvent, Notification};

/// Fil de lecture ; s'arrête quand la valeur est abandonnée.
pub struct NotifWatcher {
    stop: Arc<AtomicBool>,
}

impl Drop for NotifWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn convert(n: &UserNotification) -> windows::core::Result<Notification> {
    let info = n.AppInfo()?;
    let display = info.DisplayInfo()?;
    let binding = n
        .Notification()?
        .Visual()?
        .GetBinding(&KnownNotificationBindings::ToastGeneric()?)?;
    let texts: Vec<String> = binding
        .GetTextElements()?
        .into_iter()
        .filter_map(|t| t.Text().ok().map(|s| s.to_string()))
        .collect();
    Ok(Notification {
        id: n.Id()?,
        app_name: display.DisplayName()?.to_string(),
        app_id: info.AppUserModelId()?.to_string(),
        title: texts.first().cloned().unwrap_or_default(),
        body: texts.iter().skip(1).cloned().collect::<Vec<_>>().join("\n"),
        at_ms: horizon_core::time::from_winrt(n.CreationTime()?.UniversalTime),
    })
}

fn access(listener: &UserNotificationListener) -> windows::core::Result<bool> {
    let mut status = listener.GetAccessStatus()?;
    if status == UserNotificationListenerAccessStatus::Unspecified {
        status = listener.RequestAccessAsync()?.join()?;
    }
    Ok(status == UserNotificationListenerAccessStatus::Allowed)
}

fn run(sink: &Sender<NotifEvent>, stop: &AtomicBool) -> windows::core::Result<()> {
    let listener = UserNotificationListener::Current()?;
    // Avis de changement : s'ils sont disponibles, la liste n'est relue qu'à chaque avis (et
    // toutes les EVENT_SAFETY par sécurité) au lieu de toutes les quelques secondes.
    let (changed_tx, changed) = crossbeam_channel::unbounded::<()>();
    let handler = TypedEventHandler::new(move |_, _| {
        let _ = changed_tx.send(());
        Ok(())
    });
    let token = match listener.NotificationChanged(&handler) {
        Ok(t) => {
            log::info!("notifications : avis de changement disponibles");
            Some(t)
        }
        Err(e) => {
            log::info!("notifications : avis de changement indisponibles ({e}) : relecture périodique");
            None
        }
    };
    let result = watch(&listener, sink, stop, token.is_some().then_some(&changed));
    if let Some(t) = token {
        let _ = listener.RemoveNotificationChanged(t);
    }
    result
}

fn watch(
    listener: &UserNotificationListener,
    sink: &Sender<NotifEvent>,
    stop: &AtomicBool,
    changed: Option<&crossbeam_channel::Receiver<()>>,
) -> windows::core::Result<()> {
    let mut denied_sent = false;
    let mut first_notice = true;
    let mut known: HashSet<u32> = HashSet::new();
    let mut first = true;
    while !stop.load(Ordering::Relaxed) {
        crate::presence::wait_while_away();
        if !access(listener)? {
            if !denied_sent {
                let _ = sink.send(NotifEvent::AccessDenied);
                denied_sent = true;
            }
            std::thread::sleep(DENIED_RETRY);
            continue;
        }
        denied_sent = false;
        let list = listener.GetNotificationsAsync(NotificationKinds::Toast)?.join()?;
        let mut now: HashSet<u32> = HashSet::new();
        for n in list {
            let Ok(id) = n.Id() else { continue };
            now.insert(id);
            // Au premier passage, les notifications déjà présentes sont aussi transmises.
            if (first || !known.contains(&id))
                && let Ok(c) = convert(&n)
            {
                let _ = sink.send(NotifEvent::Added(c));
            }
        }
        for gone in known.difference(&now) {
            let _ = sink.send(NotifEvent::Removed(*gone));
        }
        known = now;
        first = false;
        match changed {
            Some(rx) => {
                if rx.recv_timeout(EVENT_SAFETY).is_ok() {
                    if first_notice {
                        log::info!("notifications : premier avis de changement reçu");
                        first_notice = false;
                    }
                    // Avis en rafale (ajout puis retrait) : une seule relecture.
                    std::thread::sleep(Duration::from_millis(100));
                    while rx.try_recv().is_ok() {}
                }
            }
            None => std::thread::sleep(poll_period()),
        }
    }
    Ok(())
}

/// Retire une notification du centre de notifications de Windows (« Écarter »), sur un fil
/// WinRT à part ; l'avis de changement (ou la relecture suivante) met la liste à jour.
pub fn remove(id: u32) {
    let spawned = std::thread::Builder::new().name("notification-retrait".into()).spawn(move || {
        // SAFETY : initialisation WinRT du fil courant, une fois.
        if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            log::warn!("notifications : retrait impossible (WinRT) : {e}");
            return;
        }
        if let Err(e) = UserNotificationListener::Current().and_then(|l| l.RemoveNotification(id)) {
            log::warn!("notifications : retrait de {id} refusé : {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("notifications : retrait impossible : {e}");
    }
}

/// Démarre la lecture sur un fil dédié.
pub fn start(sink: Sender<NotifEvent>) -> anyhow::Result<NotifWatcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            // SAFETY : initialisation WinRT du fil courant, une fois.
            if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                log::error!("WinRT indisponible, notifications désactivées : {e}");
                let _ = sink.send(NotifEvent::Unavailable);
                return;
            }
            // Une erreur Windows (identité de paquet absente, service redémarré…) n'arrête pas
            // la lecture : on signale « indisponible » et on reprend avec un délai croissant.
            crate::restart::with_restart(
                "lecture des notifications",
                || Ok(run(&sink, &s)?),
                || !s.load(Ordering::Relaxed) && sink.send(NotifEvent::Unavailable).is_ok(),
            );
        })?;
    Ok(NotifWatcher { stop })
}
