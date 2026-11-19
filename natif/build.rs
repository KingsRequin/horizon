//! Compilation : interface Slint (`ui/main.slint`), catalogue des widgets et des icônes, manifeste
//! Windows de l'exe.
//!
//! Catalogues générés dans `$OUT_DIR` :
//! - `widgets.rs` (inclus par `src/widgets/mod.rs`) : `MANIFESTS`, le `widget.json` de chaque
//!   dossier `ui/widgets/<id>/` (nom, icône, rang de masquage…) ; `PERSONAL_WIDGETS` et
//!   `SQUEEZE_ORDER` en sont tirés ; `PANELS`, les widgets qui ont un `panel.slint` ;
//! - `icons.rs` (inclus par `src/ui/icons.rs`) : `ICONS`, chaque `icons/phosphor/<nom>.svg` ;
//! - `panel-view.slint` (importé sous le nom `@panel-view`) : `PanelView` (fenêtre surgissante,
//!   `ui/popup.slint`), qui affiche le panneau `<Id>Panel` de chaque
//!   `ui/widgets/<id>/panel.slint` à sa largeur, et `PanelFill` (tuile d'une barre haute,
//!   `ui/tile.slint`), le même à la largeur imposée ;
//! - `panels.slint` (réexporté par `ui/main.slint` sous le nom `@panels`) : ce que ces fichiers
//!   exportent (leurs globaux et types, pour Rust).
//!
//! C'est le seul point d'enregistrement des panneaux : rien à écrire à la main pour en ajouter
//! un.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut out = String::new();
    let icons = icons(&dir, &mut out);
    std::fs::write(out_dir.join("icons.rs"), out).expect("écriture du catalogue des icônes");
    let mut out = String::new();
    let panels = widgets(&dir, &icons, &mut out);
    std::fs::write(out_dir.join("widgets.rs"), out).expect("écriture du catalogue des widgets");
    let (view, exports) = panel_view(&dir, &panels);
    let (view_path, exports_path) = (out_dir.join("panel-view.slint"), out_dir.join("panels.slint"));
    std::fs::write(&view_path, view).expect("écriture de panel-view.slint");
    std::fs::write(&exports_path, exports).expect("écriture de panels.slint");
    let libraries = std::collections::HashMap::from([
        ("panel-view".to_owned(), view_path),
        ("panels".to_owned(), exports_path),
        ("ui".to_owned(), dir.join("ui")),
    ]);
    let config = slint_build::CompilerConfiguration::new().with_library_paths(libraries);
    slint_build::compile_with_config("ui/main.slint", config).expect("compilation de ui/main.slint");

    // Manifeste : identité de paquet (notifications), DPI par écran (PerMonitorV2), contrôles
    // communs v6.
    let manifest = dir.join("horizon.exe.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());

    // Icône de l'exe (ressource 1, reprise par les fenêtres : `platform::window`).
    println!("cargo:rerun-if-changed=icons/horizon.ico");
    embed_resource::compile("horizon.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("compilation de horizon.rc");
}

/// Entrées d'un dossier, triées (ordre stable du catalogue).
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", dir.display()))
        .map(|e| e.expect("entrée de dossier").path())
        .collect();
    v.sort();
    v
}

/// `ICONS` : (nom, contenu SVG). Renvoie les noms.
fn icons(dir: &Path, out: &mut String) -> Vec<String> {
    let icons_dir = dir.join("icons/phosphor");
    println!("cargo:rerun-if-changed={}", icons_dir.display());
    let mut names = Vec::new();
    writeln!(out, "/// Icônes Phosphor embarquées : (nom de `lib/icons.ts`, SVG).").unwrap();
    writeln!(out, "pub const ICONS: &[(&str, &[u8])] = &[").unwrap();
    for path in entries(&icons_dir) {
        if path.extension().is_none_or(|e| e != "svg") {
            continue;
        }
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        writeln!(out, "    ({name:?}, include_bytes!({:?})),", path.display().to_string()).unwrap();
        names.push(name);
    }
    writeln!(out, "];").unwrap();
    names
}

/// `MANIFESTS`, `PERSONAL_WIDGETS`, `SQUEEZE_ORDER`, `PANELS`. Renvoie les identifiants des
/// widgets qui ont un panneau.
fn widgets(dir: &Path, icons: &[String], out: &mut String) -> Vec<String> {
    let ui_dir = dir.join("ui/widgets");
    println!("cargo:rerun-if-changed={}", ui_dir.display());
    // Édition publique (sans la fonction `personal`, voir `PERSONAL` de lib.rs) : noms sans les
    // applis personnelles.
    let share = std::env::var_os("CARGO_FEATURE_PERSONAL").is_none();
    let mut manifests = String::new();
    let mut personal = String::new();
    let mut squeeze: Vec<(u64, String)> = Vec::new();
    let mut panels = Vec::new();
    for d in entries(&ui_dir) {
        let manifest = d.join("widget.json");
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        let json: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{} invalide : {e}", manifest.display()));
        let id = d.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "widget « {id} » : minuscules, chiffres et _ seulement"
        );
        let label = json["label"].as_str().unwrap_or_else(|| panic!("{} : « label » manquant", manifest.display()));
        let label = json["shareLabel"].as_str().filter(|_| share).unwrap_or(label);
        let icon = json["icon"].as_str().unwrap_or_else(|| panic!("{} : « icon » manquant", manifest.display()));
        assert!(icons.iter().any(|i| i == icon), "{} : icône « {icon} » absente de icons/phosphor", manifest.display());
        let rank = json["squeeze"].as_u64();
        if let Some(r) = rank {
            assert!(!squeeze.iter().any(|(s, _)| *s == r), "{} : rang de masquage {r} déjà pris", manifest.display());
            squeeze.push((r, id.clone()));
        }
        let flag = |k: &str| json[k] == true;
        // Widget extensible : sa plus petite largeur (« +N » seul), en px logiques.
        let grows = json["grows"].as_u64().map(|v| v as u16);
        assert!(json["grows"].is_null() || grows.is_some(), "{} : « grows » = largeur minimale en px", manifest.display());
        writeln!(
            manifests,
            "    Manifest {{ id: {id:?}, label: {label:?}, icon: {icon:?}, squeeze: {rank:?}, clickable_on_hover: {}, grows: {grows:?}, personal: {}, personal_settings: {} }},",
            flag("clickableOnHover"),
            flag("personal"),
            flag("personalSettings"),
        )
        .unwrap();
        if flag("personal") {
            writeln!(personal, "    {id:?},").unwrap();
        }
        if d.join("panel.slint").is_file() {
            panels.push(id);
        }
    }
    squeeze.sort();
    writeln!(out, "\n/// Manifeste (`widget.json`) de chaque widget, par identifiant.").unwrap();
    writeln!(out, "pub const MANIFESTS: &[Manifest] = &[\n{manifests}];").unwrap();
    writeln!(out, "\n/// Widgets liés aux applis personnelles (`\"personal\": true`), absents de l'édition partagée.")
        .unwrap();
    writeln!(out, "pub const PERSONAL_WIDGETS: &[&str] = &[\n{personal}];").unwrap();
    writeln!(out, "\n/// Ordre de masquage d'une barre trop chargée, du plus secondaire au moins secondaire.").unwrap();
    let order: Vec<String> = squeeze.into_iter().map(|(_, id)| format!("{id:?}")).collect();
    writeln!(out, "pub const SQUEEZE_ORDER: &[&str] = &[{}];", order.join(", ")).unwrap();
    writeln!(out, "
/// Widgets qui ont un panneau surgissant (`ui/widgets/<id>/panel.slint`).").unwrap();
    let list: Vec<String> = panels.iter().map(|id| format!("{id:?}")).collect();
    writeln!(out, "pub const PANELS: &[&str] = &[{}];", list.join(", ")).unwrap();
    panels
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
    let mut exports = String::from("// Généré par natif/build.rs : ce qu'exporte chaque `ui/widgets/<id>/panel.slint`.\n");
    let mut branches = String::new();
    let mut fills = String::new();
    for id in panels {
        let file = dir.join("ui/widgets").join(id).join("panel.slint");
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
        "// Généré par natif/build.rs : un panneau par `ui/widgets/<id>/panel.slint`. Ne pas modifier.",
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
