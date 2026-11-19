//! Interface Slint des plugins : compilation par l'interpréteur (bibliothèque `@horizon` servie
//! depuis le programme, imports limités au dossier du plugin) et rendu de chaque exemplaire dans
//! une fenêtre hors écran (`platform::offscreen`), posé ensuite comme image dans la barre ou le
//! panneau (méthode validée par l'essai de la branche `essai-plugins`).

use crate::platform::{self, Offscreen};
use slint::platform::software_renderer::PremultipliedRgbaColor;
use slint::platform::{PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, Image, LogicalPosition, PhysicalSize, Rgba8Pixel, SharedPixelBuffer};
use slint_interpreter::{ComponentDefinition, ComponentInstance, Compiler, DiagnosticLevel, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Source de la bibliothèque `@horizon`.
const HORIZON_API: &str = include_str!("../../ui/plugin-api/horizon.slint");
/// Chemin virtuel de la bibliothèque (jamais lu sur le disque).
const API_DIR: &str = "horizon-plugin-api";

/// Erreur de compilation, avec sa place dans le fichier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    /// Fichier relatif au dossier du plugin (« widget.slint »).
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl std::fmt::Display for Diag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line > 0 { write!(f, "{}:{}:{} : {}", self.file, self.line, self.column, self.message) } else { write!(f, "{} : {}", self.file, self.message) }
    }
}

/// Composants compilés d'un plugin.
pub struct Compiled {
    pub widget: Option<ComponentDefinition>,
    pub panel: Option<ComponentDefinition>,
}

/// Propriétés standard qu'Horizon pose sur un exemplaire, si son composant les déclare.
pub const STD_PROPS: [&str; 5] = ["compact", "bar-height", "limit", "hovered", "instance"];

/// Compile `widget.slint` (si `widget`) et `panel.slint` (si `panel`) du dossier `dir`.
pub fn compile(dir: &Path, widget: bool, panel: bool) -> Result<Compiled, Vec<Diag>> {
    let widget = widget.then(|| compile_one(dir, "widget.slint", "Widget")).transpose()?;
    let panel = panel.then(|| compile_one(dir, "panel.slint", "Panel")).transpose()?;
    Ok(Compiled { widget, panel })
}

/// Propriétés standard que le composant déclare.
pub fn std_props_of(def: &ComponentDefinition) -> Vec<&'static str> {
    let declared: Vec<String> = def.properties().map(|(name, _)| name.replace('_', "-")).collect();
    STD_PROPS.into_iter().filter(|p| declared.iter().any(|d| d == p)).collect()
}

/// Événement de souris transmis à un exemplaire (coordonnées logiques dans l'exemplaire).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pointer {
    Move(f32, f32),
    Down(f32, f32),
    Up(f32, f32),
    /// Molette : position et crans (positif vers le haut).
    Wheel(f32, f32, f32),
    Leave,
}

/// Valeurs des propriétés standard d'un exemplaire.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StdProps {
    pub compact: bool,
    pub bar_height: f32,
    pub limit: f32,
    pub hovered: bool,
    pub instance: String,
}

/// Un exemplaire rendu hors écran (widget d'une barre ou panneau).
pub struct Surface {
    inst: ComponentInstance,
    win: Rc<Offscreen>,
    std: Vec<&'static str>,
    scale: f32,
    /// Taille du tampon (px physiques).
    size: (u32, u32),
    buf: Vec<PremultipliedRgbaColor>,
}

impl Surface {
    /// Crée un exemplaire de `def` à l'échelle `scale` (fil de l'interface).
    pub fn new(def: &ComponentDefinition, scale: f32) -> anyhow::Result<Surface> {
        let (inst, win) = platform::offscreen(|| def.create());
        let inst = inst.map_err(|e| anyhow::anyhow!("création de l'exemplaire : {e}"))?;
        let win = win.ok_or_else(|| anyhow::anyhow!("fenêtre hors écran absente"))?;
        win.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        inst.show().map_err(|e| anyhow::anyhow!("affichage hors écran : {e}"))?;
        Ok(Surface { inst, win, std: std_props_of(def), scale, size: (0, 0), buf: Vec::new() })
    }

    pub fn instance(&self) -> &ComponentInstance {
        &self.inst
    }

    /// Largeur voulue par le composant (px logiques).
    pub fn preferred_width(&self) -> f32 {
        self.preferred().0
    }

    /// Hauteur voulue par le composant (px logiques).
    pub fn preferred_height(&self) -> f32 {
        self.preferred().1
    }

    fn preferred(&self) -> (f32, f32) {
        use i_slint_core::item_tree::ItemTreeRc;
        use i_slint_core::layout::Orientation;
        let inner = i_slint_core::window::WindowInner::from_pub(self.win.window());
        let c = inner.component();
        let c = ItemTreeRc::borrow_pin(&c);
        let h = c.as_ref().layout_info(Orientation::Horizontal).preferred_bounded();
        let v = c.as_ref().layout_info(Orientation::Vertical).preferred_bounded();
        (h, v)
    }

    /// Pose les propriétés standard que le composant déclare.
    pub fn set_std(&self, p: &StdProps) {
        for name in &self.std {
            let v = match *name {
                "compact" => Value::Bool(p.compact),
                "bar-height" => Value::Number(f64::from(p.bar_height)),
                "limit" => Value::Number(f64::from(p.limit)),
                "hovered" => Value::Bool(p.hovered),
                _ => Value::String(p.instance.as_str().into()),
            };
            if let Err(e) = self.inst.set_property(name, v) {
                log::warn!("plugin : propriété {name} : {e}");
            }
        }
    }

    /// Image de l'exemplaire à la taille logique `w` × `h` ; `None` si rien n'a changé.
    pub fn render(&mut self, w: f32, h: f32) -> Option<Image> {
        let size = ((w * self.scale).round().max(1.0) as u32, (h * self.scale).round().max(1.0) as u32);
        if size != self.size {
            self.size = size;
            self.buf = vec![PremultipliedRgbaColor::default(); (size.0 * size.1) as usize];
            self.win.set_size(PhysicalSize::new(size.0, size.1));
            self.win.request_redraw();
        }
        let mut drew = false;
        let (bw, buf) = (size.0 as usize, &mut self.buf);
        self.win.draw_if_needed(|r| {
            buf.fill(PremultipliedRgbaColor::default());
            r.render(buf, bw);
            drew = true;
        });
        if !drew {
            return None;
        }
        let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(size.0, size.1);
        for (d, s) in px.make_mut_slice().iter_mut().zip(&self.buf) {
            *d = Rgba8Pixel { r: s.red, g: s.green, b: s.blue, a: s.alpha };
        }
        Some(Image::from_rgba8_premultiplied(px))
    }

    /// Une animation ou un changement attend d'être dessiné.
    pub fn dirty(&self) -> bool {
        self.win.has_active_animations()
    }

    /// Transmet un événement de souris.
    pub fn pointer(&self, ev: Pointer) {
        let w = self.win.window();
        let at = |x: f32, y: f32| LogicalPosition::new(x, y);
        let event = match ev {
            Pointer::Move(x, y) => WindowEvent::PointerMoved { position: at(x, y) },
            Pointer::Down(x, y) => WindowEvent::PointerPressed { position: at(x, y), button: PointerEventButton::Left },
            Pointer::Up(x, y) => WindowEvent::PointerReleased { position: at(x, y), button: PointerEventButton::Left },
            Pointer::Wheel(x, y, notches) => WindowEvent::PointerScrolled { position: at(x, y), delta_x: 0.0, delta_y: notches * 40.0 },
            Pointer::Leave => WindowEvent::PointerExited,
        };
        w.dispatch_event(event);
    }

    /// Transmet une touche (texte de la touche, appui ou relâchement).
    pub fn key(&self, text: &str, pressed: bool) {
        let text: slint::SharedString = text.into();
        let event = if pressed { WindowEvent::KeyPressed { text } } else { WindowEvent::KeyReleased { text } };
        self.win.window().dispatch_event(event);
    }
}

/// Chemin sans `.` ni `..` (résolus sans toucher au disque), en minuscules pour comparer.
fn lexical(p: &Path) -> String {
    use std::path::Component;
    let mut out: Vec<String> = Vec::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str().to_string_lossy().to_lowercase()),
        }
    }
    out.join("\\").replace("\\\\", "\\")
}

/// Compile le fichier `file` du plugin et en tire le composant `component`.
fn compile_one(dir: &Path, file: &str, component: &str) -> Result<ComponentDefinition, Vec<Diag>> {
    let diag = |message: String| vec![Diag { file: file.to_owned(), line: 0, column: 0, message }];
    let path = dir.join(file);
    if !path.is_file() {
        return Err(diag(format!("fichier absent (il faut `export component {component} inherits Window`)")));
    }
    check_paths(dir)?;
    let mut compiler = Compiler::default();
    let api = PathBuf::from(API_DIR).join("horizon.slint");
    compiler.set_library_paths(HashMap::from([("horizon".to_owned(), api)]));
    let root = lexical(dir);
    compiler.set_file_loader(move |p: &Path| {
        let answer: Option<std::io::Result<String>> = if p.components().any(|c| c.as_os_str() == API_DIR) {
            Some(Ok(HORIZON_API.to_owned()))
        } else if lexical(p).starts_with(&format!("{root}\\")) {
            None
        } else {
            Some(Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "import hors du dossier du plugin")))
        };
        Box::pin(std::future::ready(answer))
    });
    let result = spin_on(compiler.build_from_path(&path));
    let errors: Vec<Diag> = result
        .diagnostics()
        .filter(|d| d.level() == DiagnosticLevel::Error)
        .map(|d| {
            let (line, column) = d.line_column();
            let shown = d
                .source_file()
                .map(|f| f.strip_prefix(dir).unwrap_or(f).to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|| file.to_owned());
            Diag { file: shown, line, column, message: d.message().to_owned() }
        })
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    result
        .component(component)
        .ok_or_else(|| diag(format!("il faut `export component {component} inherits Window`")))
}

/// Chemin cité dans un `.slint` (`@image-url`, `import`) : il doit rester dans le dossier du
/// plugin. Refusés : chemin absolu, lecteur, réseau (`\\serveur` : Windows y enverrait les
/// identifiants de la session), `..`. Les bibliothèques (`@horizon`, `std-widgets.slint`) passent.
fn path_allowed(p: &str) -> bool {
    if p.starts_with('@') || p == "std-widgets.slint" {
        return true;
    }
    let p = p.replace('\\', "/");
    !(p.starts_with('/') || p.contains(':') || p.split('/').any(|c| c == ".."))
}

/// Vérifie les chemins cités par tous les `.slint` du dossier : les images ne passent pas par
/// le chargeur de fichiers, elles sont donc contrôlées ici, avant la compilation. Toute chaîne
/// qui suit `@image-url(`, `import` ou `from` (même sur la ligne suivante) est examinée.
fn check_paths(dir: &Path) -> Result<(), Vec<Diag>> {
    let mut errors = Vec::new();
    let files = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path());
    for f in files.filter(|f| f.extension().is_some_and(|e| e == "slint")) {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        for (at, p) in quoted_paths(&text) {
            if !path_allowed(&p) {
                let line = text[..at].matches('\n').count() + 1;
                errors.push(Diag { file: name.clone(), line, column: 1, message: format!("« {p} » : chemin hors du dossier du plugin") });
            }
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Chaînes (position du mot qui les introduit, contenu brut) qui suivent `@image-url(`, `import`
/// ou `from`, blancs et retours à la ligne sautés.
fn quoted_paths(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for word in ["@image-url(", "import", "from"] {
        for (at, _) in text.match_indices(word) {
            let rest = text[at + word.len()..].trim_start();
            if rest.starts_with('"')
                && let Some(s) = first_string(rest)
            {
                out.push((at, s));
            }
        }
    }
    out
}

/// Contenu brut de la chaîne `"…"` qui commence `s` (échappements gardés tels quels : `\\` y
/// reste double, donc refusé comme chemin réseau).
fn first_string(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => {
                out.push(c);
                out.push(chars.next()?);
            }
            c => out.push(c),
        }
    }
    None
}

/// Attend un futur qui ne dépend d'aucun réveil extérieur (compilation Slint, fichiers lus
/// d'un coup).
fn spin_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("horizon-slint-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (f, text) in files {
            std::fs::write(dir.join(f), text).unwrap();
        }
        dir
    }

    const WIDGET: &str = r#"import { Island, Pill, Theme } from "@horizon";
export component Widget inherits Window {
    in property <bool> compact;
    in property <string> instance;
    background: transparent;
    Island { Pill { text: "3"; } }
}
"#;

    #[test]
    fn compiles_minimal_widget() {
        let dir = plugin("ok", &[("widget.slint", WIDGET)]);
        let c = compile(&dir, true, false).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(c.widget.is_some());
        assert!(c.panel.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reports_file_and_line() {
        let dir = plugin("err", &[("widget.slint", "export component Widget inherits Window {\n    width: 10px;\n    nimporte: 3;\n}\n")]);
        let e = compile(&dir, true, false).err().expect("erreur attendue");
        assert_eq!(e[0].file, "widget.slint");
        assert_eq!(e[0].line, 3, "{e:?}");
        assert!(e[0].to_string().starts_with("widget.slint:3:"), "{}", e[0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn requires_widget_component() {
        let dir = plugin("name", &[("widget.slint", "export component Foo inherits Window {}\n")]);
        let e = compile(&dir, true, false).err().expect("erreur attendue");
        assert!(e[0].message.contains("export component Widget inherits Window"), "{e:?}");
        let dir2 = plugin("nopanel", &[("widget.slint", WIDGET)]);
        let e = compile(&dir2, true, true).err().expect("panneau absent");
        assert_eq!(e[0].file, "panel.slint");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn imports_stay_in_the_plugin_folder() {
        let outside = plugin("dehors", &[("secret.slint", "export component S inherits Rectangle {}\n")]);
        let rel = format!("../{}/secret.slint", outside.file_name().unwrap().to_str().unwrap());
        let text = format!("import {{ S }} from \"{rel}\";\nexport component Widget inherits Window {{ S {{}} }}\n");
        let dir = plugin("import", &[("widget.slint", &text)]);
        assert!(compile(&dir, true, false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn resources_stay_in_the_plugin_folder() {
        for bad in [
            r#"@image-url("\\serveur\partage\x.png")"#,
            r#"@image-url("//serveur/partage/x.png")"#,
            r#"@image-url("C:/Windows/x.png")"#,
            r#"@image-url("../x.png")"#,
            r#"@image-url("assets/../../x.png")"#,
            "@image-url(\n    \"//serveur/x.png\")",
        ] {
            let text = format!("export component Widget inherits Window {{ Image {{ source: {bad}; }} }}\n");
            let dir = plugin("res", &[("widget.slint", &text)]);
            let e = compile(&dir, true, false).err().unwrap_or_else(|| panic!("accepté : {bad}"));
            assert_eq!(e[0].line, 1, "{bad}");
            assert!(e[0].message.contains("hors du dossier"), "{e:?}");
            let _ = std::fs::remove_dir_all(&dir);
        }
        let text = "import \"assets/font.ttf\";\nexport component Widget inherits Window {}\n";
        let ok = plugin("res-ok", &[("widget.slint", text)]);
        // Police absente : erreur du compilateur, pas du contrôle des chemins.
        let e = compile(&ok, true, false).err().unwrap_or_default();
        assert!(e.iter().all(|d| !d.message.contains("hors du dossier")), "{e:?}");
        let _ = std::fs::remove_dir_all(&ok);
    }

    #[test]
    fn std_props_only_if_declared() {
        let dir = plugin("std", &[("widget.slint", WIDGET)]);
        let c = compile(&dir, true, false).unwrap();
        assert_eq!(std_props_of(c.widget.as_ref().unwrap()), ["compact", "instance"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
