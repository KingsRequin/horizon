//! Appels du téléphone (fichier de l'agent SharkApp, comme les batteries) : relu quand il
//! change, l'appel en cours est envoyé aux barres (îlot pendant la sonnerie et l'appel). Rien
//! d'autre : SharkApp met déjà le média du PC en pause. Réglage `[features] phone_island`.
//! `HORIZON_PHONE_FILE` remplace le chemin (essai avec un faux fichier).
//!
//! Repris de `app/src-tauri/src/phone.rs` (identique). Affichage : `ui::island`.

use crate::events::EV_PHONE;
use crate::host::{AppHandle, Manager};
use crate::util::{MtimeCache, Published, start_thread};
use horizon_core::phone::{PhoneFile, PhoneView, parse, view};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Relecture : la date du fichier seulement, tant qu'il ne change pas.
const PERIOD: Duration = Duration::from_secs(1);

pub struct PhoneState {
    shown: Published<Option<PhoneView>>,
    /// Dernière mise à jour lue dans le fichier (page « État »).
    file: MtimeCache<Option<PhoneFile>>,
}

impl Default for PhoneState {
    fn default() -> Self {
        PhoneState { shown: Published::new(EV_PHONE, None), file: MtimeCache::new() }
    }
}

pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HORIZON_PHONE_FILE") {
        return Some(PathBuf::from(p));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("KingsHub").join("phone.json"))
}

/// Contenu du fichier ; absent ou illisible : pas d'appel.
fn read(path: &Path) -> Option<PhoneFile> {
    if !path.exists() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    parse(&text)
        .inspect_err(|e| log::warn!("appels : {} illisible : {e:#}", path.display()))
        .ok()
}

/// Dernier contenu lu (page « État »).
pub fn last(app: &AppHandle) -> Option<PhoneFile> {
    app.state::<PhoneState>().file.last().flatten()
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("appels", move || {
        let Some(path) = path() else { return };
        let state = app.state::<PhoneState>();
        loop {
            horizon_sys::presence::wait_while_away();
            let file = state.file.get(&path, || Some(read(&path))).flatten();
            // Relu chaque seconde même sans changement : un fichier figé devient « idle ».
            let now = horizon_core::time::now_ms();
            let shown = if crate::state::settings().features.phone_island { view(file.as_ref(), now) } else { None };
            if state.shown.set(&app, shown.clone()) {
                match &shown {
                    Some(v) => log::info!("appel : {:?}", v.state),
                    None => log::info!("appel : fin"),
                }
            }
            if crate::resume::sleep(PERIOD) {
                state.file.forget();
            }
        }
    })
}

/// Appel en cours (îlot d'une barre qui s'ouvre).
pub fn phone_state(app: &AppHandle) -> Option<PhoneView> {
    app.state::<PhoneState>().shown.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::phone::CallState;

    #[test]
    fn fake_file_is_read_like_sharkapp_writes_it() {
        let dir = std::env::temp_dir().join(format!("horizon-natif-phone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("phone.json");
        assert_eq!(read(&p), None, "absent : pas d'appel");
        let now = horizon_core::time::now_ms();
        let json = format!(r#"{{"state":"ringing","name":null,"number":"0612345678","since_ms":{now},"updated_ms":{now}}}"#);
        std::fs::write(&p, format!("\u{feff}{json}")).unwrap();
        let v = view(read(&p).as_ref(), now + 500).unwrap();
        assert_eq!((v.state, v.label.as_str()), (CallState::Ringing, "0612345678"));
        assert_eq!(view(read(&p).as_ref(), now + 3 * 60_000), None, "figé depuis plus de 2 min");
        std::fs::write(&p, "{").unwrap();
        assert_eq!(read(&p), None, "illisible : pas d'appel");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
