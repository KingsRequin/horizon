//! Lecture en cours de Windows (SMTC) : un fil dédié écoute les sessions, relit l'état à chaque
//! changement (et, pendant la lecture, l'état de lecture et la position chaque seconde, sans
//! redemander titres ni pochette) et exécute les commandes.

use horizon_core::media::{seek_target, timeline_ms};
use crossbeam_channel::{Receiver, Sender, unbounded};
use horizon_core::media::{MediaInfo, merge};
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
    Changed,
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
    last_art_key: Option<(String, String)>,
    /// Pochette trouvée pour le morceau courant ; sinon on la redemande à chaque événement, et
    /// aux relectures périodiques pendant `ART_RETRY` après le changement de morceau (beaucoup
    /// d'applis publient le titre avant la miniature).
    last_art_found: bool,
    art_retry_until: Instant,
    /// Sessions ouvertes et leur titre, relues seulement sur un événement.
    sessions: Vec<(String, String)>,
}

/// Durée pendant laquelle une pochette absente est redemandée à chaque relecture.
const ART_RETRY: Duration = Duration::from_secs(10);

fn notifier<S, A>(tx: Sender<Msg>, sessions: bool) -> TypedEventHandler<S, A>
where
    S: windows::core::RuntimeType + 'static,
    A: windows::core::RuntimeType + 'static,
{
    TypedEventHandler::new(move |_, _| {
        let _ = tx.send(if sessions { Msg::SessionsChanged } else { Msg::Changed });
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
                let a = s.PlaybackInfoChanged(&notifier(self.tx.clone(), false))?;
                let b = s.MediaPropertiesChanged(&notifier(self.tx.clone(), false))?;
                let c = s.TimelinePropertiesChanged(&notifier(self.tx.clone(), false))?;
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
    /// position le sont, titre, sessions et pochette sont repris de la dernière lecture.
    fn snapshot(&mut self, changed: bool) -> MediaSnapshot {
        let session = self.current();
        let known = if changed { None } else { self.last.as_ref() };
        let info = session
            .as_ref()
            .and_then(|s| read_info(s, known).ok())
            .map(|i| merge(self.last.as_ref(), i, horizon_core::time::now_ms()));
        let key = info.as_ref().map(|i| (i.app_id.clone(), i.title.clone()));
        let new_track = key != self.last_art_key;
        if new_track {
            self.art_retry_until = Instant::now() + ART_RETRY;
        }
        let retry = !self.last_art_found && (changed || Instant::now() < self.art_retry_until);
        let art = if new_track || retry {
            let a = session.as_ref().and_then(read_art);
            let changed = new_track || a.is_some();
            self.last_art_key = key;
            self.last_art_found = a.is_some();
            changed.then_some(a)
        } else {
            None
        };
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
    manager.CurrentSessionChanged(&notifier(tx.clone(), true))?;
    manager.SessionsChanged(&notifier(tx.clone(), true))?;
    let mut w = Watcher {
        manager,
        tx: tx.clone(),
        subscriptions: Vec::new(),
        selected: None,
        last: None,
        last_art_key: None,
        last_art_found: false,
        art_retry_until: Instant::now(),
        sessions: Vec::new(),
    };
    w.resubscribe();
    on_update(w.snapshot(true));
    loop {
        let tick = Duration::from_secs(1);
        let msg = match rx.recv_timeout(tick) {
            Ok(m) => Some(m),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
            Err(_) => return Ok(()),
        };
        let changed = msg.is_some();
        match msg {
            Some(Msg::SessionsChanged) => w.resubscribe(),
            Some(Msg::Command(c)) => {
                if let Err(e) = w.run_command(c) {
                    log::warn!("commande média : {e:#}");
                }
            }
            Some(Msg::Changed) => {}
            // Rien de nouveau : on ne relit que pendant la lecture (position qui avance).
            None if !w.last.as_ref().is_some_and(|i| i.playing) => continue,
            None => {}
        }
        // Les événements arrivent en rafale : on laisse passer les suivants.
        while let Ok(m) = rx.recv_timeout(Duration::from_millis(40)) {
            match m {
                Msg::SessionsChanged => w.resubscribe(),
                Msg::Command(c) => {
                    let _ = w.run_command(c);
                }
                Msg::Changed => {}
            }
        }
        on_update(w.snapshot(changed));
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
