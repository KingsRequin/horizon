//! Compilation : catalogue des widgets et des icônes, manifeste Windows et icône de l'exe.
//! L'interface Slint (`ui/`) est compilée par son propre paquet, `horizon-ui` (`ui/build.rs`).
//!
//! Catalogues générés dans `$OUT_DIR` :
//! - `widgets.rs` (inclus par `src/widgets/mod.rs`) : `MANIFESTS`, le `widget.json` de chaque
//!   dossier `ui/widgets/<id>/` (nom, icône, rang de masquage…), et `SQUEEZE_ORDER`, qui en est
//!   tiré ;
//! - `icons.rs` (inclus par `src/ui/icons.rs`) : `ICONS`, chaque `icons/phosphor/<nom>.svg`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut out = String::new();
    let icons = icons(&dir, &mut out);
    std::fs::write(out_dir.join("icons.rs"), out).expect("écriture du catalogue des icônes");
    let mut out = String::new();
    widgets(&dir, &icons, &mut out);
    std::fs::write(out_dir.join("widgets.rs"), out).expect("écriture du catalogue des widgets");

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

/// `MANIFESTS` et `SQUEEZE_ORDER`.
fn widgets(dir: &Path, icons: &[String], out: &mut String) {
    let ui_dir = dir.join("ui/widgets");
    println!("cargo:rerun-if-changed={}", ui_dir.display());
    let mut manifests = String::new();
    let mut squeeze: Vec<(u64, String)> = Vec::new();
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
            "    Manifest {{ id: {id:?}, label: {label:?}, icon: {icon:?}, squeeze: {rank:?}, clickable_on_hover: {}, grows: {grows:?} }},",
            flag("clickableOnHover"),
        )
        .unwrap();
    }
    squeeze.sort();
    writeln!(out, "\n/// Manifeste (`widget.json`) de chaque widget, par identifiant.").unwrap();
    writeln!(out, "pub const MANIFESTS: &[Manifest] = &[\n{manifests}];").unwrap();
    writeln!(out, "\n/// Ordre de masquage d'une barre trop chargée, du plus secondaire au moins secondaire.").unwrap();
    let order: Vec<String> = squeeze.into_iter().map(|(_, id)| format!("{id:?}")).collect();
    writeln!(out, "pub const SQUEEZE_ORDER: &[&str] = &[{}];", order.join(", ")).unwrap();
}
