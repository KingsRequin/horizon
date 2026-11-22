//! Sonde de la lecture en cours (lecture seule) : ce que chaque session SMTC publie vraiment
//! (titre, état, miniature : taille, format, couleur dominante), relevé brut deux fois par
//! seconde, à côté de ce que le fil média d'Horizon en retient (`horizon_sys::media`). Une ligne
//! par changement : sert à voir, navigateur par navigateur, quand la pochette arrive après le
//! titre et si Horizon la suit.
//!
//! `cargo run -p horizon-sys --example media_probe -- [secondes] [dossier des miniatures]`

use horizon_core::accent::{dominant_color, readable_accent};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
};
use windows::Storage::Streams::DataReader;
use windows_future::{AsyncStatus, IAsyncOperation};

fn wait<T: windows::core::RuntimeType + 'static>(op: &IAsyncOperation<T>) -> anyhow::Result<T> {
    let start = Instant::now();
    loop {
        match op.Status()? {
            AsyncStatus::Completed => return Ok(op.GetResults()?),
            AsyncStatus::Started if start.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(2)),
            s => anyhow::bail!("opération non terminée (état {})", s.0),
        }
    }
}

fn stamp() -> String {
    let ms = horizon_core::time::now_ms() % 86_400_000;
    format!("{:02}:{:02}:{:02}.{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Octets → « 512 o PNG #1a2b3c4d 64×64 dominante [r, g, b] → accent [r, g, b] ».
fn describe(bytes: &[u8], keep: Option<&PathBuf>) -> String {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    let hash = h.finish() as u32;
    let kind = match bytes {
        [0x89, b'P', b'N', b'G', ..] => "PNG",
        [0xFF, 0xD8, ..] => "JPEG",
        [b'R', b'I', b'F', b'F', ..] => "RIFF (WebP ?)",
        [b'B', b'M', ..] => "BMP",
        [b'G', b'I', b'F', ..] => "GIF",
        _ => "format inconnu",
    };
    if let Some(dir) = keep {
        let _ = std::fs::write(dir.join(format!("miniature-{hash:08x}.bin")), bytes);
    }
    let picture = match horizon_sys::art::decode_rgba(bytes) {
        Some((w, h, px)) => match dominant_color(&px, w, h) {
            Some(c) => format!("{w}×{h} dominante {c:?} → accent {:?}", readable_accent(c)),
            None => format!("{w}×{h} sans couleur dominante (grise)"),
        },
        None => "INDÉCODABLE par Horizon".into(),
    };
    format!("{} o {kind} #{hash:08x} {picture}", bytes.len())
}

/// Miniature d'une session, étape par étape (chaque échec est nommé).
fn raw_art(session: &Session, keep: Option<&PathBuf>) -> String {
    let read = || -> anyhow::Result<Option<Vec<u8>>> {
        let props = wait(&session.TryGetMediaPropertiesAsync()?)?;
        let Ok(thumb) = props.Thumbnail() else { return Ok(None) };
        let stream = wait(&thumb.OpenReadAsync().map_err(|e| anyhow::anyhow!("OpenReadAsync : {e}"))?)
            .map_err(|e| anyhow::anyhow!("ouverture : {e}"))?;
        let size = stream.Size()?;
        let kind = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
        if size == 0 {
            anyhow::bail!("flux vide (type « {kind} »)");
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        let got = wait(&reader.LoadAsync(size as u32)?)?;
        let mut buf = vec![0u8; got as usize];
        reader.ReadBytes(&mut buf)?;
        if u64::from(got) != size {
            anyhow::bail!("{got} octets lus sur {size} annoncés (type « {kind} »)");
        }
        Ok(Some(buf))
    };
    match read() {
        Ok(Some(b)) => describe(&b, keep),
        Ok(None) => "aucune miniature".into(),
        Err(e) => format!("ÉCHEC : {e:#}"),
    }
}

fn raw_line(session: &Session, current: Option<&str>, keep: Option<&PathBuf>) -> anyhow::Result<(String, String)> {
    let id = session.SourceAppUserModelId()?.to_string();
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?.0;
    let props = wait(&session.TryGetMediaPropertiesAsync()?)?;
    let line = format!(
        "{}état {status} « {} » de « {} » | {}",
        if current == Some(id.as_str()) { "COURANTE " } else { "" },
        props.Title()?,
        props.Artist()?,
        raw_art(session, keep)
    );
    Ok((id, line))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let keep = args.next().map(PathBuf::from);
    if let Some(dir) = &keep {
        std::fs::create_dir_all(dir)?;
    }

    // Ce qu'Horizon retient : le même fil que l'appli.
    let _media = horizon_sys::media::start(move |snap| {
        let info = snap.info.as_ref().map_or("aucune lecture".to_owned(), |i| {
            format!("[{}] « {} » {}", i.app_id, i.title, if i.playing { "en lecture" } else { "en pause" })
        });
        match &snap.art {
            Some(Some(bytes)) => println!("{} HORIZON pochette  {info} | {}", stamp(), describe(bytes, None)),
            Some(None) => println!("{} HORIZON pochette  {info} | aucune", stamp()),
            None => {}
        }
    })?;

    // Ce que les sessions publient : relevé brut.
    let manager = wait(&Manager::RequestAsync()?)?;
    let mut last: HashMap<String, String> = HashMap::new();
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        let current = manager.GetCurrentSession().ok().and_then(|s| s.SourceAppUserModelId().ok()).map(|s| s.to_string());
        let mut seen = Vec::new();
        for s in manager.GetSessions()? {
            let (id, line) = match raw_line(&s, current.as_deref(), keep.as_ref()) {
                Ok(x) => x,
                Err(e) => (s.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default(), format!("ÉCHEC : {e:#}")),
            };
            if last.get(&id) != Some(&line) {
                println!("{} brut    [{id}] {line}", stamp());
                last.insert(id.clone(), line);
            }
            seen.push(id);
        }
        last.retain(|id, _| {
            let open = seen.contains(id);
            if !open {
                println!("{} brut    [{id}] session fermée", stamp());
            }
            open
        });
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}
