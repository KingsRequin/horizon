//! Journal : `%APPDATA%\Horizon\horizon.log`, mis de côté en `horizon.old.log` dès qu'il dépasse
//! 1 Mo, au lancement comme en cours de route (Horizon tourne des semaines sans redémarrer).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Taille au-delà de laquelle le journal est mis de côté.
const MAX_LEN: u64 = 1024 * 1024;

pub fn path() -> PathBuf {
    crate::state::app_dir().join("horizon.log")
}

pub fn init() {
    use simplelog::{ConfigBuilder, LevelFilter, WriteLogger};
    let dir = crate::state::app_dir();
    let _ = std::fs::create_dir_all(&dir);
    let config = ConfigBuilder::new()
        .add_filter_allow_str("horizon")
        .set_time_offset_to_local()
        .unwrap_or_else(|c| c)
        .build();
    if let Ok(f) = Rotating::open(path(), dir.join("horizon.old.log")) {
        let _ = WriteLogger::init(LevelFilter::Info, config, f);
    }
}

/// Fichier en ajout qui se met de côté une fois `MAX_LEN` dépassé. Ajout plutôt que réécriture :
/// un second lancement (`--quit`) ne doit pas effacer le journal de l'instance en cours.
struct Rotating {
    path: PathBuf,
    old: PathBuf,
    file: File,
    len: u64,
}

impl Rotating {
    fn open(path: PathBuf, old: PathBuf) -> io::Result<Rotating> {
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LEN) {
            let _ = std::fs::rename(&path, &old);
        }
        let (file, len) = append(&path)?;
        Ok(Rotating { path, old, file, len })
    }

    /// Met le journal de côté et en recommence un ; s'il ne peut pas être renommé (ouvert
    /// ailleurs), on continue dans le même plutôt que de perdre des lignes.
    fn rotate(&mut self) {
        let _ = self.file.flush();
        if std::fs::rename(&self.path, &self.old).is_ok()
            && let Ok((file, len)) = append(&self.path)
        {
            self.file = file;
            self.len = len;
        } else {
            self.len = 0;
        }
    }
}

fn append(path: &Path) -> io::Result<(File, u64)> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((file, len))
}

impl Write for Rotating {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.len > MAX_LEN {
            self.rotate();
        }
        let n = self.file.write(buf)?;
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_aside_once_too_big() {
        let dir = std::env::temp_dir().join(format!("horizon-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (path, old) = (dir.join("h.log"), dir.join("h.old.log"));
        let mut log = Rotating::open(path.clone(), old.clone()).unwrap();
        let line = vec![b'x'; 64 * 1024];
        for _ in 0..17 {
            log.write_all(&line).unwrap();
        }
        assert!(!old.exists(), "pas encore au-delà de 1 Mo au moment d'écrire");
        log.write_all(b"suite\n").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 17 * 64 * 1024);
        assert_eq!(std::fs::read(&path).unwrap(), b"suite\n");
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
