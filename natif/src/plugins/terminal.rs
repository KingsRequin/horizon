//! Terminal SSH demandé par un plugin (`horizon.ssh.open`, action `ssh`).

use horizon_core::plugin::safe_ssh_target;
use std::path::PathBuf;

/// Ouvre un terminal SSH vers `target` : Windows Terminal s'il est installé, sinon `ssh` dans
/// une nouvelle console. `target` est un nom d'hôte, jamais une option ni une commande.
pub fn open_ssh(target: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if !safe_ssh_target(target) {
        return Err("nom d’hôte refusé".into());
    }
    let wt = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\WindowsApps\wt.exe"));
    if let Some(wt) = wt.filter(|p| p.exists()) {
        return std::process::Command::new(wt).args(["ssh", target]).spawn().map(|_| ()).map_err(|e| e.to_string());
    }
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    std::process::Command::new("ssh")
        .arg(target)
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("ssh introuvable ({e})"))
}
