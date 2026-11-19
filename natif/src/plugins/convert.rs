//! Valeurs JSON (scripts) ↔ valeurs Slint interprétées (propriétés et rappels des plugins).
//!
//! Le type d'une propriété est connu au premier niveau (`ValueType`) ; dans un tableau ou une
//! structure, il est deviné : nombre, booléen, chaîne, couleur (`"#rrggbb"`, `"#rrggbbaa"`),
//! tableau, objet. Une image est un chemin relatif au dossier du plugin, vérifié (ni absolu, ni
//! réseau, ni `..`) : seulement pour une propriété `image` de premier niveau.

use serde_json::Value as Json;
use slint::{Brush, Color, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use slint_interpreter::{Struct, Value, ValueType};
use std::path::{Path, PathBuf};

/// Éléments au plus d'un tableau venu d'un script (au-delà : coupé).
pub const MAX_ITEMS: usize = 1000;
/// Côté maximal d'une image de plugin (px), et son poids maximal.
pub const MAX_IMAGE: u32 = 4096;
const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;

/// Un élément de `path` sous `dir` est un lien (lien symbolique, jonction) : refusé, il
/// pourrait mener ailleurs, jusqu'à un partage réseau.
pub fn has_link_below(dir: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(dir) else { return true };
    let mut cur = dir.to_path_buf();
    rel.components().any(|c| {
        cur.push(c);
        std::fs::symlink_metadata(&cur).is_ok_and(|m| m.file_type().is_symlink())
    })
}

/// Chemin `rel` du dossier `dir` s'il y reste : relatif, sans `..`, lecteur, réseau ni lien.
pub fn inside(dir: &Path, rel: &str) -> Option<PathBuf> {
    let n = rel.replace('\\', "/");
    if n.is_empty() || n.starts_with('/') || n.contains(':') || n.split('/').any(|c| c == ".." || c.is_empty()) {
        return None;
    }
    let path = dir.join(&n);
    (!has_link_below(dir, &path)).then_some(path)
}

/// Couleur `#rrggbb` ou `#rrggbbaa`.
fn color(s: &str) -> Option<Color> {
    let hex = s.strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let a = if hex.len() == 8 { byte(6)? } else { 255 };
    Some(Color::from_argb_u8(a, byte(0)?, byte(2)?, byte(4)?))
}

/// Image `rel` du dossier `dir` du plugin (chemin vérifié avant tout accès au disque).
pub fn plugin_image(dir: &Path, rel: &str) -> Option<Image> {
    let path = inside(dir, rel)?;
    // Taille vérifiée avant de décoder (une image piégée occuperait des gigaoctets).
    if std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    // Un seul décodage, borné (dimensions et mémoire) : pas d'écart entre ce qui est vérifié et
    // ce qui est décodé.
    let mut reader = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE);
    limits.max_image_height = Some(MAX_IMAGE);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let rgba = reader.decode().ok()?.to_rgba8();
    let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height());
    Some(Image::from_rgba8(buf))
}

/// Valeur Slint de `v` pour une propriété de type `ty` (`None` : ne convient pas).
pub fn json_to_value(v: &Json, ty: ValueType, dir: &Path) -> Option<Value> {
    Some(match ty {
        ValueType::Number => Value::Number(v.as_f64()?),
        ValueType::String => Value::String(match v {
            Json::String(s) => s.as_str().into(),
            Json::Null => SharedString::new(),
            other => other.to_string().into(),
        }),
        ValueType::Bool => Value::Bool(v.as_bool()?),
        ValueType::Brush => Value::Brush(Brush::SolidColor(color(v.as_str()?)?)),
        ValueType::Image => Value::Image(match v.as_str()? {
            "" => Image::default(),
            rel => plugin_image(dir, rel)?,
        }),
        ValueType::Model => {
            let items: Vec<Value> = v.as_array()?.iter().take(MAX_ITEMS).map(guess).collect();
            Value::Model(ModelRc::new(VecModel::from(items)))
        }
        ValueType::Struct => guess(v),
        _ => return None,
    })
}

/// Valeur Slint devinée pour un élément de tableau ou un champ de structure.
fn guess(v: &Json) -> Value {
    match v {
        Json::Null => Value::Void,
        Json::Bool(b) => Value::Bool(*b),
        Json::Number(n) => Value::Number(n.as_f64().unwrap_or_default()),
        Json::String(s) => match color(s) {
            Some(c) => Value::Brush(Brush::SolidColor(c)),
            None => Value::String(s.as_str().into()),
        },
        Json::Array(a) => Value::Model(ModelRc::new(VecModel::from(a.iter().take(MAX_ITEMS).map(guess).collect::<Vec<_>>()))),
        Json::Object(o) => Value::Struct(o.iter().map(|(k, v)| (k.replace('_', "-"), guess(v))).collect::<Struct>()),
    }
}

/// Valeur JSON d'une valeur Slint (arguments des rappels vers le script).
pub fn value_to_json(v: &Value) -> Json {
    use slint::Model;
    match v {
        // Un entier reste un entier pour le script (`1`, pas `1.0`).
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => Json::from(*n as i64),
        Value::Number(n) => serde_json::Number::from_f64(*n).map_or(Json::Null, Json::Number),
        Value::String(s) => Json::String(s.to_string()),
        Value::Bool(b) => Json::Bool(*b),
        Value::Brush(b) => {
            let c = b.color();
            Json::String(format!("#{:02x}{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue(), c.alpha()))
        }
        Value::Model(m) => Json::Array(m.iter().map(|x| value_to_json(&x)).collect()),
        Value::Struct(s) => Json::Object(s.iter().map(|(k, v)| (k.to_owned(), value_to_json(v))).collect()),
        Value::EnumerationValue(_, name) => Json::String(name.clone()),
        _ => Json::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn convert_roundtrip() {
        let dir = Path::new(".");
        for (v, ty) in [
            (json!("texte"), ValueType::String),
            (json!(12.5), ValueType::Number),
            (json!(true), ValueType::Bool),
            (json!("#11223344"), ValueType::Brush),
            (json!([{ "nom": "a", "n": 1 }, { "nom": "b", "n": 2 }]), ValueType::Model),
        ] {
            let slint = json_to_value(&v, ty, dir).unwrap_or_else(|| panic!("{v}"));
            assert_eq!(value_to_json(&slint), v, "{ty:?}");
        }
        assert_eq!(value_to_json(&json_to_value(&json!("#aabbcc"), ValueType::Brush, dir).unwrap()), json!("#aabbccff"));
        assert!(json_to_value(&json!("x"), ValueType::Number, dir).is_none());
        assert!(json_to_value(&json!("rouge"), ValueType::Brush, dir).is_none());
    }

    #[test]
    fn long_arrays_are_cut() {
        let v = Json::Array((0..5000).map(|i| json!(i)).collect());
        let Some(Value::Model(m)) = json_to_value(&v, ValueType::Model, Path::new(".")) else { panic!() };
        use slint::Model;
        assert_eq!(m.row_count(), MAX_ITEMS);
    }

    #[test]
    fn huge_or_linked_images_are_refused() {
        let dir = std::env::temp_dir().join(format!("horizon-convert-big-{}", std::process::id()));
        let elsewhere = std::env::temp_dir().join(format!("horizon-convert-ailleurs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        image::RgbaImage::from_pixel(MAX_IMAGE + 1, 1, image::Rgba([1, 2, 3, 255])).save(dir.join("assets/large.png")).unwrap();
        assert!(plugin_image(&dir, "assets/large.png").is_none());
        // Jonction (sans droits particuliers) vers un dossier hors du plugin.
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])).save(elsewhere.join("a.png")).unwrap();
        let link = dir.join("lien");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J", &link.to_string_lossy(), &elsewhere.to_string_lossy()])
            .output()
            .is_ok_and(|o| o.status.success());
        if made {
            assert!(plugin_image(&dir, "lien/a.png").is_none(), "image lue à travers une jonction");
        }
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    #[test]
    fn images_stay_in_the_plugin_folder() {
        let dir = std::env::temp_dir().join(format!("horizon-convert-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255])).save(dir.join("assets/a.png")).unwrap();
        assert!(plugin_image(&dir, "assets/a.png").is_some());
        for bad in [r"\\serveur\x.png", "//serveur/x.png", "C:/x.png", "../a.png", "assets/../../a.png", ""] {
            assert!(plugin_image(&dir, bad).is_none(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
