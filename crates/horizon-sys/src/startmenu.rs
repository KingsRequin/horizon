//! Menu Démarrer : un seul parcours de ses raccourcis (utilisateur, puis tous les
//! utilisateurs), pour la palette, la relance des applis suggérées, le panneau Audio et les
//! raccourcis par défaut. Dans chaque dossier, les fichiers passent avant les sous-dossiers.

use horizon_core::apps::is_noise;
use std::path::{Path, PathBuf};

const PROGRAMS: &str = r"Microsoft\Windows\Start Menu\Programs";
/// Profondeur des sous-dossiers parcourus.
const DEPTH: u32 = 4;

/// Raccourci du menu Démarrer : nom affiché (nom du fichier) et chemin du `.lnk` / `.url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartApp {
    pub name: String,
    pub path: String,
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from)
}

/// Dossiers du menu Démarrer (utilisateur puis commun).
fn roots() -> Vec<PathBuf> {
    ["APPDATA", "PROGRAMDATA"].into_iter().filter_map(env_dir).map(|d| d.join(PROGRAMS)).collect()
}

fn has_ext(p: &Path, exts: &[&str]) -> bool {
    p.extension().is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
}

/// Fichiers d'extension `exts` sous `dir`, jusqu'à `depth` niveaux de sous-dossiers.
fn walk(dir: &Path, depth: u32, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if has_ext(&p, exts) {
            out.push(p);
        }
    }
    if depth > 0 {
        for d in subdirs {
            walk(&d, depth - 1, exts, out);
        }
    }
}

/// Raccourcis `.lnk` du menu Démarrer.
pub fn shortcuts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for r in roots() {
        walk(&r, DEPTH, &["lnk"], &mut out);
    }
    out
}

fn stem(p: &Path) -> Option<String> {
    p.file_stem().map(|s| s.to_string_lossy().to_string())
}

/// Applis du menu Démarrer (`.lnk`, et `.url` des jeux Steam), sans doublon de nom ni
/// désinstalleur ou documentation.
pub fn apps() -> Vec<StartApp> {
    let mut files = Vec::new();
    for r in roots() {
        walk(&r, DEPTH, &["lnk", "url"], &mut files);
    }
    let mut out: Vec<StartApp> = Vec::new();
    for p in files {
        let Some(name) = stem(&p) else { continue };
        if !is_noise(&name) && !out.iter().any(|a| a.name.eq_ignore_ascii_case(&name)) {
            out.push(StartApp { name, path: p.display().to_string() });
        }
    }
    out
}

/// Raccourci `<nom>.lnk` ou `<nom>.url` dans le menu Démarrer, sinon sur un bureau.
pub fn find(name: &str) -> Option<String> {
    let desktops = ["USERPROFILE", "PUBLIC"].into_iter().filter_map(env_dir).map(|d| d.join("Desktop"));
    roots().into_iter().chain(desktops).find_map(|dir| {
        let mut files = Vec::new();
        walk(&dir, 3, &["lnk", "url"], &mut files);
        files.into_iter().find(|p| stem(p).is_some_and(|s| s.eq_ignore_ascii_case(name))).map(|p| p.display().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_come_before_subfolders_and_depth_is_bounded() {
        let root = std::env::temp_dir().join(format!("horizon-startmenu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let deep = root.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        for f in [root.join("a").join("Deux.lnk"), root.join("Un.LNK"), deep.join("Trois.lnk"), root.join("x.txt")] {
            std::fs::write(f, b"").unwrap();
        }
        let mut out = Vec::new();
        walk(&root, 1, &["lnk"], &mut out);
        let names: Vec<String> = out.iter().filter_map(|p| stem(p)).collect();
        assert_eq!(names, ["Un", "Deux"]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
