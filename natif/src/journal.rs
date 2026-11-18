//! Journal : `%APPDATA%\Horizon\horizon.log` (mis de côté en `horizon.old.log` au-delà de 1 Mo).

use std::path::PathBuf;

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
    // Ajout plutôt que réécriture : un second lancement (`--quit`) ne doit pas effacer le
    // journal de l'instance en cours. Au-delà de 1 Mo, l'ancien journal est mis de côté.
    let path = path();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("horizon.old.log"));
    }
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = WriteLogger::init(LevelFilter::Info, config, f);
    }
}
