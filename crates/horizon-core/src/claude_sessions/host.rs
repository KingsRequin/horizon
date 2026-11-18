//! Fenêtre du terminal qui héberge une session (processus ancêtres du hook).

use serde::Deserialize;

/// Processus hôtes gardés par session (la chaîne envoyée par le hook est bornée elle aussi).
pub(super) const MAX_PROCS: usize = 24;

/// Processus ancêtre du hook (terminal, éditeur…), ajouté à l'événement par `horizon-hook`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HostProc {
    pub pid: u32,
    /// Nom de l'exécutable (« WindowsTerminal.exe ») : un PID réutilisé ne trompe pas.
    pub name: String,
}

/// Fenêtre principale à mettre au premier plan pour une session : celle du premier processus
/// hôte (du plus proche au plus lointain) qui tourne toujours sous le même nom et possède une
/// fenêtre ; parmi les fenêtres de ce processus, celle de la console du hook (`hint`) d'abord
/// (Windows Terminal fait tourner toutes ses fenêtres dans un seul processus).
/// `exe_name(pid)` donne le nom de l'exécutable actuel du PID ; `windows` liste les fenêtres
/// principales visibles (fenêtre, PID propriétaire).
pub fn pick_host_window(
    procs: &[HostProc],
    hint: Option<isize>,
    exe_name: impl Fn(u32) -> Option<String>,
    windows: &[(isize, u32)],
) -> Option<isize> {
    procs.iter().find_map(|p| {
        let mut own = windows.iter().filter(|(_, pid)| *pid == p.pid);
        let w = own.clone().find(|(h, _)| Some(*h) == hint).or_else(|| own.next())?;
        exe_name(p.pid).filter(|n| n.eq_ignore_ascii_case(&p.name)).map(|_| w.0)
    })
}
