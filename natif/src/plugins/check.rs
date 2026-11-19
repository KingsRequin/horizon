//! Vérification d'un dossier de plugin sans le charger (`horizon.exe --check-plugin <dossier>`,
//! pour qu'un auteur, ou une IA avec un terminal, corrige seul) : manifeste, interface Slint
//! (mêmes règles qu'au chargement), syntaxe du script.

use horizon_core::plugin::parse_manifest;
use std::path::Path;

/// Taille maximale de `main.js` (comme au chargement).
const MAX_MAIN: u64 = 1024 * 1024;

/// Problèmes du plugin du dossier `dir` (« fichier:ligne : message ») ; vide : il se chargera.
pub fn check(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let json = match std::fs::read_to_string(dir.join("plugin.json")) {
        Ok(j) => j,
        Err(e) => return vec![format!("plugin.json : illisible ({e})")],
    };
    let m = match parse_manifest(&json, &super::builtin()) {
        Ok(m) => m,
        Err(e) => return vec![e.to_string()],
    };
    let folder = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if m.id != folder {
        out.push(format!("plugin.json, id : « {} » doit être le nom du dossier (« {folder} »)", m.id));
    }
    if m.widget.is_none() && !m.panel {
        out.push("plugin.json : ni « widget » ni « panel » : le plugin n'affiche rien".to_owned());
    }
    if let Err(diags) = super::slint_host::compile(dir, m.widget.is_some(), m.panel) {
        out.extend(diags.iter().map(ToString::to_string));
    }
    let main = dir.join("main.js");
    match std::fs::metadata(&main) {
        Ok(meta) if meta.len() > MAX_MAIN => out.push("main.js : fichier trop gros (1 Mo au plus)".to_owned()),
        Ok(_) => match std::fs::read_to_string(&main) {
            Ok(src) => out.extend(super::js::syntax_error(&src)),
            Err(e) => out.push(format!("main.js : illisible ({e})")),
        },
        Err(_) => {}
    }
    out
}

/// Ligne de commande : vérifie `dir`, écrit le rapport, renvoie le code de sortie (0 : bon).
pub fn cli(dir: &Path) -> i32 {
    let problems = check(dir);
    if problems.is_empty() {
        println!("{} : plugin valide", dir.display());
        0
    } else {
        println!("{} : {} problème(s)", dir.display(), problems.len());
        for p in &problems {
            println!("  {p}");
        }
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("horizon-check-{}", std::process::id())).join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for (f, t) in files {
            std::fs::write(d.join(f), t).unwrap();
        }
        d
    }

    #[test]
    fn examples_are_valid() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/plugins");
        for dir in std::fs::read_dir(&root).unwrap().flatten().map(|e| e.path()) {
            assert_eq!(check(&dir), Vec::<String>::new(), "{}", dir.display());
        }
    }

    #[test]
    fn reports_each_kind_of_problem() {
        let json = r#"{ "id": "essai", "name": "Essai", "version": "1.0.0", "api": 0, "widget": {} }"#;
        let ok_widget = "export component Widget inherits Window {}\n";
        let d = tmp("essai", &[("plugin.json", json), ("widget.slint", ok_widget), ("main.js", "let a = 1;\nfaute(;\n")]);
        let p = check(&d);
        assert!(p.iter().any(|m| m.starts_with("main.js:2")), "{p:?}");
        let d = tmp("autre", &[("plugin.json", json), ("widget.slint", "export component Foo inherits Window {}\n")]);
        let p = check(&d);
        assert!(p.iter().any(|m| m.contains("doit être le nom du dossier")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("export component Widget inherits Window")), "{p:?}");
        let d = tmp("vide", &[("plugin.json", "{ cassé")]);
        assert!(check(&d)[0].starts_with("plugin.json"));
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
    }
}
