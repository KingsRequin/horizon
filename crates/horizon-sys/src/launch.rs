//! Ouverture des raccourcis et recherche des applis installées.

use std::path::PathBuf;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

/// Identifiant d'appli de Windows Terminal (application empaquetée).
const TERMINAL_APP: &str = r"shell:AppsFolder\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App";

/// Ouvre une cible (exe, .lnk, dossier, URL, `shell:`) comme un double-clic, sur le fil shell
/// (sans attendre).
pub fn launch(target: &str) {
    let target = target.to_owned();
    crate::shell::spawn(move || {
        // SAFETY : chaînes valides pendant l'appel ; COM initialisé par le fil shell.
        let r = unsafe {
            ShellExecuteW(None, w!("open"), &HSTRING::from(target.as_str()), None, None, SW_SHOWNORMAL)
        };
        // ShellExecute renvoie une valeur > 32 en cas de succès.
        if r.0 as isize <= 32 {
            log::warn!("ouverture de {target} impossible (code {})", r.0 as isize);
        }
    });
}

/// Raccourci d'une appli installée : `<nom>.lnk` ou `<nom>.url` (jeux Steam) dans le menu
/// Démarrer, sinon sur un bureau.
/// « Terminal » renvoie l'appli empaquetée Windows Terminal si elle est installée.
pub fn find_app(name: &str) -> Option<String> {
    if name == "Terminal" {
        let alias = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(r"Microsoft\WindowsApps\wt.exe");
        return alias.exists().then(|| TERMINAL_APP.to_owned());
    }
    crate::startmenu::find(name)
}

/// Montre un fichier sélectionné dans l'Explorateur.
pub fn reveal(path: &str) {
    use std::os::windows::process::CommandExt;
    // `/select,"chemin"` : l'Explorateur attend les guillemets autour du seul chemin (un nom de
    // fichier Windows ne peut pas en contenir).
    if let Err(e) = std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn() {
        log::warn!("Explorateur : {e}");
    }
}
