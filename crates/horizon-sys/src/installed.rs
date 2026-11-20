//! Entrée d'Horizon dans les applications installées de Windows, posée par l'installateur
//! (`packaging/horizon.iss`). La mise à jour automatique remplace les exécutables sans repasser
//! par lui : la version affichée est remise à celle qui tourne.

use crate::registry::Key;
use horizon_core::update::Version;
use std::path::Path;
use windows::core::w;

/// `AppId` de `packaging/horizon.iss` (Inno Setup nomme la clé `<AppId>_is1`).
const APP_ID: &str = "{71B70E6C-14F8-4AC1-B059-24BF2643951A}";

fn entry() -> String {
    format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{APP_ID}_is1")
}

/// Affiche `version` dans les applications installées de Windows, si l'installation de `dir` y
/// figure et affiche autre chose. Vrai si la version affichée a changé.
pub fn sync_version(dir: &Path, version: Version) -> anyhow::Result<bool> {
    sync_at(&entry(), dir, version)
}

fn sync_at(entry: &str, dir: &Path, version: Version) -> anyhow::Result<bool> {
    // Pas d'entrée : installée depuis les sources, ou par l'ancien zip.
    let Ok(key) = Key::open(entry, true) else { return Ok(false) };
    let shown = version.to_string();
    let location = key.string(w!("InstallLocation")).unwrap_or_default();
    if !same_dir(&location, &dir.to_string_lossy()) || key.string(w!("DisplayVersion")).is_ok_and(|v| v == shown) {
        return Ok(false);
    }
    key.set_string(w!("DisplayVersion"), &shown)?;
    for name in [w!("MajorVersion"), w!("VersionMajor")] {
        key.set_dword(name, version.0)?;
    }
    for name in [w!("MinorVersion"), w!("VersionMinor")] {
        key.set_dword(name, version.1)?;
    }
    Ok(true)
}

/// Même dossier, à la casse et à la barre finale près.
fn same_dir(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    !a.is_empty() && norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::Scratch;

    #[test]
    fn app_id_is_the_installer_s() {
        // Inno Setup écrit « {{ » pour une accolade ouvrante.
        let line = format!("AppId={{{APP_ID}");
        let iss = include_str!("../../../packaging/horizon.iss");
        assert!(iss.lines().any(|l| l.trim() == line), "AppId de horizon.iss différent de {APP_ID}");
    }

    #[test]
    fn same_dir_ignores_case_and_trailing_separator() {
        assert!(same_dir("C:\\Users\\Moi\\Programs\\Horizon\\", "c:\\users\\moi\\programs\\horizon"));
        assert!(same_dir("C:/Applis/Horizon", "C:\\Applis\\Horizon\\"));
        assert!(!same_dir("C:\\Applis\\Horizon", "C:\\Applis\\Horizon2"));
        assert!(!same_dir("", ""), "entrée sans dossier : pas la nôtre");
    }

    #[test]
    fn shown_version_follows_the_running_one() {
        let scratch = Scratch::new("version");
        let dir = Path::new("c:\\applis\\horizon");
        {
            let key = Key::open(&scratch.0, true).unwrap();
            key.set_string(w!("InstallLocation"), "C:\\Applis\\Horizon\\").unwrap();
            key.set_string(w!("DisplayVersion"), "0.6.0").unwrap();
            key.set_dword(w!("MinorVersion"), 6).unwrap();
        }
        let shown = || {
            let key = Key::open(&scratch.0, false).unwrap();
            (key.string(w!("DisplayVersion")).unwrap(), key.dword(w!("MajorVersion")).ok(), key.dword(w!("MinorVersion")).ok())
        };
        // Une autre installation (copie lancée d'ailleurs) ne touche pas à l'entrée.
        assert!(!sync_at(&scratch.0, Path::new("D:\\Ailleurs"), Version(1, 7, 2)).unwrap());
        assert_eq!(shown(), ("0.6.0".to_owned(), None, Some(6)));
        assert!(sync_at(&scratch.0, dir, Version(1, 7, 2)).unwrap());
        assert_eq!(shown(), ("1.7.2".to_owned(), Some(1), Some(7)));
        assert!(!sync_at(&scratch.0, dir, Version(1, 7, 2)).unwrap(), "déjà à jour : rien à écrire");
    }

    #[test]
    fn nothing_happens_without_an_entry() {
        assert!(!sync_at("Software\\Horizon-essai-absente", Path::new("C:\\Applis\\Horizon"), Version(1, 0, 0)).unwrap());
    }
}
