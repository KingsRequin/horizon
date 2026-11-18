//! `git status` d'un dossier, sans fenêtre de console et avec un délai maximal.

use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Pas de fenêtre de console pour le processus lancé.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Sortie de `git status --porcelain=v2 --branch` pour `cwd`, ou `None` (pas un dépôt, git
/// absent, délai dépassé : le processus est alors arrêté).
pub fn status(cwd: &str, timeout: Duration) -> Option<String> {
    // Le cwd vient d'un événement de hook : seul un dossier local est lu (un chemin réseau
    // ferait contacter un serveur par Windows, avec les identifiants de la session).
    if !is_local_drive_path(cwd) || !Path::new(cwd).is_dir() {
        return None;
    }
    let mut child = Command::new("git")
        // Aucune commande tirée de la config du dépôt (fsmonitor, hooks, pager, diff externe) :
        // un dépôt piégé ne doit rien pouvoir lancer par Horizon.
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "core.pager=cat",
            "-c",
            "diff.external=",
        ])
        // Pas de verrou d'index : ne gêne pas un git lancé au même moment par Claude Code.
        .args(["--no-optional-locks", "-C", cwd, "status", "--porcelain=v2", "--branch"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()?;
    // Lecture sur un fil à part : une sortie longue remplirait le tube et bloquerait git.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).ok().map(|_| s)
    });
    let deadline = Instant::now() + timeout;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok().flatten();
    if ok { out } else { None }
}

/// Chemin absolu sur un lecteur local (`C:\...`), sans préfixe réseau ni de périphérique.
fn is_local_drive_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

#[cfg(test)]
mod tests {
    use super::is_local_drive_path;

    #[test]
    fn seuls_les_chemins_locaux_sont_acceptes() {
        assert!(is_local_drive_path(r"E:\Documents\APPLICATION\deskbar"));
        assert!(is_local_drive_path("C:/Users/x"));
        assert!(!is_local_drive_path(r"\\serveur\partage\depot"));
        assert!(!is_local_drive_path("//serveur/partage"));
        assert!(!is_local_drive_path(r"\\?\C:\x"));
        assert!(!is_local_drive_path(r"\\.\pipe\x"));
        assert!(!is_local_drive_path("C:relatif"));
        assert!(!is_local_drive_path(""));
    }
}
