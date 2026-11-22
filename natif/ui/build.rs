//! Compilation de l'interface Slint (`main.slint`) et des panneaux des widgets.
//!
//! Générés dans `$OUT_DIR` :
//! - `panel-view.slint` (importé sous le nom `@panel-view`) : `PanelView` (fenêtre surgissante,
//!   `popup.slint`), qui affiche le panneau `<Id>Panel` de chaque `widgets/<id>/panel.slint` à sa
//!   largeur, et `PanelFill` (tuile d'une barre haute, `tile.slint`), le même à la largeur
//!   imposée ;
//! - `panels.slint` (réexporté par `main.slint` sous le nom `@panels`) : ce que ces fichiers
//!   exportent (leurs globaux et types, pour Rust) ;
//! - `panels.rs` (inclus par `lib.rs`) : `PANELS`, les widgets qui ont un panneau.
//!
//! C'est le seul point d'enregistrement des panneaux : rien à écrire à la main pour en ajouter
//! un.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    if std::env::var_os("CARGO_FEATURE_LIVE").is_some() {
        live();
    }
    let panels = panels(&dir);
    let list: Vec<String> = panels.iter().map(|id| format!("{id:?}")).collect();
    let catalog = format!(
        "/// Widgets qui ont un panneau surgissant (`widgets/<id>/panel.slint`).\npub const PANELS: &[&str] = &[{}];\n",
        list.join(", ")
    );
    write_if_changed(&out_dir.join("panels.rs"), &catalog);
    let (view, exports) = panel_view(&dir, &panels);
    let (view_path, exports_path) = (out_dir.join("panel-view.slint"), out_dir.join("panels.slint"));
    let sources = panels.iter().map(|id| dir.join("widgets").join(id).join("panel.slint"));
    let newest = sources.filter_map(|f| f.metadata().and_then(|m| m.modified()).ok()).max().unwrap_or(SystemTime::UNIX_EPOCH);
    for (path, text) in [(&view_path, &view), (&exports_path, &exports)] {
        write_if_changed(path, text);
        // Daté de la plus récente de ses sources : Slint déclare ce fichier à cargo comme les
        // autres (`rerun-if-changed`), et daté de maintenant il semblerait modifié pendant ce
        // passage. Ce script repartirait alors à la commande suivante, sans aucun changement,
        // et toute la compilation de l'interface avec lui.
        let file = std::fs::File::options().write(true).open(path).expect("fichier généré");
        file.set_modified(newest).expect("date du fichier généré");
    }
    let libraries = std::collections::HashMap::from([
        ("panel-view".to_owned(), view_path),
        ("panels".to_owned(), exports_path),
        ("ui".to_owned(), dir.clone()),
    ]);
    let config = slint_build::CompilerConfiguration::new().with_library_paths(libraries);
    slint_build::compile_with_config("main.slint", config).expect("compilation de main.slint");
}

/// Fonction cargo `live` : demande à Slint une façade qui charge les `.slint` à l'exécution
/// (interpréteur) au lieu de les traduire en Rust. Refusée dans une version publiée, qui doit
/// embarquer son interface.
fn live() {
    println!("cargo:rerun-if-env-changed=HORIZON_RELEASE");
    let published = std::env::var("PROFILE").is_ok_and(|p| p == "release") || std::env::var_os("HORIZON_RELEASE").is_some();
    assert!(!published, "mode live interdit dans une version publiée : retirer `--features live`");
    // SAFETY : un script de compilation n'a qu'un fil, et rien d'autre ne lit l'environnement
    // pendant cet appel. Slint ne connaît pas d'autre interrupteur que cette variable.
    unsafe { std::env::set_var("SLINT_LIVE_PREVIEW", "1") };
}

/// Écrit `text` dans `path` s'il n'y est pas déjà : un fichier réécrit à l'identique ferait
/// recompiler ce qui l'inclut.
fn write_if_changed(path: &Path, text: &str) {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return;
    }
    std::fs::write(path, text).unwrap_or_else(|e| panic!("écriture de {} : {e}", path.display()));
}

/// Identifiants des widgets (`widgets/<id>/widget.json`) qui ont un `panel.slint`, triés.
fn panels(dir: &Path) -> Vec<String> {
    let widgets = dir.join("widgets");
    println!("cargo:rerun-if-changed={}", widgets.display());
    let mut ids: Vec<String> = std::fs::read_dir(&widgets)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", widgets.display()))
        .map(|e| e.expect("entrée de dossier").path())
        .filter(|d| d.join("widget.json").is_file() && d.join("panel.slint").is_file())
        .map(|d| d.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    ids.sort();
    ids
}

/// « claude_quota » → « ClaudeQuota ».
fn camel(id: &str) -> String {
    id.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

/// `panel-view.slint` (`PanelView` : le panneau du genre demandé) et `panels.slint` (les types et
/// globaux exportés par chaque `panel.slint` ; leurs noms doivent être uniques dans Horizon :
/// préfixe `<Id>Panel`).
fn panel_view(dir: &Path, panels: &[String]) -> (String, String) {
    let mut imports = String::new();
    let mut exports = String::from("// Généré par natif/ui/build.rs : ce qu'exporte chaque `widgets/<id>/panel.slint`.\n");
    let mut branches = String::new();
    let mut fills = String::new();
    for id in panels {
        let file = dir.join("widgets").join(id).join("panel.slint");
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{} illisible : {e}", file.display()));
        let component = format!("{}Panel", camel(id));
        assert!(
            text.contains(&format!("export component {component} ")),
            "{} : il faut `export component {component} inherits PanelBase`",
            file.display()
        );
        let uri = format!("@ui/widgets/{id}/panel.slint");
        writeln!(imports, "import {{ {component} }} from \"{uri}\";").unwrap();
        let exported: Vec<&str> = text
            .lines()
            .filter_map(|l| {
                let mut w = l.split_whitespace();
                (w.next() == Some("export") && matches!(w.next(), Some("global" | "struct" | "enum"))).then(|| w.next())?
            })
            .map(|name| name.trim_end_matches('{'))
            .collect();
        if !exported.is_empty() {
            writeln!(exports, "export {{ {} }} from \"{uri}\";", exported.join(", ")).unwrap();
        }
        writeln!(branches, "        if root.kind == {id:?}: {component} {{").unwrap();
        writeln!(branches, "            t: root.t;\n            instance: root.instance;\n            pinned: root.pinned;\n        }}").unwrap();
        writeln!(fills, "        if root.kind == {id:?}: {component} {{").unwrap();
        writeln!(fills, "            t: root.t;\n            instance: root.instance;\n            width: root.width;\n        }}").unwrap();
    }
    let view = [
        "// Généré par natif/ui/build.rs : un panneau par `widgets/<id>/panel.slint`. Ne pas modifier.",
        "import { StyleTokens } from \"@ui/theme.slint\";",
        "import { PluginPanel } from \"@ui/plugin-view.slint\";",
        &imports,
        "// Le panneau du widget `kind` (vide : aucun), et sa taille naturelle.",
        "export component PanelView inherits Rectangle {",
        "    in property <string> kind;",
        "    in property <string> instance;",
        "    in property <StyleTokens> t;",
        "    in property <bool> pinned;",
        "    out property <length> natural-w: col.preferred-width;",
        "    out property <length> natural-h: col.preferred-height;",
        "    col := VerticalLayout {",
        "        if root.kind == \"plugin\" || root.kind.starts-with(\"plugin:\"): PluginPanel { instance: root.instance; }",
        &branches,
        "    }",
        "}",
        "",
        "// Le panneau du widget `kind` à la largeur de l'élément (colonne d'une tuile, `width: auto`",
        "// de Tile.svelte), et sa hauteur naturelle à cette largeur.",
        "export component PanelFill inherits Rectangle {",
        "    in property <string> kind;",
        "    in property <string> instance;",
        "    in property <StyleTokens> t;",
        "    out property <length> natural-h: col.preferred-height;",
        "    col := VerticalLayout {",
        "        alignment: start;",
        "        if root.kind == \"plugin\" || root.kind.starts-with(\"plugin:\"): PluginPanel { instance: root.instance; }",
        &fills,
        "    }",
        "}",
        "",
    ]
    .join("\n");
    (view, exports)
}
