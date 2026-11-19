//! Couleur d'accent : la pochette pendant la lecture (widget média), sinon la couleur du fond
//! d'écran (relu ici chaque minute). Elle va aux fenêtres (`Theme.accent`, `ui::accent`), à
//! Prisme (scène musique pendant la lecture, ambiance sinon) et, si on le veut, à Windows.
//! Réglages `[features] wallpaper_accent` et `windows_accent`.
//!
//! La couleur d'accent de Windows d'origine est gardée dans `accent-windows.json` à la première
//! écriture et remise dès que la fonction s'arrête (même après un redémarrage d'Horizon).
//!
//! Repris de l'`app/src-tauri/src/accent.rs` de la version Tauri. Le chemin d'écriture passe par
//! `AccentIo`, essayé par les tests avec un faux système.

use crate::events::EV_ACCENT;
use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{Waker, lock, start_thread};
use horizon_core::accent::{self, AccentSources};
use horizon_sys::win_accent::{self, AccentValues};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

/// Délai entre deux relevés du fond d'écran (les listes de lecture changent toutes les 15 min).
const EVERY: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Accent {
    inner: Mutex<Inner>,
    waker: Waker,
}

#[derive(Default)]
struct Inner {
    src: AccentSources,
    /// Dernier accent envoyé aux fenêtres.
    bars: Option<[u8; 3]>,
    /// Couleur actuellement appliquée à Windows par Horizon.
    windows: Option<[u8; 3]>,
}

impl Accent {
    /// Réglages modifiés : relevé et application tout de suite.
    pub fn wake(&self) {
        self.waker.wake();
    }
}

/// Pochette (`None` = inchangée) et état de lecture, depuis le widget média.
pub fn set_media(app: &AppHandle, cover: Option<Option<[u8; 3]>>, playing: bool) {
    update(app, |s| {
        if let Some(c) = cover {
            s.cover = c;
        }
        s.playing = playing;
    });
}

fn update(app: &AppHandle, f: impl FnOnce(&mut AccentSources)) {
    let state = app.state::<Accent>();
    let (bars, prisme) = {
        let mut g = lock(&state.inner);
        f(&mut g.src);
        let bars = g.src.bars();
        let changed = (g.bars != bars).then_some(bars);
        g.bars = bars;
        (changed, g.src.prisme())
    };
    if let Some(c) = bars {
        let _ = app.emit(EV_ACCENT, c);
    }
    // Prisme filtre lui-même les répétitions.
    app.state::<crate::prisme::Prisme>().set_accent(prisme);
    crate::plugins::events::emit(app, "accent", serde_json::json!({ "color": prisme.map(crate::plugins::events::hex) }));
}

/// Accent courant des fenêtres, pour une fenêtre qui s'ouvre (commande `accent_state`).
pub fn accent_state(app: &AppHandle) -> Option<[u8; 3]> {
    lock(&app.state::<Accent>().inner).bars
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("fond-d-ecran", move || {
        // Laisse Wallpaper Engine démarrer avec la session.
        std::thread::sleep(Duration::from_secs(5));
        let io = WindowsIo::new(backup_path());
        loop {
            tick(&app, &io);
            app.state::<Accent>().waker.wait(EVERY);
        }
    })
}

fn tick(app: &AppHandle, io: &impl AccentIo) {
    let f = crate::state::settings().features;
    let wallpaper = if !f.wallpaper_accent {
        None
    } else if horizon_sys::presence::away() || crate::game::active() {
        // Rien à voir, ou un jeu à ne pas déranger : on garde la couleur d'avant.
        return;
    } else {
        match read_wallpaper() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("fond d'écran illisible : {e:#}");
                return;
            }
        }
    };
    let state = app.state::<Accent>();
    let previous = lock(&state.inner).src.wallpaper;
    // Le fond animé bouge sans cesse : seul un vrai changement de couleur compte.
    let keep = matches!((previous, wallpaper), (Some(a), Some(b)) if accent::same_accent(a, b));
    if !keep && previous != wallpaper {
        log::info!("couleur du fond d'écran : {}", wallpaper.map_or("aucune".into(), |c| format!("{c:?}")));
        update(app, |s| s.wallpaper = wallpaper);
    }
    let wanted = if f.wallpaper_accent && f.windows_accent { lock(&state.inner).src.wallpaper } else { None };
    let mut current = lock(&state.inner).windows;
    apply_windows(io, &mut current, wanted);
    lock(&state.inner).windows = current;
}

/// Couleur dominante (lisible) du fond de l'écran principal ; `None` s'il est gris ou noir.
fn read_wallpaper() -> anyhow::Result<Option<[u8; 3]>> {
    let main = horizon_sys::monitors::list()
        .into_iter()
        .find(|m| m.primary)
        .ok_or_else(|| anyhow::anyhow!("pas d'écran principal"))?;
    let (w, h, px) = horizon_sys::wallpaper::capture(main.rect)?;
    Ok(accent::dominant_color(&px, w, h).map(accent::readable_accent))
}

fn backup_path() -> PathBuf {
    crate::state::app_dir().join("accent-windows.json")
}

/// Ce que l'accent de Windows lit et écrit : le registre et la sauvegarde de la couleur
/// d'origine.
pub trait AccentIo {
    fn read(&self) -> anyhow::Result<AccentValues>;
    fn write(&self, v: &AccentValues) -> anyhow::Result<()>;
    /// Sauvegarde de la couleur d'origine, si elle existe.
    fn load_backup(&self) -> Option<Vec<u8>>;
    fn has_backup(&self) -> bool;
    fn save_backup(&self, json: &[u8]) -> std::io::Result<()>;
    fn remove_backup(&self);
}

/// Le vrai système : registre de Windows et sauvegarde `path`.
pub struct WindowsIo {
    path: PathBuf,
}

impl WindowsIo {
    pub fn new(path: PathBuf) -> WindowsIo {
        WindowsIo { path }
    }
}

impl AccentIo for WindowsIo {
    fn read(&self) -> anyhow::Result<AccentValues> {
        win_accent::read()
    }

    fn write(&self, v: &AccentValues) -> anyhow::Result<()> {
        win_accent::write(v)
    }

    fn load_backup(&self) -> Option<Vec<u8>> {
        std::fs::read(&self.path).ok()
    }

    fn has_backup(&self) -> bool {
        self.path.exists()
    }

    fn save_backup(&self, json: &[u8]) -> std::io::Result<()> {
        std::fs::write(&self.path, json)
    }

    fn remove_backup(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Applique `wanted` à Windows, ou remet la couleur d'origine (`None`). `current` : couleur
/// appliquée par Horizon, tenue à jour.
fn apply_windows(io: &impl AccentIo, current: &mut Option<[u8; 3]>, wanted: Option<[u8; 3]>) {
    match wanted {
        Some(c) => {
            if current.is_some_and(|cur| accent::same_accent(cur, c)) {
                return;
            }
            if !io.has_backup() {
                // Première écriture : la couleur choisie par l'utilisateur est mise de côté.
                match io.read().map(|v| serde_json::to_vec_pretty(&v)) {
                    Ok(Ok(json)) => {
                        if let Err(e) = io.save_backup(&json) {
                            return log::warn!("accent de Windows : sauvegarde impossible ({e}), rien n'est changé");
                        }
                    }
                    Ok(Err(e)) => return log::warn!("accent de Windows : {e}"),
                    Err(e) => return log::warn!("accent de Windows illisible : {e:#}"),
                }
            }
            let palette = accent::windows_palette(c);
            let values = AccentValues::from_palette(palette);
            // Déjà en place à peu près (Horizon relancé, fond qui bouge d'un rien) : pas de
            // réécriture ni d'annonce à tout Windows.
            let base = |v: &AccentValues| v.palette.get(12..15).map(|b| [b[0], b[1], b[2]]);
            let [r, g, b, _] = palette[3];
            if io.read().is_ok_and(|v| base(&v).is_some_and(|cur| accent::same_accent(cur, [r, g, b]))) {
                *current = Some(c);
                return;
            }
            match io.write(&values) {
                Ok(()) => {
                    log::info!("accent de Windows : {c:?}");
                    *current = Some(c);
                }
                Err(e) => log::warn!("accent de Windows non écrit : {e:#}"),
            }
        }
        None => {
            *current = None;
            let Some(json) = io.load_backup() else { return };
            match serde_json::from_slice::<AccentValues>(&json).map_err(anyhow::Error::from).and_then(|v| io.write(&v)) {
                Ok(()) => {
                    log::info!("accent de Windows d'origine remis");
                    io.remove_backup();
                }
                Err(e) => log::warn!("accent de Windows d'origine non remis : {e:#}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Faux système : registre et sauvegarde en mémoire, écritures comptées.
    struct Fake {
        reg: RefCell<AccentValues>,
        backup: RefCell<Option<Vec<u8>>>,
        writes: RefCell<u32>,
    }

    fn values(rgb: [u8; 3]) -> AccentValues {
        AccentValues::from_palette(accent::windows_palette(rgb))
    }

    impl Fake {
        fn new(rgb: [u8; 3]) -> Fake {
            Fake { reg: RefCell::new(values(rgb)), backup: RefCell::default(), writes: RefCell::default() }
        }
    }

    impl AccentIo for Fake {
        fn read(&self) -> anyhow::Result<AccentValues> {
            Ok(self.reg.borrow().clone())
        }
        fn write(&self, v: &AccentValues) -> anyhow::Result<()> {
            *self.writes.borrow_mut() += 1;
            *self.reg.borrow_mut() = v.clone();
            Ok(())
        }
        fn load_backup(&self) -> Option<Vec<u8>> {
            self.backup.borrow().clone()
        }
        fn has_backup(&self) -> bool {
            self.backup.borrow().is_some()
        }
        fn save_backup(&self, json: &[u8]) -> std::io::Result<()> {
            *self.backup.borrow_mut() = Some(json.to_vec());
            Ok(())
        }
        fn remove_backup(&self) {
            *self.backup.borrow_mut() = None;
        }
    }

    #[test]
    fn windows_accent_is_saved_written_then_restored() {
        let (blue, red) = ([40, 120, 220], [220, 40, 50]);
        let io = Fake::new(blue);
        let mut current = None;
        apply_windows(&io, &mut current, Some(red));
        assert_eq!(current, Some(red));
        assert_eq!(*io.writes.borrow(), 1);
        let saved: AccentValues = serde_json::from_slice(&io.load_backup().unwrap()).unwrap();
        assert_eq!(saved, values(blue), "couleur d'origine mise de côté");
        // Même couleur (ou presque) : rien de réécrit.
        apply_windows(&io, &mut current, Some([222, 42, 52]));
        assert_eq!(*io.writes.borrow(), 1);
        // Fonction coupée : l'origine revient, la sauvegarde disparaît.
        apply_windows(&io, &mut current, None);
        assert_eq!((current, *io.writes.borrow()), (None, 2));
        assert_eq!(*io.reg.borrow(), values(blue));
        assert!(!io.has_backup());
        // Sans sauvegarde, couper ne fait rien.
        apply_windows(&io, &mut current, None);
        assert_eq!(*io.writes.borrow(), 2);
    }

    #[test]
    fn accent_already_in_place_is_not_rewritten() {
        let red = [220, 40, 50];
        let io = Fake::new(red);
        let mut current = None;
        apply_windows(&io, &mut current, Some(red));
        assert_eq!(current, Some(red));
        assert_eq!(*io.writes.borrow(), 0, "Horizon relancé : déjà en place");
        assert!(io.has_backup());
    }
}
