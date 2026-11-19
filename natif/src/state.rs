//! État partagé : disposition chargée, écrans connus, chemin de la config.
//!
//! Repris de l'`app/src-tauri/src/state.rs` de la version Tauri.
//!
//! # Ordre des verrous
//!
//! Un fil qui doit tenir plusieurs verrous les prend toujours dans cet ordre, jamais l'inverse :
//!
//! 1. `AppState::layout` (disposition) ;
//! 2. `AppState::undo` (historique d'annulation) ;
//! 3. `AppState::renamed`, puis l'écriture de `layout.json` (`AppState::writer`).
//!
//! `Bars` (fenêtres de barre) ne se combine avec aucun d'eux : on en copie ce qu'il faut
//! (étiquettes, écrans) et on le relâche avant de lire la disposition ; on ne crée ni ne
//! détruit de fenêtre en le tenant. Aucun verrou n'est tenu pendant une écriture de fichier,
//! sauf celui de `writer`, qui ne sert qu'à cela, ni pendant un `emit`.

use horizon_core::config::{self, LayoutLoad, OrderedWriter};
use horizon_core::geometry::MonitorInfo;
use horizon_core::layout::{BarLayout, Layout, UndoStack, WidgetKind};
use horizon_core::settings::{Settings, SettingsFile};
use std::path::PathBuf;
use std::sync::Mutex;
use crate::util::{MtimeCache, lock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Dossier `%APPDATA%\Horizon`.
pub fn app_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Horizon")
}

/// Réglages globaux (`settings.toml`, modifiable à la main). Relus seulement quand la date de
/// modification du fichier change : un fichier illisible n'est signalé qu'une fois. Une lecture
/// impossible (fichier verrouillé un instant) n'est pas gardée : elle est retentée au prochain
/// appel. Un fichier absent est créé (commenté), un fichier illisible mis de côté
/// (`Settings::read`).
pub fn settings() -> Settings {
    static CACHE: MtimeCache<Settings> = MtimeCache::new();
    let path = settings_path();
    CACHE
        .get(&path, || {
            Settings::read(&path)
                .inspect_err(|e| log::warn!("lecture de {} : {e} : réglages précédents pour cette fois", path.display()))
                .ok()
        })
        .unwrap_or_default()
}

/// `%APPDATA%\Horizon\settings.toml`.
pub fn settings_path() -> PathBuf {
    app_dir().join("settings.toml")
}

/// Enregistre les réglages (fenêtre « Réglages d'Horizon ») : `settings.toml` réécrit, commenté
/// (refusé s'il est illisible).
pub fn save_settings(s: &Settings) -> anyhow::Result<()> {
    save_settings_to(s, &settings_path())
}

fn save_settings_to(s: &Settings, path: &std::path::Path) -> anyhow::Result<()> {
    if let SettingsFile::Invalid(why) = Settings::inspect(path)? {
        anyhow::bail!("settings.toml est illisible ({why}) : corrige-le ou supprime-le d’abord");
    }
    s.clone().bounded().save(path)?;
    Ok(())
}

pub struct AppState {
    layout: Mutex<Layout>,
    undo: Mutex<UndoStack>,
    pub layout_path: PathBuf,
    /// Écrans qui ont changé d'identifiant (ancien, nouveau), pas encore reportés dans le
    /// journal des suggestions.
    renamed: Mutex<Vec<(String, String)>>,
    /// Écriture de `layout.json` ; numéro de version donné sous le verrou `layout`.
    writer: OrderedWriter,
    /// Faux tant que le fichier présent au démarrage n'a pas pu être lu : il ne doit pas être
    /// écrasé par la disposition par défaut.
    writable: AtomicBool,
    /// Widgets ajoutés par une migration au chargement, à annoncer (« Nouveautés »).
    news: Mutex<Vec<WidgetKind>>,
}

impl AppState {
    pub fn load() -> AppState {
        let layout_path = app_dir().join("layout.json");
        let monitors = horizon_sys::monitors::list();
        // Un fichier verrouillé un instant (antivirus, sauvegarde) : quelques essais.
        let mut loaded = config::load_layout(&layout_path, &monitors);
        for _ in 0..3 {
            if loaded.writable() {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
            loaded = config::load_layout(&layout_path, &monitors);
        }
        let state = AppState::new(layout_path, loaded);
        state.sync_monitors(&monitors);
        state
    }

    /// État à partir d'une disposition chargée (migrée ici).
    pub fn new(layout_path: PathBuf, loaded: LayoutLoad) -> AppState {
        let writable = loaded.writable();
        if let LayoutLoad::Unreadable(_, why) = &loaded {
            log::error!(
                "{} illisible ({why}) : disposition par défaut, le fichier ne sera pas écrasé",
                layout_path.display()
            );
        }
        let mut layout = loaded.into_layout();
        let news = layout.migrate();
        AppState {
            layout: Mutex::new(layout),
            undo: Mutex::new(UndoStack::default()),
            layout_path,
            renamed: Mutex::default(),
            writer: OrderedWriter::new(),
            writable: AtomicBool::new(writable),
            news: Mutex::new(news),
        }
    }

    /// Widgets ajoutés par les migrations du chargement (une seule fois).
    pub fn take_news(&self) -> Vec<WidgetKind> {
        std::mem::take(&mut *lock(&self.news))
    }

    /// Faux si `layout.json` était illisible au démarrage (il n'est pas écrasé).
    pub fn writable(&self) -> bool {
        self.writable.load(Ordering::SeqCst)
    }

    fn lock_layout(&self) -> std::sync::MutexGuard<'_, Layout> {
        lock(&self.layout)
    }

    /// Lit la disposition sous son verrou, relâché au retour de `f` (ne rien y attendre).
    pub fn read<T>(&self, f: impl FnOnce(&Layout) -> T) -> T {
        f(&self.lock_layout())
    }

    /// Copie de la disposition.
    pub fn snapshot(&self) -> Layout {
        self.read(Layout::clone)
    }

    /// Vide l'historique d'annulation (changement de profil : il parle d'autres barres).
    pub fn clear_undo(&self) {
        *lock(&self.undo) = UndoStack::default();
    }

    /// Modifie la disposition puis l'enregistre, verrou relâché. Renvoie la valeur de `f` et
    /// une copie de la disposition modifiée.
    pub fn update<T>(&self, f: impl FnOnce(&mut Layout) -> anyhow::Result<T>) -> anyhow::Result<(T, Layout)> {
        let (out, version, snapshot) = {
            let mut l = self.lock_layout();
            let out = f(&mut l)?;
            (out, self.writer.ticket(), l.clone())
        };
        self.persist(version, &snapshot);
        Ok((out, snapshot))
    }

    /// Modification du mode édition : l'état d'avant entre dans l'historique d'annulation.
    pub fn mutate(&self, f: impl FnOnce(&mut Layout) -> anyhow::Result<()>) -> anyhow::Result<()> {
        self.mutate_in(None, f)
    }

    /// Comme `mutate` ; les modifications successives d'un même `group` (frappes dans la
    /// fenêtre de réglages d'un widget) ne font qu'un état d'annulation.
    pub fn mutate_in(&self, group: Option<&str>, f: impl FnOnce(&mut Layout) -> anyhow::Result<()>) -> anyhow::Result<()> {
        self.update(|l| {
            let before = l.clone();
            f(l)?;
            lock(&self.undo).push(before, group);
            Ok(())
        })
        .map(|_| ())
    }

    /// Termine le groupe d'annulation en cours (nouvelle ouverture des réglages d'un widget).
    pub fn end_undo_group(&self) {
        lock(&self.undo).end_group();
    }

    /// Reprend l'état précédent ; faux si l'historique est vide. L'état repris peut dater d'avant
    /// un branchement d'écran : chaque écran de `monitors` garde sa barre.
    pub fn undo(&self, monitors: &[MonitorInfo]) -> bool {
        self.travel(monitors, UndoStack::undo)
    }

    /// Rétablit l'état annulé en dernier ; faux s'il n'y en a pas.
    pub fn redo(&self, monitors: &[MonitorInfo]) -> bool {
        self.travel(monitors, UndoStack::redo)
    }

    /// Remplace la disposition par l'état que donne `step` (verrous pris dans l'ordre :
    /// disposition, puis historique).
    fn travel(&self, monitors: &[MonitorInfo], step: fn(&mut UndoStack, Layout) -> Option<Layout>) -> bool {
        let renamed = self.update(|l| {
            let Some(other) = step(&mut lock(&self.undo), l.clone()) else {
                return Ok(None);
            };
            *l = other;
            let renamed = l.ensure_monitors(monitors);
            crate::widgets::shortcuts::fill_defaults(l);
            Ok(Some(renamed))
        });
        match renamed {
            Ok((Some(renamed), _)) => {
                lock(&self.renamed).extend(renamed);
                true
            }
            _ => false,
        }
    }

    /// Garantit une barre par écran branché (et met à jour identifiants et positions).
    pub fn sync_monitors(&self, monitors: &[MonitorInfo]) {
        let renamed = self.update(|l| {
            let renamed = l.ensure_monitors(monitors);
            crate::widgets::shortcuts::fill_defaults(l);
            Ok(renamed)
        });
        if let Ok((renamed, _)) = renamed {
            lock(&self.renamed).extend(renamed);
        }
    }

    /// Écrans renommés depuis le dernier appel.
    pub fn take_renamed(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *lock(&self.renamed))
    }

    /// Écrit la version `version` de la disposition, sauf si une plus récente l'a déjà été, ou
    /// si le fichier du démarrage était illisible (il est relu à chaque essai : absent ou mis
    /// de côté, l'écriture reprend).
    fn persist(&self, version: u64, l: &Layout) {
        if !self.writable.load(Ordering::SeqCst) {
            match config::load_layout(&self.layout_path, &[]) {
                LayoutLoad::Default(_) => {
                    log::info!("{} peut de nouveau être écrit", self.layout_path.display());
                    self.writable.store(true, Ordering::SeqCst);
                }
                LayoutLoad::Loaded(_) => {
                    log::warn!(
                        "{} est de nouveau lisible : il n'est pas écrasé ; relancer Horizon pour le reprendre",
                        self.layout_path.display()
                    );
                    return;
                }
                LayoutLoad::Unreadable(..) => {
                    log::warn!("{} toujours illisible : modification non enregistrée", self.layout_path.display());
                    return;
                }
            }
        }
        let written = serde_json::to_string_pretty(l)
            .map_err(std::io::Error::other)
            .and_then(|text| self.writer.write(&self.layout_path, version, text.as_bytes()));
        if let Err(e) = written {
            log::error!("enregistrement de la disposition : {e}");
        }
    }

    /// Écran de la barre principale (rôle `Main`).
    pub fn main_monitor(&self) -> Option<String> {
        self.read(|l| {
            l.bars.iter().find(|b| b.role == horizon_core::layout::BarRole::Main).map(|b| b.monitor_id.clone())
        })
    }

    pub fn bar(&self, monitor_id: &str) -> Option<BarLayout> {
        self.read(|l| l.bars.iter().find(|b| b.monitor_id == monitor_id).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::geometry::Rect;
    use horizon_core::layout::{WidgetKind, Zone};

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("horizon-app-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mon(id: &str, x: i32, primary: bool) -> MonitorInfo {
        MonitorInfo {
            id: id.into(),
            device_name: id.into(),
            rect: Rect { x, y: 0, w: 1920, h: 1080 },
            work: Rect { x, y: 0, w: 1920, h: 1040 },
            scale: 1.0,
            primary,
            portrait: false,
        }
    }

    fn on_disk(p: &std::path::Path) -> Layout {
        config::load_layout(p, &[]).into_layout()
    }

    /// Premier widget de la zone gauche de la première barre.
    fn first_left(l: &Layout) -> (String, String) {
        (l.bars[0].left[0].id.clone(), l.bars[0].monitor_id.clone())
    }

    #[test]
    fn mutate_saves_and_undo_restores() {
        let d = Dir::new("mutate");
        let p = d.0.join("layout.json");
        let monitors = [mon("a", 0, true)];
        let s = AppState::new(p.clone(), config::load_layout(&p, &monitors));
        s.sync_monitors(&monitors);
        let start = s.lock_layout().clone();
        let (id, monitor) = first_left(&start);
        s.mutate(|l| l.move_widget(&id, &monitor, Zone::Right, 0)).unwrap();
        assert_eq!(s.lock_layout().bars[0].right[0].id, id);
        assert_eq!(on_disk(&p), *s.lock_layout(), "modification enregistrée");
        assert!(s.undo(&monitors));
        assert_eq!(*s.lock_layout(), start);
        assert_eq!(on_disk(&p), start, "annulation enregistrée");
        assert!(!s.undo(&monitors), "historique vide");
        assert!(s.redo(&monitors));
        assert_eq!(s.lock_layout().bars[0].right[0].id, id, "rétabli");
        assert!(!s.redo(&monitors));
    }

    #[test]
    fn failed_change_is_neither_applied_nor_kept_for_undo() {
        let d = Dir::new("failed");
        let p = d.0.join("layout.json");
        let monitors = [mon("a", 0, true)];
        let s = AppState::new(p.clone(), config::load_layout(&p, &monitors));
        let before = s.lock_layout().clone();
        assert!(s.mutate(|l| l.move_widget("inconnu", "a", Zone::Left, 0)).is_err());
        assert_eq!(*s.lock_layout(), before);
        assert!(!s.undo(&monitors));
    }

    #[test]
    fn undo_after_plugging_a_screen_keeps_its_bar() {
        let d = Dir::new("plug");
        let p = d.0.join("layout.json");
        let one = [mon("a", 0, true)];
        let s = AppState::new(p.clone(), config::load_layout(&p, &one));
        s.sync_monitors(&one);
        let (id, monitor) = first_left(&s.lock_layout());
        s.mutate(|l| l.move_widget(&id, &monitor, Zone::Right, 0)).unwrap();
        let two = [mon("a", 0, true), mon("b", 1920, false)];
        s.sync_monitors(&two);
        assert!(s.undo(&two));
        let l = s.lock_layout();
        assert!(l.bars.iter().any(|b| b.monitor_id == "b"), "la barre de l'écran branché reste");
        assert_eq!(l.bars.iter().find(|b| b.monitor_id == "a").unwrap().left[0].id, id);
    }

    #[test]
    fn unreadable_file_is_never_overwritten() {
        let d = Dir::new("unreadable");
        let p = d.0.join("layout.json");
        // Un dossier à la place du fichier : lecture impossible.
        std::fs::create_dir_all(p.join("x")).unwrap();
        let monitors = [mon("a", 0, true)];
        let s = AppState::new(p.clone(), config::load_layout(&p, &monitors));
        s.sync_monitors(&monitors);
        let (id, monitor) = first_left(&s.lock_layout());
        s.mutate(|l| l.move_widget(&id, &monitor, Zone::Right, 0)).unwrap();
        assert!(p.join("x").is_dir(), "rien n'a été écrit à la place");
        // Une fois l'obstacle levé (fichier absent), l'écriture reprend.
        std::fs::remove_dir_all(&p).unwrap();
        s.mutate(|l| l.add_widget(&monitor, Zone::Left, WidgetKind::CLOCK).map(|_| ())).unwrap();
        assert_eq!(on_disk(&p), *s.lock_layout());
    }

    #[test]
    fn settings_are_written_never_over_an_unreadable_file() {
        let d = Dir::new("settings");
        let p = d.0.join("settings.toml");
        let mut s = Settings::default();
        s.features.osd = !s.features.osd;
        save_settings_to(&s, &p).unwrap();
        assert!(matches!(Settings::inspect(&p).unwrap(), SettingsFile::Valid(x) if x == s), "écrits");
        std::fs::write(&p, "pas = [du toml").unwrap();
        assert!(save_settings_to(&s, &p).is_err(), "fichier illisible jamais écrasé");
    }

    #[test]
    fn an_older_version_is_never_written_over_a_newer_one() {
        let d = Dir::new("order");
        let p = d.0.join("layout.json");
        let monitors = [mon("a", 0, true)];
        let s = AppState::new(p.clone(), config::load_layout(&p, &monitors));
        let (_, v1) = s.update(|l| l.set_style("a", horizon_core::layout::BarStyle::Graphic)).unwrap();
        s.update(|l| l.set_style("a", horizon_core::layout::BarStyle::Islands)).unwrap();
        let newer = on_disk(&p);
        assert_ne!(newer, v1);
        // L'écriture de la version 1, en retard, arrive après celle de la version 2.
        s.persist(1, &v1);
        assert_eq!(on_disk(&p), newer);
    }
}
