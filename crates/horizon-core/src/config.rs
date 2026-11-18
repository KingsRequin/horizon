//! Lecture de la disposition (`layout.json`) ; écriture atomique et ordonnée des fichiers de
//! configuration.

use crate::geometry::MonitorInfo;
use crate::layout::{LAYOUT_VERSION, Layout};
use std::io::Write;
use std::path::Path;

/// Issue du chargement de `layout.json`.
#[derive(Debug, PartialEq)]
pub enum LayoutLoad {
    /// Fichier lu.
    Loaded(Layout),
    /// Fichier absent, ou illisible mais mis de côté en `layout.json.bak` : la disposition par
    /// défaut peut le remplacer.
    Default(Layout),
    /// Fichier présent qu'on n'a pas pu lire (verrouillé, droits…) ni mettre de côté : la
    /// disposition par défaut sert en mémoire, mais le fichier ne doit pas être écrasé.
    Unreadable(Layout, String),
}

impl LayoutLoad {
    /// La disposition à utiliser, quelle que soit l'issue.
    pub fn into_layout(self) -> Layout {
        match self {
            LayoutLoad::Loaded(l) | LayoutLoad::Default(l) | LayoutLoad::Unreadable(l, _) => l,
        }
    }

    /// Peut-on enregistrer par-dessus le fichier ?
    pub fn writable(&self) -> bool {
        !matches!(self, LayoutLoad::Unreadable(..))
    }
}

/// Charge la disposition. Fichier absent → disposition par défaut. Contenu illisible ou d'une
/// version inconnue → copié en `layout.json.bak` puis disposition par défaut. Fichier
/// impossible à lire → `Unreadable` : à ne pas écraser.
pub fn load_layout(path: &Path, monitors: &[MonitorInfo]) -> LayoutLoad {
    let (bytes, why) = match read(path, monitors) {
        Read::Done(load) => return load,
        Read::Invalid(bytes, why) => (bytes, why),
    };
    let bak = path.with_extension("json.bak");
    match write_atomic(&bak, &bytes) {
        Ok(()) => {
            log::error!("ancien fichier sauvegardé dans {}", bak.display());
            LayoutLoad::Default(Layout::default_for(monitors))
        }
        Err(e) => {
            log::error!("sauvegarde de {} : {e}", path.display());
            LayoutLoad::Unreadable(Layout::default_for(monitors), format!("{why} ; sauvegarde impossible : {e}"))
        }
    }
}

/// Lecture brute : issue définitive, ou contenu invalide (octets et raison) à traiter.
enum Read {
    Done(LayoutLoad),
    Invalid(Vec<u8>, String),
}

fn read(path: &Path, monitors: &[MonitorInfo]) -> Read {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Done(LayoutLoad::Default(Layout::default_for(monitors))),
        Err(e) => {
            log::error!("lecture de {} : {e}", path.display());
            return Read::Done(LayoutLoad::Unreadable(Layout::default_for(monitors), e.to_string()));
        }
    };
    let why = match serde_json::from_slice::<Layout>(&bytes) {
        Ok(l) if l.version == LAYOUT_VERSION => return Read::Done(LayoutLoad::Loaded(l)),
        Ok(l) => format!("version {} inconnue", l.version),
        Err(e) => e.to_string(),
    };
    log::error!("{} illisible ({why}) : disposition par défaut", path.display());
    Read::Invalid(bytes, why)
}

/// Écrit un fichier sans jamais laisser de version à moitié écrite : fichier temporaire voisin,
/// écrit puis forcé sur le disque (`sync_all`), puis renommé par-dessus l'ancien. Le dossier
/// est créé au besoin. Le fichier temporaire est retiré en cas d'échec.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU32, Ordering};
    // Un nom par écriture : deux écritures simultanées du même fichier ne se marchent pas dessus.
    static NEXT: AtomicU32 = AtomicU32::new(0);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_file_name(format!("{name}.{}-{n}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Écritures d'un même fichier faites hors du verrou de l'état qu'il reflète, sans qu'une
/// version ancienne remplace une plus récente : le numéro de version est pris sous le verrou
/// de l'état (`ticket`), l'écriture se fait après l'avoir relâché (`write`).
#[derive(Debug, Default)]
pub struct OrderedWriter {
    next: std::sync::atomic::AtomicU64,
    written: std::sync::Mutex<u64>,
}

impl OrderedWriter {
    pub const fn new() -> OrderedWriter {
        OrderedWriter { next: std::sync::atomic::AtomicU64::new(0), written: std::sync::Mutex::new(0) }
    }

    /// Numéro de la version capturée ; à prendre sous le verrou de l'état.
    pub fn ticket(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    /// Écrit (atomiquement) la version `ticket`, sauf si une plus récente l'a déjà été.
    pub fn write(&self, path: &Path, ticket: u64, bytes: &[u8]) -> std::io::Result<()> {
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        if ticket <= *written {
            return Ok(());
        }
        write_atomic(path, bytes)?;
        *written = ticket;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Zone;
    use crate::layout::tests::real_monitors;
    use crate::testdir::TestDir;

    fn no_tmp_left(d: &TestDir) -> bool {
        std::fs::read_dir(d.join("")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
    }

    #[test]
    fn missing_file_gives_writable_default() {
        let d = TestDir::new("missing");
        let r = load_layout(&d.join("layout.json"), &real_monitors());
        assert!(matches!(&r, LayoutLoad::Default(l) if l.bars.len() == 4));
        assert!(r.writable());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let d = TestDir::new("roundtrip");
        let p = d.join("layout.json");
        let mut l = Layout::default_for(&real_monitors());
        let id = l.bars[0].left[0].id.clone();
        l.move_widget(&id, &l.bars[0].monitor_id.clone(), Zone::Right, 0).unwrap();
        write_atomic(&p, serde_json::to_string_pretty(&l).unwrap().as_bytes()).unwrap();
        assert_eq!(load_layout(&p, &real_monitors()), LayoutLoad::Loaded(l));
        assert!(no_tmp_left(&d));
    }

    #[test]
    fn corrupt_layout_is_kept_aside_then_default() {
        let d = TestDir::new("corrupt");
        let p = d.join("layout.json");
        std::fs::write(&p, "{ pas du json").unwrap();
        let r = load_layout(&p, &real_monitors());
        assert!(matches!(&r, LayoutLoad::Default(l) if l.bars.len() == 4));
        assert_eq!(std::fs::read_to_string(d.join("layout.json.bak")).unwrap(), "{ pas du json");
    }


    #[test]
    fn non_utf8_layout_is_kept_as_bak() {
        let d = TestDir::new("bytes");
        let p = d.join("layout.json");
        std::fs::write(&p, [0xff, 0xfe, 0x00, 0x7b]).unwrap();
        assert!(load_layout(&p, &real_monitors()).writable());
        assert_eq!(std::fs::read(d.join("layout.json.bak")).unwrap(), vec![0xff, 0xfe, 0x00, 0x7b]);
    }

    #[test]
    fn future_version_falls_back_to_default() {
        let d = TestDir::new("future");
        let p = d.join("layout.json");
        let mut l = Layout::default_for(&real_monitors());
        l.version = LAYOUT_VERSION + 1;
        std::fs::write(&p, serde_json::to_string(&l).unwrap()).unwrap();
        assert_eq!(load_layout(&p, &real_monitors()).into_layout().version, LAYOUT_VERSION);
        assert!(d.join("layout.json.bak").exists());
    }

    #[test]
    fn unreadable_file_must_not_be_overwritten() {
        // Un dossier à la place du fichier : la lecture échoue autrement que « absent ».
        let d = TestDir::new("unreadable");
        let p = d.join("layout.json");
        std::fs::create_dir_all(&p).unwrap();
        let r = load_layout(&p, &real_monitors());
        assert!(matches!(&r, LayoutLoad::Unreadable(l, _) if l.bars.len() == 4));
        assert!(!r.writable());
        assert!(!d.join("layout.json.bak").exists());
    }

    #[test]
    fn corrupt_layout_whose_backup_fails_is_unreadable() {
        // Le `.bak` ne peut pas être écrit (un dossier non vide occupe son nom) : sans copie,
        // le fichier de l'utilisateur ne doit pas être remplacé.
        let d = TestDir::new("nobak");
        let p = d.join("layout.json");
        std::fs::write(&p, "{ pas du json").unwrap();
        std::fs::create_dir_all(d.join("layout.json.bak").join("occupé")).unwrap();
        assert!(!load_layout(&p, &real_monitors()).writable());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ pas du json");
        assert!(no_tmp_left(&d));
    }

    #[test]
    fn write_atomic_replaces_and_creates_the_folder() {
        let d = TestDir::new("atomic");
        let p = d.join("sous").join("f.json");
        write_atomic(&p, b"un").unwrap();
        write_atomic(&p, b"deux").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"deux");
        assert!(std::fs::read_dir(d.join("sous")).unwrap().count() == 1);
    }

    #[test]
    fn ordered_writer_never_writes_an_older_version_last() {
        let d = TestDir::new("ordered");
        let p = d.join("f.json");
        let w = OrderedWriter::new();
        let (old, new) = (w.ticket(), w.ticket());
        w.write(&p, new, b"nouveau").unwrap();
        w.write(&p, old, b"ancien").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"nouveau");
        // Un échec n'avance pas la version écrite : la suivante est tentée.
        let failed = w.ticket();
        std::fs::create_dir_all(d.join("dossier").join("x")).unwrap();
        assert!(w.write(&d.join("dossier"), failed, b"x").is_err());
        let next = w.ticket();
        w.write(&p, next, b"suivant").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"suivant");
    }

    #[test]
    fn write_atomic_failure_leaves_no_temporary_file() {
        let d = TestDir::new("atomicfail");
        let p = d.join("cible");
        std::fs::create_dir_all(p.join("occupé")).unwrap();
        assert!(write_atomic(&p, b"x").is_err());
        assert!(no_tmp_left(&d));
    }
}
