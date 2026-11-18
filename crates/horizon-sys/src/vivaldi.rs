//! Emplacements de Vivaldi : profils (historique, icônes) et exécutable.

use std::path::{Path, PathBuf};

/// Un profil Vivaldi : nom du dossier (`Default`, `Profile 1`…) et ses bases.
#[derive(Debug, Clone)]
pub struct Profile {
    pub dir: String,
    pub history: PathBuf,
    pub favicons: PathBuf,
}

fn user_data() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Vivaldi\User Data"))
}

/// Profils présents (ceux qui ont un historique).
pub fn profiles() -> Vec<Profile> {
    let Some(root) = user_data() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&root) else { return Vec::new() };
    let mut out: Vec<Profile> = entries
        .flatten()
        .filter_map(|e| {
            let dir = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            let history = p.join("History");
            ((dir == "Default" || dir.starts_with("Profile ")) && history.is_file()).then(|| Profile {
                favicons: p.join("Favicons"),
                history,
                dir,
            })
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Exécutable de Vivaldi (installation par utilisateur, sinon pour tous).
pub fn exe() -> Option<PathBuf> {
    let candidates = [
        std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join(r"Vivaldi\Application\vivaldi.exe")),
        std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join(r"Vivaldi\Application\vivaldi.exe")),
    ];
    candidates.into_iter().flatten().find(|p| p.is_file())
}

/// Ouvre une adresse dans Vivaldi, dans ce profil (un nouvel onglet si Vivaldi tourne déjà).
pub fn open(url: &str, profile: &str) {
    let Some(exe) = exe() else {
        log::warn!("Vivaldi introuvable : {url} ouvert dans le navigateur par défaut");
        crate::launch::launch(url);
        return;
    };
    let r = std::process::Command::new(&exe)
        .arg(format!("--profile-directory={profile}"))
        .arg(url)
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .spawn();
    if let Err(e) = r {
        log::warn!("ouverture de {url} dans Vivaldi : {e}");
    }
}
