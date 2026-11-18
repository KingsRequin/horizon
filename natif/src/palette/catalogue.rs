//! Catalogue de la palette : entrées (raccourcis, suggestions, actions, profils, scènes,
//! sorties audio, serveurs, applis du menu Démarrer) et ce que fait chacune.
//!
//! Repris de `app/src-tauri/src/palette/catalogue.rs` ; différences : les icônes sont rendues par
//! `ui::palette`, et pendant un essai (`--palette-essai`) les actions sont seulement journalisées.

use super::Palette;
use crate::alerts::{Alert, Severity};
use crate::state::AppState;
use horizon_core::dnd::AlertKind;
use horizon_core::layout::WidgetKind;
use horizon_core::shortcuts::{Shortcut, ShortcutIcon};
use crate::host::{AppHandle, Manager};


/// Ce que fait une entrée.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Action {
    Launch(String),
    Reveal(String),
    Suggestion(String),
    Profile(String),
    Scene(Option<String>),
    LightToggle,
    Mic,
    Krisp,
    NextOutput,
    Output(String),
    SettingsFile,
    /// Fenêtre « Réglages d'Horizon », sur une section.
    Options(&'static str),
    Profiles,
    Edit,
    Quit,
    Ssh(String),
    Url(String),
    Dnd,
    TimerStart(u32),
    Pomodoro,
    TimerPause(bool),
    TimerStop,
    TimerExtend(u32),
    PlayPause,
    NextTrack,
    PrevTrack,
    /// Micro (`mute`) ou son (`deaf`) de Discord.
    Voice(&'static str),
    LeaveCall,
    ClaudeFocus(String),
    ClaudeForget(String),
    Reload,
    /// Fenêtre ramenée sur l'écran de la palette, puis au premier plan.
    BringWindow(isize),
    /// Fenêtre au premier plan sans la déplacer.
    FocusWindow(isize),
}

/// Icône d'une entrée, rendue par la fenêtre (`ui::palette`).
#[derive(Debug, Clone, PartialEq)]
pub enum IconRef {
    /// Icône d'interface (`lib/icons.ts`).
    Glyph { name: &'static str },
    /// Comme un raccourci de la barre (logo, image, icône d'exe ou de `.lnk`).
    Shortcut { item: Shortcut },
    Suggestion { key: String },
}

pub(super) struct Entry {
    pub(super) id: String,
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) keywords: String,
    pub(super) icon: IconRef,
    /// La première est l'action principale (Entrée), les autres se choisissent avec Tab.
    pub(super) actions: Vec<(String, Action)>,
}

impl Entry {
    fn new(id: impl Into<String>, title: impl Into<String>, subtitle: impl Into<String>, icon: IconRef) -> Entry {
        Entry { id: id.into(), title: title.into(), subtitle: subtitle.into(), keywords: String::new(), icon, actions: Vec::new() }
    }

    fn keywords(mut self, k: impl Into<String>) -> Entry {
        self.keywords = k.into();
        self
    }

    fn action(mut self, label: &str, a: Action) -> Entry {
        self.actions.push((label.to_owned(), a));
        self
    }
}

fn glyph(name: &'static str) -> IconRef {
    IconRef::Glyph { name }
}

/// Toutes les entrées, dans l'ordre d'intérêt à recherche vide.
pub(super) fn catalogue(app: &AppHandle) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    let layout = app.state::<AppState>().snapshot();

    // Suggestions des écrans (applis et sites selon l'usage).
    let mut seen = std::collections::HashSet::new();
    let shown = crate::widgets::suggestions::suggestions_state(app);
    for s in shown.into_values().flatten() {
        if seen.insert(s.key.clone()) {
            let what = if s.kind == "site" { "Site suggéré" } else { "Appli suggérée" };
            out.push(
                Entry::new(format!("sg:{}", s.key), s.label, format!("{what} · {}", s.reason), IconRef::Suggestion { key: s.key.clone() })
                    .keywords("suggestion")
                    .action("Ouvrir", Action::Suggestion(s.key)),
            );
        }
    }

    // Raccourcis de toutes les barres.
    let mut targets = std::collections::HashSet::new();
    for w in layout.widgets_of(WidgetKind::SHORTCUTS) {
        let items: Vec<Shortcut> = serde_json::from_value(w.settings["items"].clone()).unwrap_or_default();
        for s in items {
            if targets.insert(s.target.to_lowercase()) {
                out.push(
                    Entry::new(format!("sc:{}", s.target), s.label.clone(), "Raccourci", IconRef::Shortcut { item: s.clone() })
                        .keywords(format!("raccourci {}", s.target))
                        .action("Ouvrir", Action::Launch(s.target)),
                );
            }
        }
    }

    // Actions.
    sessions_entries(app, &mut out);
    let (muted, krisp) = crate::widgets::mic_krisp::status(app);
    if let Some(m) = muted {
        let (title, icon) = if m { ("Rouvrir le micro", "microphone") } else { ("Couper le micro", "microphoneSlash") };
        out.push(Entry::new("act:mic", title, "Micro par défaut de Windows", glyph(icon)).keywords("micro muet son windows").action(title, Action::Mic));
    }
    if let Some(k) = krisp {
        let title = if k { "Désactiver Krisp" } else { "Activer Krisp" };
        out.push(Entry::new("act:krisp", title, "Discord", glyph("waveform")).keywords("krisp bruit discord").action(title, Action::Krisp));
    }
    discord_entries(app, &mut out);
    dnd_entry(app, &mut out);
    timer_entries(app, &mut out);
    media_entries(app, &mut out);
    let (outputs, default) = crate::mixer::outputs(app);
    if outputs.len() > 1 {
        out.push(
            Entry::new("act:next-output", "Sortie audio suivante", "Favoris s’il y en a", glyph("speakerHigh"))
                .keywords("son audio casque haut-parleur basculer")
                .action("Changer", Action::NextOutput),
        );
    }
    out.push(Entry::new("act:edit", "Modifier la barre", "Mode édition de l’écran principal", glyph("pencilSimple")).keywords("édition disposition widgets").action("Modifier", Action::Edit));
    out.push(Entry::new("act:profiles", "Gérer les profils…", "Profils de disposition", glyph("userSwitch")).keywords("profil réglages").action("Ouvrir", Action::Profiles));
    out.push(
        Entry::new("act:settings", "Réglages d’Horizon", "Barres, alertes, nuit, minuteur, palette", glyph("gearSix"))
            .keywords("réglages paramètres configuration options toml")
            .action("Ouvrir", Action::Options("barres"))
            .action("Modifier settings.toml", Action::SettingsFile),
    );
    out.push(
        Entry::new("act:status", "État d’Horizon", "Intégrations, journal", glyph("warning"))
            .keywords("état santé diagnostic journal intégrations")
            .action("Ouvrir", Action::Options("etat")),
    );
    out.push(
        Entry::new("act:reload", "Recharger les barres", "Recrée les fenêtres des barres", glyph("arrowsClockwise"))
            .keywords("recharger actualiser barres fenêtres")
            .action("Recharger", Action::Reload),
    );
    out.push(Entry::new("act:quit", "Quitter Horizon", "Ferme toutes les barres", glyph("power")).keywords("fermer arrêter").action("Quitter", Action::Quit));

    // Profils.
    for p in &layout.profiles {
        let active = p.id == layout.active_profile;
        out.push(
            Entry::new(format!("profile:{}", p.id), format!("Profil : {}", p.name), if active { "Actif" } else { "Activer ce profil" }, glyph("userSwitch"))
                .keywords("profil disposition")
                .action("Activer", Action::Profile(p.id.clone())),
        );
    }

    // Scènes Prisme.
    let light = crate::prisme::lighting_state(app.clone());
    if light.connected {
        out.push(Entry::new("scene:auto", "Éclairage automatique", "Prisme", glyph("lightbulb")).keywords("éclairage prisme scène règles").action("Activer", Action::Scene(None)));
        out.push(Entry::new("scene:toggle", "Éteindre ou rallumer l’éclairage", "Prisme", glyph("power")).keywords("éclairage prisme lumière").action("Basculer", Action::LightToggle));
        for s in light.scenes.iter().filter(|s| *s != "off") {
            out.push(
                Entry::new(format!("scene:{s}"), format!("Éclairage : {}", horizon_core::light_signal::scene_name(s)), "Scène Prisme", glyph("lightbulbFill"))
                    .keywords(format!("éclairage prisme scène {s}"))
                    .action("Activer", Action::Scene(Some(s.clone()))),
            );
        }
    }

    // Sorties audio.
    for d in outputs.into_iter().filter(|d| default.as_deref() != Some(d.id.as_str())) {
        out.push(
            Entry::new(format!("out:{}", d.id), format!("Sortie : {}", d.name), "Devient la sortie audio par défaut", glyph("speakerHigh"))
                .keywords("son audio sortie")
                .action("Choisir", Action::Output(d.id)),
        );
    }

    // Micros.
    let (inputs, default_in) = crate::mixer::inputs(app);
    for d in inputs.into_iter().filter(|d| default_in.as_deref() != Some(d.id.as_str())) {
        out.push(
            Entry::new(format!("in:{}", d.id), format!("Micro : {}", d.name), "Devient le micro par défaut", glyph("microphone"))
                .keywords("micro entrée enregistrement")
                .action("Choisir", Action::Output(d.id)),
        );
    }

    // Hôtes du homelab.
    for h in crate::widgets::servers::known_hosts(app) {
        let mut e = Entry::new(format!("host:{}", h.id), format!("SSH : {}", h.label), h.address.clone(), glyph("hardDrives"))
            .keywords(format!("ssh serveur homelab terminal {}", h.id));
        if let Some(t) = h.ssh {
            e = e.action("Terminal SSH", Action::Ssh(t));
        }
        if let Some(u) = h.url {
            e = e.action("Interface web", Action::Url(u));
        }
        if !e.actions.is_empty() {
            out.push(e);
        }
    }

    window_entries(app, &mut out);

    // Applis du menu Démarrer (sans celles déjà en raccourci).
    for a in app.state::<Palette>().start_apps(app) {
        if targets.contains(&a.path.to_lowercase()) {
            continue;
        }
        let item = Shortcut { label: a.name.clone(), target: a.path.clone(), icon: ShortcutIcon::Exe, color: None };
        out.push(
            Entry::new(format!("app:{}", a.path), a.name, "Application", IconRef::Shortcut { item })
                .keywords("appli programme")
                .action("Ouvrir", Action::Launch(a.path.clone()))
                .action("Emplacement du fichier", Action::Reveal(a.path)),
        );
    }
    out
}

/// Préfixe des entrées de fenêtres : elles ne vivent que le temps de la fenêtre, leur usage
/// n'est pas retenu.
pub(super) const WINDOW_PREFIX: &str = "win:";

/// « Ramener une fenêtre ici » : chaque fenêtre ouverte (taper « ramener » les liste toutes),
/// ramenée sur l'écran de la palette ; celles déjà sur cet écran sont seulement affichées.
fn window_entries(app: &AppHandle, out: &mut Vec<Entry>) {
    let cursor = horizon_sys::fullscreen::cursor().map(|(x, y)| horizon_core::geometry::Rect { x, y, w: 1, h: 1 });
    for g in crate::widgets::windows::groups(app, cursor) {
        for w in g.windows {
            let minimized = if w.minimized { " · réduite" } else { "" };
            let item = Shortcut { label: w.app.clone(), target: w.icon.clone(), icon: ShortcutIcon::Exe, color: None };
            let (title, subtitle) = if g.here {
                (format!("Afficher : {}", w.title), format!("{} · déjà sur cet écran{minimized}", w.app))
            } else {
                (format!("Ramener ici : {}", w.title), format!("{} · {}{minimized}", w.app, g.name))
            };
            let mut e = Entry::new(format!("{WINDOW_PREFIX}{}", w.hwnd), title, subtitle, IconRef::Shortcut { item })
                .keywords(format!("ramener fenêtre ici déplacer écran afficher {} {}", w.app, g.name));
            let h = w.hwnd as isize;
            e = if g.here {
                e.action("Afficher", Action::FocusWindow(h))
            } else {
                e.action("Ramener ici", Action::BringWindow(h)).action("Afficher sans déplacer", Action::FocusWindow(h))
            };
            out.push(e);
        }
    }
}

/// Sessions Claude Code : Entrée = terminal au premier plan ; Tab = oublier la session.
fn sessions_entries(app: &AppHandle, out: &mut Vec<Entry>) {
    use horizon_core::claude_sessions::SessionState;
    for s in crate::widgets::claude_sessions::sessions(app) {
        let state = match s.state {
            SessionState::Waiting => "attend",
            SessionState::Working => "travaille",
            SessionState::Done => "terminé",
        };
        let subtitle = match &s.message {
            Some(m) => format!("Claude {state} · {m}"),
            None => format!("Claude {state}"),
        };
        let mut e = Entry::new(format!("claude:{}", s.id), format!("Session : {}", s.name), subtitle, glyph("terminalWindow"))
            .keywords(format!("claude session terminal {state} {}", s.cwd));
        if s.can_focus {
            e = e.action("Afficher le terminal", Action::ClaudeFocus(s.id.clone()));
        }
        out.push(e.action("Oublier la session", Action::ClaudeForget(s.id)));
    }
}

/// Discord en appel : micro, son, quitter le salon.
fn discord_entries(app: &AppHandle, out: &mut Vec<Entry>) {
    let Some((mute, deaf, can_leave)) = crate::widgets::discord_voice::call_status(app) else { return };
    let (title, icon) =
        if mute { ("Rouvrir le micro dans Discord", "microphone") } else { ("Couper le micro dans Discord", "microphoneSlash") };
    out.push(
        Entry::new("voice:mute", title, "Discord vocal", glyph(icon))
            .keywords("discord micro muet appel")
            .action(title, Action::Voice("mute")),
    );
    let (title, icon) = if deaf { ("Rétablir le son dans Discord", "headphones") } else { ("Couper le son dans Discord", "earSlash") };
    out.push(
        Entry::new("voice:deaf", title, "Discord vocal", glyph(icon))
            .keywords("discord son sourdine appel")
            .action(title, Action::Voice("deaf")),
    );
    if can_leave {
        out.push(
            Entry::new("voice:leave", "Quitter le salon vocal", "Discord", glyph("phoneDisconnect"))
                .keywords("discord raccrocher quitter appel salon")
                .action("Quitter", Action::LeaveCall),
        );
    }
}

/// « Ne pas déranger ».
fn dnd_entry(app: &AppHandle, out: &mut Vec<Entry>) {
    let on = crate::dnd::is_active(app);
    let (title, icon) = if on { ("Désactiver « Ne pas déranger »", "moon") } else { ("Activer « Ne pas déranger »", "moonFill") };
    out.push(
        Entry::new("act:dnd", title, "Notifications", glyph(icon))
            .keywords("ne pas déranger npd silence notifications")
            .action(title, Action::Dnd),
    );
}

/// Minuteur : préréglages et pomodoro, ou pause / reprise / +5 min / arrêt d'une séance.
fn timer_entries(app: &AppHandle, out: &mut Vec<Entry>) {
    let kw = "minuteur timer chrono pomodoro";
    match crate::timer::paused(app) {
        None => {
            let t = crate::state::settings().timer;
            for m in t.presets {
                out.push(
                    Entry::new(format!("timer:{m}"), format!("Minuteur : {m} min"), "Compte à rebours près de l’horloge", glyph("timer"))
                        .keywords(kw)
                        .action("Démarrer", Action::TimerStart(m)),
                );
            }
            out.push(
                Entry::new("timer:pomodoro", format!("Pomodoro : {} min / {} min", t.focus, t.rest), "Travail et pause en boucle", glyph("timer"))
                    .keywords(kw)
                    .action("Démarrer", Action::Pomodoro),
            );
        }
        Some(paused) => {
            let (title, icon) = if paused { ("Reprendre le minuteur", "play") } else { ("Mettre le minuteur en pause", "pause") };
            out.push(Entry::new("timer:pause", title, "Minuteur", glyph(icon)).keywords(kw).action(title, Action::TimerPause(!paused)));
            out.push(
                Entry::new("timer:extend", "Minuteur : +5 min", "Prolonge la phase en cours", glyph("plus"))
                    .keywords(kw)
                    .action("Ajouter", Action::TimerExtend(5)),
            );
            out.push(Entry::new("timer:stop", "Arrêter le minuteur", "Minuteur", glyph("stop")).keywords(kw).action("Arrêter", Action::TimerStop));
        }
    }
}

/// Lecture en cours : lecture / pause, suivant, précédent.
fn media_entries(app: &AppHandle, out: &mut Vec<Entry>) {
    let Some((title, playing)) = crate::widgets::media::now_playing(app) else { return };
    let kw = "musique lecture média";
    let (label, icon) = if playing { ("Mettre en pause", "pause") } else { ("Reprendre la lecture", "play") };
    out.push(Entry::new("media:play", label, title.clone(), glyph(icon)).keywords(format!("{kw} pause play")).action(label, Action::PlayPause));
    out.push(
        Entry::new("media:next", "Morceau suivant", title.clone(), glyph("skipForward"))
            .keywords(format!("{kw} suivant"))
            .action("Suivant", Action::NextTrack),
    );
    out.push(
        Entry::new("media:prev", "Morceau précédent", title, glyph("skipBack"))
            .keywords(format!("{kw} précédent"))
            .action("Précédent", Action::PrevTrack),
    );
}

pub(super) fn run_action(app: &AppHandle, a: Action) -> Result<(), String> {
    match a {
        Action::Launch(t) => {
            // Comme un clic sur un raccourci : l'appli déjà ouverte revient au premier plan.
            crate::widgets::shortcuts::running::shortcuts_open(t, false);
            Ok(())
        }
        Action::Reveal(p) => {
            horizon_sys::launch::reveal(&p);
            Ok(())
        }
        Action::Suggestion(k) => crate::widgets::suggestions::suggestions_open(app, &k),
        Action::Profile(id) => crate::profiles::switch(app, &id),
        Action::Scene(s) => crate::prisme::lighting_set_scene(app.clone(), s),
        Action::LightToggle => crate::prisme::lighting_toggle(app.clone()),
        Action::Mic => crate::widgets::mic_krisp::mic_toggle(app.clone()),
        Action::Krisp => crate::widgets::mic_krisp::krisp_toggle(app.clone()),
        Action::NextOutput => crate::mixer::next_output(app).map(|name| {
            let alert = Alert::icon("speakerHigh", Severity::Info, "Sortie audio".into(), name);
            crate::alerts::show(app, AlertKind::Feedback, alert);
        }),
        Action::Output(id) => crate::mixer::set_default(app, id),
        Action::SettingsFile => std::process::Command::new("notepad.exe")
            .arg(crate::state::app_dir().join("settings.toml"))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string()),
        Action::Options(section) => {
            crate::settings_window::open(app, crate::settings_window::SettingsPage::Options(section.into()));
            Ok(())
        }
        Action::Profiles => {
            crate::settings_window::open(app, crate::settings_window::SettingsPage::Profiles);
            Ok(())
        }
        Action::Edit => crate::edit::edit_main_bar(app),
        Action::Quit => {
            app.exit(0);
            Ok(())
        }
        Action::Ssh(t) => crate::widgets::servers::open_ssh(&t),
        Action::Url(u) => crate::web::url_open(&u),
        Action::Dnd => {
            crate::dnd::toggle(app);
            Ok(())
        }
        Action::TimerStart(m) => {
            crate::timer::timer_start(app, m);
            Ok(())
        }
        Action::Pomodoro => {
            crate::timer::timer_pomodoro(app);
            Ok(())
        }
        Action::TimerPause(p) => {
            crate::timer::timer_pause(app, p);
            Ok(())
        }
        Action::TimerStop => {
            crate::timer::timer_stop(app);
            Ok(())
        }
        Action::TimerExtend(m) => {
            crate::timer::timer_extend(app, m);
            Ok(())
        }
        Action::PlayPause => crate::widgets::media::media_play_pause(app),
        Action::NextTrack => crate::widgets::media::media_next(app),
        Action::PrevTrack => crate::widgets::media::media_prev(app),
        Action::Voice(what) => crate::widgets::discord_voice::voice_toggle(app.clone(), what.into()),
        Action::LeaveCall => crate::widgets::discord_voice::voice_leave(app.clone()),
        Action::ClaudeFocus(id) => crate::widgets::claude_sessions::claude_focus(app.clone(), id),
        Action::ClaudeForget(id) => {
            crate::widgets::claude_sessions::claude_forget(app.clone(), id);
            Ok(())
        }
        // Comme le menu de l'icône : la reconstruction se fait sur le fil de l'interface.
        Action::Reload => {
            let a = app.clone();
            app.run_on_main_thread(move || crate::bars::rebuild(&a)).map_err(|e| e.to_string())
        }
        Action::BringWindow(h) => {
            crate::widgets::windows::bring(app, h, super::origin(app));
            Ok(())
        }
        Action::FocusWindow(h) => {
            crate::util::spawn_named("fenetre-afficher", move || {
                horizon_sys::window_focus::bring_to_front(h);
            });
            Ok(())
        }
    }
}
