//! Horizon : la barre en Rust + Slint (rendu logiciel, fenêtres layered), sans
//! WebView2. Mode d'emploi (ajouter un widget, différences avec l'API de Tauri) :
//! `natif/README.md`.
//!
//! Organisation, comme `app/src-tauri` : `platform` (fenêtres Win32 pour Slint), `host` (ce qui
//! imite Tauri), les modules de services repris de `app/src-tauri/src` (`state`, `bars`,
//! `autohide`…), `ui` (fenêtres Slint et leur liaison), `widgets` (catalogue).

pub mod alerts;
pub mod accent;
pub mod autohide;
pub mod bars;
pub mod brands;
pub mod comfort;
pub mod dnd;
pub mod drag;
pub mod edit;
pub mod events;
pub mod foreground;
pub mod game;
pub mod host;
pub mod instance;
pub mod journal;
pub mod mixer;
pub mod motion;
pub mod options;
pub mod osd;
pub mod overflow;
pub mod palette;
pub mod phone;
pub mod platform;
pub mod popup;
pub mod presence;
pub mod profiles;
pub mod prisme;
pub mod prisme_window;
pub mod relay;
pub mod resume;
pub mod settings_window;
pub mod state;
pub mod status;
pub mod tiles;
pub mod timer;
pub mod tray;
pub mod ui;
pub mod undoable;
pub mod plugins;
pub mod update;
pub mod util;
pub mod web;
pub mod widgets;
pub mod wsclient;

use host::{AppHandle, Manager};

/// Édition personnelle (fonction Cargo `personal` : intégrations aux applis de l'auteur, Prisme,
/// SharkApp, BetterDiscord) ou publique (par défaut).
pub const PERSONAL: bool = cfg!(feature = "personal");

/// Démarrage d'une fonction du cœur (fils, fenêtres, connexions).
type Start = fn(&AppHandle) -> anyhow::Result<()>;

/// Dans l'ordre (comme `STARTS_BEFORE` / `STARTS_AFTER` de la version Tauri) : la reprise après
/// veille, les barres, ce qui les suit, les widgets (`widgets::start_all`), puis le premier plan
/// après ses abonnés.
const STARTS: &[(&str, Start)] = &[
    ("reprise après veille", resume::start),
    ("présence (verrouillage, écran éteint)", presence::start),
    // Avant les barres : elles prennent l'accent et la pulsation à leur ouverture.
    ("accent des fenêtres", ui::accent::start),
    ("pulsation des barres", ui::pulse::start),
    ("barres", bars::start),
    ("panneau surgissant", ui::popup::start),
    ("tuiles (essai)", tiles::essai),
    ("mixeur audio", mixer::start),
    ("masquage en plein écran", autohide::start),
    ("Prisme", prisme::start),
    ("couleur du fond d'écran", accent::start),
    ("affichage du volume", osd::start),
    ("icône de notification", tray::start),
    ("palette de commandes", palette::start),
    ("relais Discord", relay::start),
    ("mode nuit", comfort::start),
    ("ne pas déranger", dnd::start),
    ("minuteur", timer::start),
    ("appels du téléphone", phone::start),
    ("plugins", plugins::start),
    ("réglages", options::start),
    ("mise à jour", update::start),
    // Abonné du premier plan (démarré après les widgets).
    ("profils", profiles::start),
];
/// Après les widgets : le premier plan après ses abonnés, l'annonce des nouveautés et l'état
/// des intégrations (qui lit celui des widgets).
const STARTS_AFTER: &[(&str, Start)] =
    &[("premier plan", foreground::start), ("nouveautés", alerts::announce_news), ("état des intégrations", status::start)];

/// Démarrages du cœur réservés à l'édition personnelle.
const PERSONAL_STARTS: [&str; 3] = ["Prisme", "relais Discord", "appels du téléphone"];

/// Affiche les notifications lues pendant 60 s (diagnostic de l'identité de paquet,
/// `horizon.exe --notif-probe` depuis PowerShell).
pub fn notif_probe() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let _watcher = match horizon_sys::notifications::start(tx) {
        Ok(w) => w,
        Err(e) => return println!("démarrage impossible : {e:#}"),
    };
    let end = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
        if let Ok(ev) = rx.recv_timeout(left) {
            println!("{ev:?}");
        }
    }
}

/// Sortie de la console qui a lancé Horizon (programme sans console à lui : les lignes
/// écrites par `--check-plugin` y apparaissent).
pub fn attach_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY : sans argument pointeur ; échoue sans effet s'il n'y a pas de console parente.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

pub fn run() {
    journal::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Une panique (panic = "abort" en release) ne doit pas laisser la place réservée en haut
    // des écrans : on la libère, et on garde le message dans le journal.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panique : {info}");
        horizon_sys::appbar::release_all();
        // L'affichage du volume de Windows revient.
        osd::exit();
        default_hook(info);
    }));
    // Lancée par une mise à jour : la version d'avant se ferme d'abord.
    update::wait_previous();
    // Une seule instance ; `--quit` ferme celle qui tourne (ou ne fait rien).
    let _instance = match instance::acquire(&args) {
        Ok(Some(guard)) if !args.iter().any(|a| a == "--quit") => guard,
        Ok(_) => return,
        Err(e) => {
            log::error!("instance unique : {e:#}");
            return;
        }
    };
    log::info!("démarrage d'Horizon {}", env!("CARGO_PKG_VERSION"));
    // Une version téléchargée attend : elle est installée et lancée à la place de celle-ci.
    if update::apply_at_start() {
        return;
    }
    // Windows relance Horizon s'il plante dans du code natif (hors gestionnaire de panique).
    horizon_sys::restart::relaunch_after_crash();
    if let Err(e) = platform::install() {
        log::error!("{e:#}");
        return;
    }
    let app = AppHandle::new();
    app.manage(bars::Bars::default());
    app.manage(autohide::Autohide::default());
    app.manage(comfort::Night::default());
    app.manage(state::AppState::load());
    app.manage(foreground::ForegroundHub::default());
    app.manage(prisme::Prisme::default());
    app.manage(relay::Relay::default());
    app.manage(popup::PopupState::default());
    app.manage(mixer::Mixer::default());
    app.manage(dnd::DndHub::default());
    app.manage(timer::TimerHub::default());
    app.manage(phone::PhoneState::default());
    app.manage(status::Health::default());
    app.manage(accent::Accent::default());
    app.manage(palette::Palette::default());
    app.manage(update::Updater::default());
    app.manage(plugins::Plugins::default());
    widgets::manage_all(&app);
    if let Err(e) = instance::listen(&app) {
        log::error!("écoute des lancements suivants : {e:#}");
    }
    let start_one = |name: &str, start: Start| {
        if let Err(e) = start(&app) {
            log::error!("{name} : démarrage impossible : {e:#}");
            app.state::<status::Health>().start_failed(name, format!("{e:#}"));
        }
    };
    for (name, start) in STARTS {
        if PERSONAL || !PERSONAL_STARTS.contains(name) {
            start_one(name, *start);
        }
    }
    widgets::start_all(&app);
    for (name, start) in STARTS_AFTER {
        start_one(name, *start);
    }
    // Interface de Prisme demandée au lancement (menu de l'icône de Prisme).
    if args.iter().any(|a| a == prisme_window::ARG) {
        prisme_window::open(&app);
    }
    if args.iter().any(|a| a == "--essai")
        && let Err(e) = ui::essai::open()
    {
        log::error!("fenêtres d'essai : {e:#}");
    }
    if let Err(e) = slint::run_event_loop_until_quit() {
        log::error!("boucle d'événements : {e}");
    }
    // Place réservée rendue, icône retirée.
    widgets::exit_all(&app);
    // Les scripts des plugins disent au revoir (500 ms au plus).
    plugins::script::quit();
    bars::close_all(&app);
    ui::popup::stop();
    ui::tooltip::close();
    tray::stop();
    // L'affichage du volume de Windows revient.
    osd::exit();
    // Prisme reprend sa palette (et sa scène, si un signal ou « Ne pas déranger » la tient) ; on
    // laisse le temps aux messages de partir.
    if PERSONAL {
        let p = app.state::<prisme::Prisme>();
        p.set_focus_hold(false);
        p.end_signal();
        p.set_accent(None);
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    log::info!("arrêt d'Horizon");
}
