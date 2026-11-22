//! Lecture en cours de Windows (SMTC) : un fil dédié écoute les sessions, relit l'état à chaque
//! changement (et, pendant la lecture, l'état de lecture et la position chaque seconde, sans
//! redemander les titres) et exécute les commandes. La pochette est relue quand elle a pu
//! changer (`horizon_core::media::ArtTracker`).

use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::media::{ArtTracker, MediaInfo, merge, seek_target, timeline_ms};
use std::time::{Duration, Instant};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows_future::{AsyncStatus, IAsyncOperation};

/// Une appli figée ne doit pas bloquer le fil.
const ASYNC_TIMEOUT: Duration = Duration::from_millis(800);
/// Pochette plus lourde ignorée.
const MAX_ART: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
    /// Avance (positif) ou recule (négatif), en millisecondes.
    SeekRelative(i64),
    /// Suivre cette session (identifiant d'appli) plutôt que celle choisie par Windows.
    Select(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaSnapshot {
    pub info: Option<MediaInfo>,
    /// Sessions ouvertes : (identifiant d'appli, titre en cours).
    pub sessions: Vec<(String, String)>,
    /// `Some` quand la pochette a changé (`Some(None)` = plus de pochette).
    pub art: Option<Option<Vec<u8>>>,
}

enum Msg {
    /// État de lecture ou position d'une session.
    Changed,
    /// Titre, artiste ou miniature d'une session.
    PropertiesChanged,
    SessionsChanged,
    Command(MediaCommand),
}

pub struct MediaHandle {
    tx: Sender<Msg>,
}

impl MediaHandle {
    pub fn send(&self, c: MediaCommand) {
        let _ = self.tx.send(Msg::Command(c));
    }
}

fn wait<T: windows::core::RuntimeType + 'static>(op: &IAsyncOperation<T>) -> anyhow::Result<T> {
    let start = Instant::now();
    loop {
        match op.Status()? {
            AsyncStatus::Completed => return Ok(op.GetResults()?),
            AsyncStatus::Started if start.elapsed() < ASYNC_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(2))
            }
            AsyncStatus::Started => {
                let _ = op.Cancel();
                anyhow::bail!("délai dépassé");
            }
            s => anyhow::bail!("opération terminée avec l'état {}", s.0),
        }
    }
}

/// État d'une session. `known` : titre et artiste déjà lus pour cette appli (relecture
/// périodique) ; sinon ils sont redemandés à l'appli (appel asynchrone, plus coûteux).
fn read_info(session: &Session, known: Option<&MediaInfo>) -> anyhow::Result<MediaInfo> {
    let app_id = session.SourceAppUserModelId()?.to_string();
    let playing = session.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
    let (title, artist) = match known.filter(|k| k.app_id == app_id) {
        Some(k) => (k.title.clone(), k.artist.clone()),
        None => {
            let props = wait(&session.TryGetMediaPropertiesAsync()?)?;
            (props.Title()?.to_string(), props.Artist()?.to_string())
        }
    };
    let t = session.GetTimelineProperties()?;
    let start = t.StartTime()?.Duration;
    let ms = |d: i64| timeline_ms(d, start);
    Ok(MediaInfo {
        app_id,
        title,
        artist,
        playing,
        position_ms: ms(t.Position()?.Duration),
        duration_ms: ms(t.EndTime()?.Duration),
        timeline_at_ms: horizon_core::time::from_winrt(t.LastUpdatedTime()?.UniversalTime),
    })
}

fn read_art(session: &Session) -> Option<Vec<u8>> {
    let props = wait(&session.TryGetMediaPropertiesAsync().ok()?).ok()?;
    let stream = wait(&props.Thumbnail().ok()?.OpenReadAsync().ok()?).ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_ART {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    wait(&reader.LoadAsync(size as u32).ok()?).ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

struct Watcher {
    manager: Manager,
    tx: Sender<Msg>,
    subscriptions: Vec<(Session, i64, i64, i64)>,
    selected: Option<String>,
    last: Option<MediaInfo>,
    art: ArtTracker,
    /// Sessions ouvertes et leur titre, relues seulement sur un événement.
    sessions: Vec<(String, String)>,
}

/// Rappel d'un événement WinRT : envoie `msg()` au fil média.
fn notifier<S, A>(tx: Sender<Msg>, msg: fn() -> Msg) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(msg());
        Ok(())
    })
}

impl Watcher {
    fn resubscribe(&mut self) {
        for (s, a, b, c) in self.subscriptions.drain(..) {
            let _ = s.RemovePlaybackInfoChanged(a);
            let _ = s.RemoveMediaPropertiesChanged(b);
            let _ = s.RemoveTimelinePropertiesChanged(c);
        }
        let Ok(sessions) = self.manager.GetSessions() else { return };
        for s in sessions {
            let sub = (|| -> windows::core::Result<_> {
                let a = s.PlaybackInfoChanged(&notifier(self.tx.clone(), || Msg::Changed))?;
                let b = s.MediaPropertiesChanged(&notifier(self.tx.clone(), || Msg::PropertiesChanged))?;
                let c = s.TimelinePropertiesChanged(&notifier(self.tx.clone(), || Msg::Changed))?;
                Ok((s.clone(), a, b, c))
            })();
            match sub {
                Ok(x) => self.subscriptions.push(x),
                Err(e) => log::warn!("abonnement à une session média : {e}"),
            }
        }
    }

    /// Session suivie : celle choisie par l'utilisateur si elle existe encore, sinon celle de Windows.
    fn current(&self) -> Option<Session> {
        if let Some(id) = &self.selected
            && let Some((s, ..)) = self.subscriptions.iter().find(|(s, ..)| {
                s.SourceAppUserModelId().map(|a| a.to_string()).ok().as_deref() == Some(id)
            })
        {
            return Some(s.clone());
        }
        self.manager.GetCurrentSession().ok()
    }

    /// État courant. `changed` : un événement est arrivé (morceau, sessions, commande) et tout
    /// est relu ; sinon (relecture périodique pendant la lecture) seuls l'état de lecture et la
    /// position le sont, titre et sessions sont repris de la dernière lecture. `announced` : une
    /// appli a annoncé de nouvelles propriétés (la miniature a pu changer).
    fn snapshot(&mut self, changed: bool, announced: bool) -> MediaSnapshot {
        let session = self.current();
        let known = if changed { None } else { self.last.as_ref() };
        let now = horizon_core::time::now_ms();
        let info = session.as_ref().and_then(|s| read_info(s, known).ok()).map(|i| merge(self.last.as_ref(), i, now));
        let track = info.as_ref().map(|i| (i.app_id.clone(), i.title.clone()));
        let art = self.art.update(track, announced, now, || session.as_ref().and_then(read_art));
        if changed {
            self.sessions = self
                .subscriptions
                .iter()
                .filter_map(|(s, ..)| {
                    let id = s.SourceAppUserModelId().ok()?.to_string();
                    let title = wait(&s.TryGetMediaPropertiesAsync().ok()?)
                        .ok()
                        .and_then(|p| p.Title().ok())
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    Some((id, title))
                })
                .collect();
        }
        self.last = info.clone();
        MediaSnapshot { info, sessions: self.sessions.clone(), art }
    }

    /// Traite un message ; rend `true` si une appli a annoncé de nouvelles propriétés.
    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Changed => {}
            Msg::PropertiesChanged => return true,
            Msg::SessionsChanged => self.resubscribe(),
            Msg::Command(c) => {
                if let Err(e) = self.run_command(c) {
                    log::warn!("commande média : {e:#}");
                }
            }
        }
        false
    }

    fn run_command(&mut self, c: MediaCommand) -> anyhow::Result<()> {
        if let MediaCommand::Select(id) = c {
            self.selected = Some(id);
            return Ok(());
        }
        let Some(s) = self.current() else { return Ok(()) };
        let done = match c {
            MediaCommand::PlayPause => wait(&s.TryTogglePlayPauseAsync()?)?,
            MediaCommand::Next => wait(&s.TrySkipNextAsync()?)?,
            MediaCommand::Previous => wait(&s.TrySkipPreviousAsync()?)?,
            MediaCommand::SeekRelative(delta) => {
                let info = read_info(&s, None)?;
                let start = s.GetTimelineProperties()?.StartTime()?.Duration;
                let target = seek_target(&info, horizon_core::time::now_ms(), delta, start);
                wait(&s.TryChangePlaybackPositionAsync(target)?)?
            }
            MediaCommand::Select(_) => true,
        };
        if !done {
            log::debug!("commande média refusée par l'appli");
        }
        Ok(())
    }
}

/// Suit la lecture en cours ; `Ok` quand plus personne n'envoie de commande.
fn run(tx: &Sender<Msg>, rx: &Receiver<Msg>, on_update: &mut impl FnMut(MediaSnapshot)) -> anyhow::Result<()> {
    // SAFETY : initialisation WinRT de ce fil, une fois.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
    let manager = wait(&Manager::RequestAsync()?)?;
    manager.CurrentSessionChanged(&notifier(tx.clone(), || Msg::SessionsChanged))?;
    manager.SessionsChanged(&notifier(tx.clone(), || Msg::SessionsChanged))?;
    let mut w = Watcher {
        manager,
        tx: tx.clone(),
        subscriptions: Vec::new(),
        selected: None,
        last: None,
        art: ArtTracker::default(),
        sessions: Vec::new(),
    };
    w.resubscribe();
    on_update(w.snapshot(true, true));
    loop {
        let tick = Duration::from_secs(1);
        let msg = match rx.recv_timeout(tick) {
            Ok(m) => Some(m),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
            Err(_) => return Ok(()),
        };
        // Rien de nouveau : on ne relit que pendant la lecture (position qui avance).
        if msg.is_none() && !w.last.as_ref().is_some_and(|i| i.playing) {
            continue;
        }
        let changed = msg.is_some();
        let mut announced = msg.is_some_and(|m| w.handle(m));
        // Les événements arrivent en rafale : on laisse passer les suivants.
        while let Ok(m) = rx.recv_timeout(Duration::from_millis(40)) {
            announced |= w.handle(m);
        }
        on_update(w.snapshot(changed, announced));
    }
}

/// Démarre la surveillance ; `on_update` est appelé sur le fil média. Une erreur (Windows qui
/// tarde à répondre, service redémarré) vide l'état affiché et relance la surveillance.
pub fn start(on_update: impl FnMut(MediaSnapshot) + Send + 'static) -> anyhow::Result<MediaHandle> {
    let (tx, rx) = unbounded();
    let thread_tx = tx.clone();
    let mut on_update = on_update;
    std::thread::Builder::new().name("media".into()).spawn(move || {
        let on_update = std::cell::RefCell::new(&mut on_update);
        crate::restart::with_restart(
            "lecture en cours",
            || run(&thread_tx, &rx, &mut *on_update.borrow_mut()),
            || {
                // Les commandes en attente datent d'avant la panne : elles sont abandonnées.
                while rx.try_recv().is_ok() {}
                (on_update.borrow_mut())(MediaSnapshot { info: None, sessions: Vec::new(), art: Some(None) });
                true
            },
        );
    })?;
    Ok(MediaHandle { tx })
}
