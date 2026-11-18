//! Images chargées à la demande (icônes d'exe, images choisies, favicons, applis suggérées) :
//! le cache d'icônes de la version Tauri (`lib/iconCache.ts`). Le travail (extraction par le fil
//! shell, copie de la base des favicons, décodage) se fait sur un fil à part, une demande à la
//! fois ; l'image est rangée sur le fil de l'interface (`slint::Image` n'est pas envoyable).
//!
//! Une seule demande en cours par clé ; une image obtenue est gardée ; un échec n'est gardé que
//! `RETRY` (l'image est redemandée ensuite : Horizon occupé au démarrage, appli installée
//! entre-temps…). Au-delà de `MAX_ENTRIES` clés, les moins récemment demandées sont oubliées.

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const RETRY: Duration = Duration::from_secs(60);
pub const MAX_ENTRIES: usize = 200;

/// Image décodée hors du fil de l'interface.
pub enum Pixels {
    Rgba(SharedPixelBuffer<Rgba8Pixel>),
    /// SVG : rendu par Slint à la taille affichée.
    Svg(Vec<u8>),
}

impl Pixels {
    fn image(self) -> Option<Image> {
        match self {
            Pixels::Rgba(p) => Some(Image::from_rgba8(p)),
            Pixels::Svg(bytes) => Image::load_from_svg_data(&bytes).ok(),
        }
    }
}

/// Décode une image (PNG, JPEG, ICO, WebP) en pixels RGBA.
pub fn decode(bytes: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), w, h))
}

/// Taille maximale d'une image d'icône choisie par l'utilisateur.
pub const MAX_FILE: u64 = 2 * 1024 * 1024;

/// Image d'un fichier choisi comme icône (PNG, SVG, ICO, JPEG, WebP ; 2 Mo au plus), comme
/// `shortcuts_image` de la version Tauri.
pub fn load_file(path: &str) -> Result<Pixels, String> {
    let p = std::path::Path::new(path);
    let size = std::fs::metadata(p).map_err(|e| e.to_string())?.len();
    if size > MAX_FILE {
        return Err("image trop lourde (2 Mo au plus)".into());
    }
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some("png" | "svg" | "ico" | "jpg" | "jpeg" | "webp")) {
        return Err("format d’image non pris en charge".into());
    }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    if ext.as_deref() == Some("svg") {
        return Ok(Pixels::Svg(bytes));
    }
    decode(&bytes).map(Pixels::Rgba).ok_or_else(|| "image illisible".into())
}

/// Rappel d'une demande en attente.
type Waiter = Box<dyn FnOnce(&Image)>;

enum State {
    Ready(Image),
    Pending(Vec<Waiter>),
    Failed(Instant),
}

struct Entry {
    state: State,
    /// Dernière demande (pour oublier les plus anciennes).
    used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    clock: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::default();
}

type Job = Box<dyn FnOnce() + Send>;

/// Fil des chargements (démarré à la première demande).
fn worker() -> Option<crossbeam_channel::Sender<Job>> {
    static TX: std::sync::OnceLock<Option<crossbeam_channel::Sender<Job>>> = std::sync::OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded::<Job>();
        crate::util::spawn_named("icones", move || {
            while let Ok(job) = rx.recv() {
                job();
            }
        })
        .then_some(tx)
    })
    .clone()
}

/// Image de la clé `key` (fil de l'interface) : gardée, sinon demandée à `fetch` (fil des
/// chargements) et `None` en attendant ; `ready` est appelé à son arrivée (pas en cas d'échec).
pub fn get(
    key: &str,
    fetch: impl FnOnce() -> Option<Pixels> + Send + 'static,
    ready: impl FnOnce(&Image) + 'static,
) -> Option<Image> {
    let start = CACHE.with_borrow_mut(|c| {
        c.clock += 1;
        let now = c.clock;
        match c.entries.get_mut(key) {
            Some(e) => {
                e.used = now;
                match &mut e.state {
                    State::Ready(img) => return Err(img.clone()),
                    State::Pending(waiters) => {
                        waiters.push(Box::new(ready));
                        return Ok(false);
                    }
                    State::Failed(at) if at.elapsed() < RETRY => return Ok(false),
                    State::Failed(_) => e.state = State::Pending(vec![Box::new(ready)]),
                }
            }
            None => {
                c.entries.insert(key.to_owned(), Entry { state: State::Pending(vec![Box::new(ready)]), used: now });
                forget_oldest(c);
            }
        }
        Ok(true)
    });
    match start {
        Err(img) => Some(img),
        Ok(false) => None,
        Ok(true) => {
            let k = key.to_owned();
            let job: Job = Box::new(move || {
                let px = fetch();
                let _ = slint::invoke_from_event_loop(move || arrived(&k, px));
            });
            match worker() {
                Some(tx) if tx.send(job).is_ok() => {}
                _ => arrived(key, None),
            }
            None
        }
    }
}

/// Image déjà gardée pour `key`, sans rien demander.
pub fn cached(key: &str) -> Option<Image> {
    CACHE.with_borrow(|c| match c.entries.get(key) {
        Some(Entry { state: State::Ready(img), .. }) => Some(img.clone()),
        _ => None,
    })
}

fn forget_oldest(c: &mut Cache) {
    while c.entries.len() > MAX_ENTRIES {
        // Les demandes en cours restent (leurs rappels attendent).
        let oldest = c
            .entries
            .iter()
            .filter(|(_, e)| !matches!(e.state, State::Pending(_)))
            .min_by_key(|(_, e)| e.used)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                c.entries.remove(&k);
            }
            None => break,
        }
    }
}

/// Résultat d'un chargement (fil de l'interface) : rangé, puis les rappels sont appelés (hors
/// de l'emprunt du cache : ils peuvent redemander des images).
fn arrived(key: &str, px: Option<Pixels>) {
    let img = px.and_then(Pixels::image);
    let waiters = CACHE.with_borrow_mut(|c| {
        let Some(e) = c.entries.get_mut(key) else { return Vec::new() };
        let next = match &img {
            Some(i) => State::Ready(i.clone()),
            None => State::Failed(Instant::now()),
        };
        match std::mem::replace(&mut e.state, next) {
            State::Pending(w) => w,
            _ => Vec::new(),
        }
    });
    if let Some(i) = &img {
        for w in waiters {
            w(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_png() {
        // PNG 1 × 1 rouge opaque.
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 1, 1);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[255, 0, 0, 255]).unwrap();
        }
        let px = decode(&bytes).unwrap();
        assert_eq!((px.width(), px.height()), (1, 1));
        assert_eq!(px.as_slice()[0], Rgba8Pixel { r: 255, g: 0, b: 0, a: 255 });
        assert!(decode(b"pas une image").is_none());
    }

    #[test]
    fn file_formats_and_size_are_checked() {
        let dir = std::env::temp_dir().join(format!("horizon-natif-images-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let txt = dir.join("a.txt");
        std::fs::write(&txt, "x").unwrap();
        assert!(load_file(&txt.to_string_lossy()).is_err());
        let svg = dir.join("a.svg");
        std::fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"/>"#).unwrap();
        assert!(matches!(load_file(&svg.to_string_lossy()), Ok(Pixels::Svg(_))));
        let big = dir.join("b.png");
        std::fs::write(&big, vec![0u8; (MAX_FILE + 1) as usize]).unwrap();
        assert_eq!(load_file(&big.to_string_lossy()).err().as_deref(), Some("image trop lourde (2 Mo au plus)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oldest_entries_are_forgotten_but_not_pending_ones() {
        let mut c = Cache::default();
        for i in 0..MAX_ENTRIES + 5 {
            let state = if i == 0 { State::Pending(Vec::new()) } else { State::Failed(Instant::now()) };
            c.entries.insert(format!("k{i}"), Entry { state, used: i as u64 });
        }
        forget_oldest(&mut c);
        assert_eq!(c.entries.len(), MAX_ENTRIES);
        assert!(c.entries.contains_key("k0"), "demande en cours gardée");
        assert!(!c.entries.contains_key("k1"));
        assert!(c.entries.contains_key(&format!("k{}", MAX_ENTRIES + 4)));
    }
}
