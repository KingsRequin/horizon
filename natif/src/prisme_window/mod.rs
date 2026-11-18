//! Fenêtre « Prisme » : l'interface complète du moteur RGB (pages Lumière, Disposition, Effets,
//! Automatique, Réglages), reprise de `app/src/apps/prisme/` (Svelte) et de
//! `app/src-tauri/src/prisme_window.rs`. Ouverte à la demande (panneau Éclairage, `--prisme` :
//! menu de l'icône de Prisme) ; une seule à la fois (rouvrir la ramène au premier plan) ;
//! détruite à la fermeture (connexion fermée, rien ne reste en mémoire).
//!
//! Comme la page Tauri, la fenêtre parle au moteur directement (`ws://127.0.0.1:28650`, `link.rs`)
//! : état et appareils poussés, prévisualisation des LED pendant qu'une toile est montrée,
//! requêtes (`calls.rs`). La config se modifie dans un brouillon enregistré seul (`config.rs`).
//!
//! Tout vit sur le fil de l'interface (`CTX`) ; le fil de la connexion n'y dépose que des
//! messages décodés.

mod calls;
mod color;
mod config;
mod effects;
mod layout;
mod link;
mod now;
mod plan;
mod render;
mod rules;
mod scenes;
mod screens;
mod settings;
mod types;

use crate::host::AppHandle;
use crate::platform::{self, Frame, WindowSpec};
use crate::ui::{PrismeCanvas, PrismeLabel, PrismeShell, PrismeTheme, PrismeWindow, model};
use calls::{Calls, Immediate, Outbox};
use config::{ConfigStore, Status};
use link::{Incoming, Link};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};
use types::{DaemonState, DeviceStatus, Preview};

/// Étiquette de la fenêtre (une seule à la fois).
pub const LABEL: &str = "prisme";
/// Argument de ligne de commande qui ouvre la fenêtre (premier lancement ou instance en cours).
pub const ARG: &str = "--prisme";
/// Taille logique (celle de la version Tauri) et minimum.
const SIZE: (f32, f32) = (1180.0, 760.0);

/// Page montrée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Now,
    Layout,
    Scenes,
    Rules,
    Settings,
}

impl Page {
    fn parse(s: &str) -> Page {
        match s {
            "layout" => Page::Layout,
            "scenes" => Page::Scenes,
            "rules" => Page::Rules,
            "settings" => Page::Settings,
            _ => Page::Now,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Page::Now => "now",
            Page::Layout => "layout",
            Page::Scenes => "scenes",
            Page::Rules => "rules",
            Page::Settings => "settings",
        }
    }
}

/// État de la fenêtre ouverte.
pub(crate) struct Ctx {
    win: PrismeWindow,
    /// Numéro de l'ouverture (les messages d'une connexion précédente sont ignorés).
    generation: u64,
    link: Option<Link>,
    calls: Calls,
    connected: bool,
    state: Option<DaemonState>,
    devices: Vec<DeviceStatus>,
    preview: Option<Preview>,
    /// Prévisualisation demandée au moteur.
    preview_on: bool,
    config: ConfigStore,
    page: Page,
    starting: bool,
    start_error: String,
    /// Taille de la toile montrée (logique) ; échelle relue au dessin.
    canvas: (f32, f32),
    /// Accent lissé de l'interface (couleur dominante des LED, page Lumière).
    live: [f64; 3],
    labels: Rc<VecModel<PrismeLabel>>,
    now: now::State,
    scenes: scenes::State,
    rules: rules::State,
    settings: settings::State,
    layout: layout::State,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// Envoi d'une modification de la config après la pause (`config::DEBOUNCE`).
    static SAVE: slint::Timer = slint::Timer::default();
    /// Effacement du message « enregistré ».
    static SAVED: slint::Timer = slint::Timer::default();
    /// Fin de « Démarrage… » (3 s).
    static STARTING: slint::Timer = slint::Timer::default();
}

/// Agit sur la fenêtre ouverte (rien si elle est fermée ou déjà occupée).
fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
    CTX.with(|c| {
        let mut b = c.try_borrow_mut().ok()?;
        b.as_mut().map(f)
    })
}

/// Comme `with`, différé au tour suivant de la boucle si l'état est déjà emprunté (rappel de
/// Slint déclenché pendant une mise à jour).
fn act(f: impl FnOnce(&mut Ctx) + 'static) {
    let busy = CTX.with(|c| c.try_borrow_mut().is_err());
    if busy {
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(f);
        });
    } else {
        with(f);
    }
}

/// Ouvre (ou ramène) la fenêtre.
pub fn open(app: &AppHandle) {
    if !crate::PERSONAL {
        return;
    }
    let app = app.clone();
    // Hors du rappel en cours (clic du panneau, message d'un second lancement).
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Err(e) = show(&app) {
            log::error!("ouverture de Prisme : {e:#}");
        }
    });
}

/// La fenêtre est-elle ouverte ?
pub fn is_open() -> bool {
    CTX.with(|c| c.borrow().is_some())
}

fn bring_back(c: &Ctx) {
    if let Some(h) = platform::hwnd(c.win.window()) {
        horizon_sys::window_focus::bring_to_front(h);
    }
}

/// Zone cliente : centrée dans la zone de travail de l'écran de la souris, à son échelle
/// (`center_on` de la version Tauri), bornée à cette zone.
fn placement() -> Option<(i32, i32, u32, u32)> {
    let monitors = horizon_sys::monitors::list();
    let (x, y) = horizon_sys::fullscreen::cursor().unwrap_or((0, 0));
    let inside = |m: &&horizon_core::geometry::MonitorInfo| {
        (m.rect.x..m.rect.x + m.rect.w).contains(&x) && (m.rect.y..m.rect.y + m.rect.h).contains(&y)
    };
    let m = monitors.iter().find(inside).or_else(|| monitors.iter().find(|m| m.primary))?;
    let scale = if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 };
    let pw = ((SIZE.0 * scale) as i32).min(m.work.w.max(1));
    let ph = ((SIZE.1 * scale) as i32).min(m.work.h.max(1));
    Some((m.work.x + (m.work.w - pw) / 2, m.work.y + (m.work.h - ph) / 2, pw as u32, ph as u32))
}

fn show(app: &AppHandle) -> anyhow::Result<()> {
    if with(|c| bring_back(c)).is_some() {
        return Ok(());
    }
    let win = PrismeWindow::new()?;
    let hwnd = platform::configure(
        win.window(),
        WindowSpec { title: "Prisme".into(), frame: Frame::Normal, rect: placement(), size: SIZE, focusable: true, topmost: false, taskbar: true },
    )?;
    crate::ui::accent::follow(&win);
    win.global::<crate::ui::Tip>().on_show(move |text| crate::ui::tooltip::show(hwnd, &text));
    win.global::<crate::ui::Tip>().on_hide(crate::ui::tooltip::hide);
    let labels = Rc::new(VecModel::default());
    win.global::<PrismeCanvas>().set_labels(ModelRc::from(labels.clone()));
    bind(&win);
    now::bind(&win);
    scenes::bind(&win);
    rules::bind(&win);
    settings::bind(&win);
    layout::bind(&win);
    let a = app.clone();
    win.window().on_close_requested(move || {
        let a = a.clone();
        slint::Timer::single_shot(Duration::ZERO, move || close(&a));
        slint::CloseRequestResponse::HideWindow
    });
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let link = Link::spawn(move |m| {
        let _ = slint::invoke_from_event_loop(move || incoming(generation, m));
    });
    CTX.with(|c| {
        *c.borrow_mut() = Some(Ctx {
            win: win.clone_strong(),
            generation,
            link: Some(link),
            calls: Calls::new(),
            connected: false,
            state: None,
            devices: Vec::new(),
            preview: None,
            preview_on: false,
            config: ConfigStore::default(),
            page: Page::Now,
            starting: false,
            start_error: String::new(),
            canvas: (0.0, 0.0),
            live: [170.0, 182.0, 255.0],
            labels,
            now: now::State::default(),
            scenes: scenes::State::default(),
            rules: rules::State::default(),
            settings: settings::State::default(),
            layout: layout::State::default(),
        })
    });
    with(refresh);
    win.show()?;
    app.register_window(LABEL, hwnd);
    log::info!("fenêtre Prisme ouverte");
    Ok(())
}

/// Fermeture : connexion arrêtée (le moteur coupe sa prévisualisation), fenêtre détruite.
fn close(app: &AppHandle) {
    SAVE.with(slint::Timer::stop);
    SAVED.with(slint::Timer::stop);
    crate::ui::tooltip::hide();
    // Une modification encore dans sa pause d'envoi est perdue, comme quand la page Tauri est
    // détruite.
    let ctx = CTX.with(|c| c.borrow_mut().take());
    drop(ctx);
    app.unregister_window(LABEL);
    log::info!("fenêtre Prisme fermée");
}

/// Envoi vers la connexion (rien hors connexion).
struct LinkOut<'a>(Option<&'a Link>);

impl Outbox for LinkOut<'_> {
    fn send(&self, msg: String) {
        if let Some(l) = self.0 {
            l.send(msg);
        }
    }
}

/// Requête au moteur ; `reply` reçoit sa réponse (toujours plus tard, jamais pendant l'appel).
fn call(cmd: &str, args: Value, reply: impl FnOnce(Result<Value, String>) + 'static) {
    let cmd = cmd.to_owned();
    let imm = with(move |c| {
        let out = LinkOut(c.link.as_ref());
        c.calls.call(&out, c.connected, &cmd, args, Box::new(reply))
    })
    .flatten();
    if let Some(i) = imm {
        slint::Timer::single_shot(Duration::ZERO, move || i.run());
    }
}

fn incoming(generation: u64, m: Incoming) {
    let mine = with(|c| c.generation == generation).unwrap_or(false);
    if !mine {
        return;
    }
    match m {
        Incoming::Connected => {
            with(|c| {
                c.connected = true;
                c.preview_on = false;
                sync_preview(c);
                refresh(c);
            });
            // Config rechargée à chaque (re)connexion au moteur.
            load_config();
        }
        Incoming::Disconnected => {
            let failed = with(|c| {
                c.connected = false;
                c.preview_on = false;
                c.preview = None;
                refresh(c);
                c.calls.fail_all()
            })
            .unwrap_or_default();
            failed.into_iter().for_each(Immediate::run);
        }
        Incoming::State(st, devices) => {
            let reload = with(|c| {
                let reload = c.config.should_reload(st.config_rev, SAVE.with(slint::Timer::running));
                let changed = c.state.as_ref() != Some(&*st) || c.devices != devices;
                c.state = Some(*st);
                c.devices = devices;
                if changed {
                    now::on_state(c);
                    refresh(c);
                }
                reload
            });
            if reload == Some(true) {
                load_config();
            }
        }
        Incoming::Preview(p) => {
            with(|c| {
                if c.preview.as_ref() != Some(&*p) {
                    c.preview = Some(*p);
                    redraw(c);
                }
            });
        }
        Incoming::Reply { id, result } => {
            if let Some(reply) = with(|c| c.calls.answered(id)).flatten() {
                reply(result);
            }
        }
    }
}

/// Recharge le brouillon depuis le moteur (`config.load()`).
fn load_config() {
    call("get_config", json!({}), |r| {
        with(|c| {
            match r {
                Ok(v) => {
                    let rev = v.get("rev").and_then(Value::as_u64).unwrap_or(0);
                    let json = v.get("json").cloned().unwrap_or(Value::Null);
                    c.config.loaded(json, rev);
                }
                Err(e) => c.config.failed(e),
            }
            refresh(c);
        });
    });
}

/// Le brouillon vient de changer : envoi après la pause, page mise à jour.
pub(crate) fn edited(c: &mut Ctx) {
    SAVE.with(|t| t.start(slint::TimerMode::SingleShot, config::DEBOUNCE, persist));
    refresh(c);
}

/// Envoie le brouillon s'il diffère de ce qui est enregistré.
fn persist() {
    let Some(Some(json)) = with(|c| c.config.pending()) else { return };
    with(|c| {
        c.config.saving();
        refresh_shell(c);
    });
    let Ok(config) = serde_json::from_str::<Value>(&json) else { return };
    call("set_config_json", json!({ "config": config }), move |r| {
        with(|c| {
            match r {
                Ok(v) => c.config.saved(json, v.get("rev").and_then(Value::as_u64), Instant::now()),
                // Brouillon conservé : l'utilisateur corrige, et l'enregistrement repart.
                Err(e) => c.config.failed(e),
            }
            refresh(c);
        });
        SAVED.with(|t| {
            t.start(slint::TimerMode::SingleShot, config::SAVED_FOR, || {
                with(|c| {
                    c.config.saved_expired();
                    refresh_shell(c);
                });
            })
        });
    });
}

/// Ctrl+Z, « Annuler la dernière modification ».
fn undo(c: &mut Ctx) {
    if c.config.undo() {
        edited(c);
    } else {
        refresh_shell(c);
    }
}

fn bind(win: &PrismeWindow) {
    let shell = win.global::<PrismeShell>();
    shell.on_go(|key| {
        act(move |c| {
            c.page = Page::parse(&key);
            c.canvas = (0.0, 0.0);
            refresh(c);
        })
    });
    shell.on_undo(|| act(undo));
    shell.on_start_daemon(|| act(start_daemon));
    let canvas = win.global::<PrismeCanvas>();
    canvas.on_resized(|w, h| {
        act(move |c| {
            if c.canvas != (w, h) {
                c.canvas = (w, h);
                redraw(c);
            }
        })
    });
    canvas.on_pointer(|kind, x, y, alt| act(move |c| layout::pointer(c, kind, x as f64, y as f64, alt)));
    canvas.on_key(|text, shift| {
        let mut taken = false;
        // Le clavier du plan ne peut pas attendre : il dit tout de suite s'il prend la touche.
        with(|c| taken = layout::key(c, &text, shift));
        taken
    });
}

/// `prisme_start_daemon` : démarre le moteur installé (`%LOCALAPPDATA%\Programs\Prisme`).
fn start_daemon(c: &mut Ctx) {
    c.starting = true;
    c.start_error = match launch_daemon() {
        Ok(()) => String::new(),
        Err(e) => e,
    };
    STARTING.with(|t| {
        t.start(slint::TimerMode::SingleShot, Duration::from_secs(3), || {
            with(|c| {
                c.starting = false;
                refresh_shell(c);
            });
        })
    });
    refresh_shell(c);
}

fn launch_daemon() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let base = std::env::var_os("LOCALAPPDATA").ok_or("Prisme n'est pas installé (prisme.exe introuvable)")?;
    let exe = std::path::Path::new(&base).join("Programs").join("Prisme").join("prisme.exe");
    if !exe.is_file() {
        return Err("Prisme n'est pas installé (prisme.exe introuvable)".into());
    }
    std::process::Command::new(&exe)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("lancement de {} impossible : {e}", exe.display()))
}

/// La page en cours montre-t-elle une toile (et donc la prévisualisation) ?
fn wants_preview(c: &Ctx) -> bool {
    c.connected
        && match c.page {
            Page::Now => true,
            Page::Scenes => scenes::has_selection(c),
            Page::Layout => c.config.draft().is_some(),
            Page::Rules | Page::Settings => false,
        }
}

/// Abonnement à la prévisualisation selon la page (`onPreview` de daemon.svelte.ts).
fn sync_preview(c: &mut Ctx) {
    let want = wants_preview(c);
    if want != c.preview_on {
        c.preview_on = want;
        if let Some(l) = &c.link {
            l.send(json!({ "cmd": "preview", "enabled": want }).to_string());
        }
    }
}

fn refresh_shell(c: &Ctx) {
    let s = c.win.global::<PrismeShell>();
    s.set_page(c.page.key().into());
    s.set_connected(c.connected);
    s.set_starting(c.starting);
    s.set_start_error(c.start_error.as_str().into());
    let (state, error) = match &c.config.status {
        Status::Idle => (0, String::new()),
        Status::Saving => (1, String::new()),
        Status::Saved => (2, String::new()),
        Status::Error(e) => (3, e.clone()),
    };
    s.set_save_state(state);
    s.set_save_error(error.into());
    s.set_can_undo(c.config.can_undo());
}

/// Tout ce qui se voit, d'après l'état.
pub(crate) fn refresh(c: &mut Ctx) {
    refresh_shell(c);
    match c.page {
        Page::Now => now::refresh(c),
        Page::Scenes => scenes::refresh(c),
        Page::Rules => rules::refresh(c),
        Page::Settings => settings::refresh(c),
        Page::Layout => layout::refresh(c),
    }
    sync_preview(c);
    redraw(c);
}

/// Redessine la toile de la page (rien si la page n'en a pas ou qu'elle n'a pas encore de taille).
pub(crate) fn redraw(c: &mut Ctx) {
    let scale = platform::scale(c.win.window()).unwrap_or(1.0);
    let size = render::Size { w: c.canvas.0, h: c.canvas.1, scale };
    if !size.usable() {
        return;
    }
    let frame = match c.page {
        Page::Now => {
            let Some((f, info)) = render::stage(size, [0x1a, 0x1d, 0x24], c.preview.as_ref(), None) else { return };
            c.win.global::<PrismeCanvas>().set_empty(info.leds == 0);
            if let Some(d) = info.dominant {
                let (smoothed, shown) = render::live_accent(c.live, d, 15.0);
                c.live = smoothed;
                c.win.global::<PrismeTheme>().set_live(slint::Color::from_rgb_u8(shown[0], shown[1], shown[2]));
            }
            f
        }
        Page::Scenes if scenes::has_selection(c) => {
            let Some((f, info)) = render::stage(size, [0x20, 0x24, 0x2c], c.preview.as_ref(), None) else { return };
            c.win.global::<PrismeCanvas>().set_empty(info.leds == 0);
            f
        }
        Page::Layout => match layout::frame(c, size) {
            Some(f) => f,
            None => return,
        },
        _ => return,
    };
    let canvas = c.win.global::<PrismeCanvas>();
    canvas.set_image(slint::Image::from_rgba8(frame.pixels));
    let labels = frame
        .labels
        .into_iter()
        .map(|l| PrismeLabel { x: l.x, y: l.y, text: l.text.into(), alpha: l.alpha, pill: l.pill })
        .collect();
    model::sync(&c.labels, labels);
}

/// Liste déroulante : menu Windows sous l'élément (`x`, `y` : coin bas-gauche, logique) ;
/// `choices` : (clé, libellé), `current` cochée. `chosen` reçoit la clé.
pub(crate) fn select(c: &Ctx, x: f32, y: f32, choices: Vec<(String, String)>, current: &str, chosen: impl FnOnce(&mut Ctx, String) + 'static) {
    let Some(hwnd) = platform::hwnd(c.win.window()) else { return };
    let scale = platform::scale(c.win.window()).unwrap_or(1.0);
    let entries = choices.iter().map(|(k, l)| crate::ui::menu::Entry::check(k.clone(), l.clone(), k == current)).collect();
    let at = client_to_screen(hwnd, (x * scale) as i32, (y * scale) as i32);
    crate::ui::menu::show_at(hwnd, entries, true, Some(at), move |key| act(move |c| chosen(c, key)));
}

fn client_to_screen(hwnd: isize, x: i32, y: i32) -> (i32, i32) {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    let mut p = POINT { x, y };
    // SAFETY : fenêtre vivante de ce fil, point local.
    let _ = unsafe { ClientToScreen(HWND(hwnd as *mut _), &mut p) };
    (p.x, p.y)
}

/// Sélecteur de couleurs de Windows (`input[type=color]` du navigateur), hors du rappel en cours.
pub(crate) fn pick_color(c: &Ctx, initial: [u8; 3], chosen: impl FnOnce(&mut Ctx, [u8; 3]) + 'static) {
    let Some(hwnd) = platform::hwnd(c.win.window()) else { return };
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(rgb) = crate::ui::dialogs::choose_color(hwnd, initial) {
            act(move |c| chosen(c, rgb));
        }
    });
}

/// Met un modèle à jour d'après des lignes « simples » (comparées entre elles) : une ligne
/// inchangée garde son élément, sous-modèles compris (survol, glissement en cours).
pub(crate) fn sync_rows<P: PartialEq + Clone, T: Clone + 'static>(
    m: &VecModel<T>,
    prev: &mut Vec<P>,
    rows: Vec<P>,
    make: impl Fn(&P) -> T,
) {
    for (i, r) in rows.iter().enumerate() {
        if i < m.row_count() {
            if prev.get(i) != Some(r) {
                m.set_row_data(i, make(r));
            }
        } else {
            m.push(make(r));
        }
    }
    while m.row_count() > rows.len() {
        m.remove(m.row_count() - 1);
    }
    *prev = rows;
}

/// Modèle Slint à partir d'une liste.
pub(crate) fn list<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_round_trip() {
        for p in [Page::Now, Page::Layout, Page::Scenes, Page::Rules, Page::Settings] {
            assert_eq!(Page::parse(p.key()), p);
        }
        assert_eq!(Page::parse("inconnue"), Page::Now);
    }
}
