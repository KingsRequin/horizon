//! Identité d'une appli, en un seul endroit : clé stable (AUMID, ou `<dossier éditeur>\<exe>`),
//! nom d'exe, exe réellement lancé par un raccourci (lanceurs Squirrel), nom affichable, et
//! raccourcis du menu Démarrer à ne jamais proposer.

/// Nom d'exe en minuscules à partir d'un chemin (`C:\…\Spotify.exe` → `spotify.exe`).
pub fn exe_key(path: &str) -> String {
    file_name(path).to_lowercase()
}

/// Dernier élément d'un chemin.
fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Dossiers qui ne nomment pas l'éditeur : version Squirrel, dossier d'installation générique,
/// numéro de version.
fn generic_dir(d: &str) -> bool {
    let d = d.to_ascii_lowercase();
    d.starts_with("app-")
        || d == "application"
        || d == "bin"
        || (!d.is_empty() && d.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// Clé stable d'une appli : l'AUMID tel quel, sinon `<dossier éditeur>\<exe>` en minuscules,
/// insensible aux mises à jour qui changent le dossier de version.
pub fn app_key(path_or_aumid: &str) -> String {
    if path_or_aumid.contains('!') {
        return path_or_aumid.to_owned();
    }
    let lower = path_or_aumid.to_ascii_lowercase();
    let mut parts = lower.split(['\\', '/']).filter(|p| !p.is_empty()).rev();
    let Some(exe) = parts.next() else { return lower };
    match parts.find(|d| !generic_dir(d) && !d.ends_with(':')) {
        Some(dir) => format!("{dir}\\{exe}"),
        None => exe.to_owned(),
    }
}

/// Exe démarré par un lanceur Squirrel (`…\Update.exe --processStart Discord.exe`), en
/// minuscules ; `None` si `target` n'est pas un lanceur Squirrel. `Some(None)` : lanceur sans
/// exe nommé.
fn squirrel_started(target: &str, args: &str) -> Option<Option<String>> {
    if exe_key(target) != "update.exe" {
        return None;
    }
    let mut words = args.split_whitespace();
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case("--processStart") {
            return Some(words.next().map(|e| e.trim_matches('"').to_lowercase()));
        }
    }
    Some(None)
}

/// Clé de l'appli lancée par un raccourci (cible + arguments). Un lanceur Squirrel désigne
/// l'exe qu'il démarre, dans le dossier de l'éditeur.
pub fn launcher_app_key(target: &str, args: &str) -> String {
    let key = app_key(target);
    if let Some(Some(exe)) = squirrel_started(target, args)
        && let Some(dir) = key.strip_suffix(r"\update.exe")
    {
        return format!("{dir}\\{exe}");
    }
    key
}

/// Nom d'exe réellement lancé par un raccourci : la cible, sauf pour un lanceur Squirrel qui
/// nomme l'exe dans ses arguments. `None` si ce n'est pas un exe.
pub fn lnk_exe_key(target: &str, args: &str) -> Option<String> {
    match squirrel_started(target, args) {
        Some(started) => started.map(|e| exe_key(&e)),
        None => Some(exe_key(target)).filter(|k| k.ends_with(".exe")),
    }
}

/// Nom lisible d'une appli empaquetée d'après son AUMID (`Éditeur.Nom_hash!App`) : dernier
/// segment non numérique du nom de paquet (`PythonSoftwareFoundation.Python.3.11` → `Python`).
pub fn packaged_name(aumid: &str) -> Option<String> {
    let (pkg, _) = aumid.split_once('!')?;
    let name = pkg.split('_').next()?;
    name.rsplit('.').find(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit())).map(str::to_owned)
}

/// Nom affichable d'une appli (AUMID, chemin ou nom d'exe) : nom du raccourci `.lnk` qui la
/// lance s'il est connu, sinon nom du paquet, sinon nom de l'exe sans extension, avec une
/// majuscule (`spotify.exe` → `Spotify`).
pub fn display_name(app: &str, launch: Option<&str>) -> String {
    if let Some(l) = launch.filter(|l| l.to_ascii_lowercase().ends_with(".lnk")) {
        let name = file_name(l);
        return name[..name.len() - 4].to_owned();
    }
    if app.contains('!') {
        return packaged_name(app).unwrap_or_else(|| app.to_owned());
    }
    let name = file_name(app);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut c = stem.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Classes du shell et des surfaces système : pas une appli utilisée.
const IGNORED_CLASSES: [&str; 9] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "ForegroundStaging",
    "MultitaskingViewFrame",
    "LockScreenControllerProxyWindow",
];
const IGNORED_EXES: [&str; 5] = [
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "lockapp.exe",
    "searchapp.exe",
];

/// Fenêtre à ignorer (bureau, barre des tâches, menu Démarrer, recherche, Alt-Tab, verrouillage).
pub fn is_shell_window(class: &str, exe_name: &str) -> bool {
    IGNORED_CLASSES.contains(&class) || IGNORED_EXES.iter().any(|e| e.eq_ignore_ascii_case(exe_name))
}

/// Raccourci du menu Démarrer à ne jamais proposer : désinstalleur, aide, documentation.
pub fn is_noise(name: &str) -> bool {
    let n = name.to_lowercase();
    ["uninstall", "désinstall", "desinstall", "readme", "lisezmoi", "documentation", "release notes"]
        .iter()
        .any(|w| n.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPDATE: &str = r"C:\Users\u\AppData\Local\Discord\Update.exe";

    #[test]
    fn keys_survive_version_folders() {
        assert_eq!(app_key(r"C:\Users\u\AppData\Local\Discord\app-1.0.9\Discord.exe"), r"discord\discord.exe");
        assert_eq!(app_key(r"C:\Program Files\Vivaldi\Application\vivaldi.exe"), r"vivaldi\vivaldi.exe");
        assert_eq!(app_key("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"), "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App");
        assert_eq!(app_key(r"C:\notepad.exe"), "notepad.exe");
        assert_eq!(exe_key(r"C:\Program Files\Spotify\Spotify.exe"), "spotify.exe");
        assert_eq!(exe_key("Foo.EXE"), "foo.exe");
    }

    #[test]
    fn squirrel_launcher_points_to_started_exe() {
        assert_eq!(launcher_app_key(UPDATE, r#"--processStart "Discord.exe""#), r"discord\discord.exe");
        assert_eq!(launcher_app_key(UPDATE, ""), r"discord\update.exe");
        assert_eq!(launcher_app_key(r"C:\Program Files\App\app.exe", "--x"), r"app\app.exe");
        assert_eq!(lnk_exe_key(UPDATE, "--processStart Discord.exe").as_deref(), Some("discord.exe"));
        assert_eq!(lnk_exe_key(UPDATE, r#"--processStart "Discord.exe""#).as_deref(), Some("discord.exe"));
        assert_eq!(lnk_exe_key(UPDATE, ""), None);
        assert_eq!(lnk_exe_key(r"C:\a\Vivaldi.exe", "").as_deref(), Some("vivaldi.exe"));
        assert_eq!(lnk_exe_key(r"C:\a\notes.txt", ""), None);
    }

    #[test]
    fn display_names() {
        assert_eq!(display_name(r"C:\a\Discord.exe", Some(r"C:\Start\Discord.lnk")), "Discord");
        assert_eq!(display_name(r"C:\a\Discord.exe", Some("https://x")), "Discord");
        assert_eq!(display_name(r"C:\a\Discord.exe", None), "Discord");
        assert_eq!(display_name("spotify.exe", None), "Spotify");
        assert_eq!(display_name("", None), "");
        assert_eq!(display_name("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", None), "WindowsTerminal");
        assert_eq!(display_name("PythonSoftwareFoundation.Python.3.11_qbz5n2kfra8p0!Python", None), "Python");
        assert_eq!(packaged_name("unchihugo.FluentFlyout_69b7b6qge1ahj!App").as_deref(), Some("FluentFlyout"));
        assert_eq!(packaged_name("vivaldi.exe"), None);
    }

    #[test]
    fn shell_windows_are_not_apps() {
        assert!(is_shell_window("Shell_TrayWnd", "explorer.exe"));
        assert!(is_shell_window("Windows.UI.Core.CoreWindow", "x.exe"));
        assert!(is_shell_window("Foo", "SearchHost.exe"));
        assert!(!is_shell_window("CabinetWClass", "explorer.exe"));
        assert!(!is_shell_window("Chrome_WidgetWin_1", "vivaldi.exe"));
    }

    #[test]
    fn noise_is_never_proposed() {
        assert!(is_noise("Uninstall Steam"));
        assert!(is_noise("Désinstaller Truc"));
        assert!(is_noise("ReadMe"));
        assert!(!is_noise("Discord"));
    }
}
