//! Îlot dynamique dans les barres (partie « flash » de `app/src/lib/live.svelte.ts`) : l'alerte
//! reçue (`horizon://alert`) est montrée quelques secondes (3,2 s, ou sa durée propre), puis
//! s'efface en fondu avant le retour du contenu habituel ; une activité durable d'un plugin (un
//! appel du téléphone, par exemple) reste dans l'îlot tant qu'elle dure (une alerte passe devant,
//! puis l'activité revient) ; clic sur une alerte qui a une action (terminal de la session, mode édition,
//! capture collée). Global `FlashData` de `ui/flash.slint`, le même pour toutes les barres ; le
//! widget qui le montre est choisi par la barre (`media-host`). Fil de l'interface.

use super::{BarWindow, FlashData, icons};
use crate::alerts::{Action, Alert, Pixels, Severity, Source};
use crate::events::EV_ALERT;
use crate::host::{self, AppHandle};
use horizon_core::layout::{BarLayout, BarStyle};
use slint::{ComponentHandle, SharedPixelBuffer};
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// Durée d'affichage d'une alerte (sans durée propre).
const FLASH_MS: u64 = 3200;
/// Fondu de sortie (`--dur-fast`).
const FADE_OUT: Duration = Duration::from_millis(120);
/// Numéro fixe de l'îlot d'une activité de plugin : il ne rejoue pas son entrée à chaque seconde.
const ACTIVITY_SEQ: i32 = -2;
/// Activités au plus par plugin (les plus anciennes partent).
const MAX_ACTIVITIES: usize = 8;

/// Pose (`Some`) ou retire (`None`) l'activité `key` (« plugin:clé »), au plus
/// [`MAX_ACTIVITIES`] par plugin.
fn put_activity(list: &mut Vec<(String, Activity)>, key: String, a: Option<Activity>) {
    list.retain(|(k, _)| *k != key);
    let Some(a) = a else { return };
    let prefix = format!("{}:", key.split(':').next().unwrap_or_default());
    while list.iter().filter(|(k, _)| k.starts_with(&prefix)).count() >= MAX_ACTIVITIES {
        if let Some(i) = list.iter().position(|(k, _)| k.starts_with(&prefix)) {
            list.remove(i);
        }
    }
    list.push((key, a));
}

/// Retire toutes les activités du plugin `id`.
fn remove_plugin_activities(list: &mut Vec<(String, Activity)>, id: &str) {
    let prefix = format!("{id}:");
    list.retain(|(k, _)| !k.starts_with(&prefix));
}

/// Activité durable d'un plugin dans l'îlot (`horizon.island.show`), comme un appel.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub icon: &'static str,
    pub title: String,
    pub subtitle: String,
    /// Début (ms Unix) : chronomètre après le sous-titre.
    pub since_ms: Option<u64>,
    pub attention: bool,
}

fn activity_view(a: &Activity, now_ms: u64) -> View {
    let clock = a.since_ms.map(|s| super::format::clock(now_ms.saturating_sub(s)));
    let body = match (a.subtitle.is_empty(), clock) {
        (_, None) => a.subtitle.clone(),
        (true, Some(c)) => c,
        (false, Some(c)) => format!("{} · {c}", a.subtitle),
    };
    View {
        seq: ACTIVITY_SEQ,
        severity: if a.attention { Severity::Attention } else { Severity::Info },
        mark: Mark::Icon(a.icon),
        title: a.title.clone(),
        body,
        claude: false,
        action_tip: "",
        // Activité qui demande l'attention (appel entrant) : l'îlot sonne.
        ringing: a.attention,
    }
}

/// Ce qui est à gauche du texte (`FlashData.mark`).
#[derive(Debug, Clone, PartialEq)]
enum Mark {
    Icon(&'static str),
    /// Logo d'une appli (nom de l'appli).
    App(String),
    Thumb,
}

/// Ce que montre l'îlot.
#[derive(Debug, Clone, PartialEq)]
struct View {
    seq: i32,
    severity: Severity,
    mark: Mark,
    title: String,
    body: String,
    claude: bool,
    /// Infobulle de l'action d'un clic (vide : aucune).
    action_tip: &'static str,
    ringing: bool,
}

/// Infobulle de l'action d'une alerte (`alertAction` de live.svelte.ts).
fn action_tip(a: Option<&Action>) -> &'static str {
    match a {
        Some(Action::Session { .. }) => "Afficher le terminal",
        Some(Action::Capture { .. }) => "Coller le chemin dans le terminal (sans l’envoyer)",
        Some(Action::Update) => "Installer et redémarrer Horizon",
        None => "",
    }
}

fn alert_view(a: &Alert) -> View {
    let mark = match (&a.image, &a.source) {
        (Some(_), _) => Mark::Thumb,
        (None, Source::Icon { icon }) => Mark::Icon(icon),
        (None, Source::App { name, .. }) => Mark::App(name.clone()),
    };
    View {
        seq: (a.id % i32::MAX as u64) as i32,
        severity: a.severity,
        mark,
        title: a.title.clone(),
        body: a.body.clone(),
        claude: matches!(a.action, Some(Action::Session { .. })),
        action_tip: action_tip(a.action.as_ref()),
        ringing: false,
    }
}

/// L'alerte passagère, sinon la dernière activité d'un plugin.
fn shown(flash: Option<&Alert>, activity: Option<&Activity>, now_ms: u64) -> Option<View> {
    flash.map(alert_view).or_else(|| activity.map(|a| activity_view(a, now_ms)))
}

#[derive(Default)]
struct State {
    flash: Option<Alert>,
    /// Fondu de sortie en cours.
    out: bool,
    /// Activités des plugins (clé « plugin:id »), la plus récente en dernier.
    activities: Vec<(String, Activity)>,
    /// Miniatures de l'alerte en cours, converties une fois.
    thumbs: Option<(slint::Image, slint::Image)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
    static BARS: RefCell<Vec<slint::Weak<BarWindow>>> = RefCell::default();
    /// Fin de l'alerte, puis fin de son fondu.
    static HOLD: slint::Timer = slint::Timer::default();
    /// Durée de l'appel, chaque seconde pendant l'appel.
    static CALL: slint::Timer = slint::Timer::default();
    static WATCHING: Cell<bool> = const { Cell::new(false) };
}

fn image(p: &Pixels) -> slint::Image {
    slint::Image::from_rgba8(SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&p.rgba, p.width, p.height))
}

/// Pose ce que montre l'îlot dans une barre.
fn apply(window: &BarWindow, view: Option<&View>, out: bool, thumbs: Option<&(slint::Image, slint::Image)>) {
    let g = window.global::<FlashData>();
    let Some(v) = view else {
        g.set_active(false);
        g.set_out(false);
        g.set_ringing(false);
        return;
    };
    g.set_severity(match v.severity {
        Severity::Info => 0,
        Severity::Attention => 1,
        Severity::Alert => 2,
    });
    match &v.mark {
        Mark::Icon(name) => {
            g.set_mark(0);
            g.set_icon(icons::image(name));
        }
        Mark::App(name) => match crate::brands::by_app_name(name) {
            Some(b) => {
                g.set_mark(1);
                g.set_icon(crate::brands::image(&b));
                let color = crate::brands::readable_on_dark(b.hex);
                g.set_logo_tinted(color.is_some());
                g.set_logo_color(color.unwrap_or_default());
            }
            None => g.set_mark(2),
        },
        Mark::Thumb => {
            g.set_mark(3);
            if let Some((island, line)) = thumbs {
                g.set_thumb(island.clone());
                g.set_thumb_line(line.clone());
            }
        }
    }
    g.set_title(v.title.as_str().into());
    g.set_body(v.body.replace('\n', " ").into());
    g.set_claude(v.claude);
    g.set_action_tip(v.action_tip.into());
    g.set_ringing(v.ringing);
    g.set_out(out);
    // Le numéro en dernier : l'entrée rejouée voit déjà le nouveau contenu.
    g.set_seq(v.seq);
    g.set_active(true);
}

/// Toutes les barres : nouvel état de l'îlot.
fn apply_all() {
    let now = horizon_core::time::now_ms();
    let (view, out, thumbs) =
        STATE.with_borrow(|s| (shown(s.flash.as_ref(), s.activities.last().map(|a| &a.1), now), s.out, s.thumbs.clone()));
    BARS.with_borrow_mut(|bars| {
        bars.retain(|w| w.upgrade().is_some());
        for w in bars.iter().filter_map(slint::Weak::upgrade) {
            apply(&w, view.as_ref(), out, thumbs.as_ref());
        }
    });
}

/// Nouvelle alerte : montrée tout de suite, pour sa durée.
fn show_flash(a: &Alert) {
    let hold = a.hold_ms.unwrap_or(FLASH_MS);
    STATE.with_borrow_mut(|s| {
        s.thumbs = a.image.as_ref().map(|t| (image(&t.island), image(&t.line)));
        s.flash = Some(a.clone());
        s.out = false;
    });
    HOLD.with(|t| t.start(slint::TimerMode::SingleShot, Duration::from_millis(hold), end_flash));
    apply_all();
}

/// Fondu de sortie, puis le contenu habituel (ou l'appel) revient.
fn end_flash() {
    if STATE.with_borrow(|s| s.flash.is_none()) {
        return;
    }
    STATE.with_borrow_mut(|s| s.out = true);
    apply_all();
    HOLD.with(|t| {
        t.start(slint::TimerMode::SingleShot, FADE_OUT, || {
            STATE.with_borrow_mut(|s| {
                s.flash = None;
                s.out = false;
                s.thumbs = None;
            });
            apply_all();
        })
    });
}

/// Activité durable d'un plugin (`None` : terminée). Chronomètre : redessin chaque seconde.
pub fn set_activity(key: String, a: Option<Activity>) {
    STATE.with_borrow_mut(|s| put_activity(&mut s.activities, key, a));
    ensure_ticking();
    apply_all();
}

/// Le plugin `id` n'est plus chargé : ses activités quittent l'îlot.
pub fn clear_plugin(id: &str) {
    STATE.with_borrow_mut(|s| remove_plugin_activities(&mut s.activities, id));
    ensure_ticking();
    apply_all();
}

/// Redessin chaque seconde tant qu'une activité chronométrée est montrée.
fn ensure_ticking() {
    let needed = STATE.with_borrow(|s| s.activities.iter().any(|(_, a)| a.since_ms.is_some()));
    CALL.with(|t| {
        if !needed {
            t.stop();
        } else if !t.running() {
            t.start(slint::TimerMode::Repeated, Duration::from_secs(1), || {
                // Une alerte passagère est devant : rien à redessiner.
                if STATE.with_borrow(|s| s.flash.is_none()) {
                    apply_all();
                }
            });
        }
    });
}

/// Clic sur l'alerte montrée dans une barre (seulement si elle a une action et que la barre est
/// cliquable).
fn run(app: &AppHandle) {
    let action = STATE.with_borrow(|s| s.flash.as_ref().and_then(|a| a.action.clone()));
    match action {
        Some(Action::Session { id }) => {
            let app = app.clone();
            let _ = crate::util::start_thread("claude-terminal", move || {
                if let Err(e) = crate::widgets::claude_sessions::claude_focus(app, id) {
                    log::warn!("terminal de la session : {e}");
                }
            });
        }
        // Chemin vide : alerte d'essai (`alerts::essai`), rien à coller.
        Some(Action::Capture { path, session }) => {
            end_flash();
            if !path.is_empty() {
                crate::widgets::claude_sessions::capture::capture_send(app, path, session);
            }
        }
        Some(Action::Update) => {
            end_flash();
            if let Err(e) = crate::update::install_now(app) {
                log::warn!("îlot, mise à jour : {e}");
            }
        }
        None => {}
    }
}

/// Disposition de la barre (création, changement) : un widget Lecture en cours en style îlots
/// montre l'alerte ; sinon, le widget Notifications.
pub fn relayout(window: &BarWindow, bar: &BarLayout) {
    let media = crate::widgets::manifest("media").is_some() && crate::widgets::instances(bar, "media").next().is_some();
    window.global::<FlashData>().set_media_host(bar.style == BarStyle::Islands && media);
}

/// Relie l'îlot d'une barre (toutes les barres : celles sans widget qui le montre ne
/// l'affichent pas).
pub fn bind(app: &AppHandle, window: &BarWindow, bar: &BarLayout) {
    if !WATCHING.replace(true) {
        host::listen(EV_ALERT, None, |e| {
            if let Some(a) = e.payload::<Alert>() {
                show_flash(a);
            }
        });
    }
    let g = window.global::<FlashData>();
    relayout(window, bar);
    let a = app.clone();
    g.on_run(move || run(&a));
    BARS.with_borrow_mut(|b| b.push(window.as_weak()));
    let now = horizon_core::time::now_ms();
    STATE.with_borrow(|s| apply(window, shown(s.flash.as_ref(), s.activities.last().map(|a| &a.1), now).as_ref(), s.out, s.thumbs.as_ref()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn island_activity_behind_an_alert() {
        let act = Activity { icon: "phoneCall", title: "Montre".into(), subtitle: "En charge".into(), since_ms: Some(1_000), attention: false };
        let a = Alert::icon("timer", Severity::Info, "Minuteur".into(), String::new());
        assert_eq!(shown(Some(&a), Some(&act), 2_000).unwrap().title, "Minuteur");
        let c = Alert::app("Claude", "", Severity::Attention, "deskbar".into(), "attend".into())
            .with_action(Action::Session { id: "s".into() });
        let v = shown(Some(&c), Some(&act), 2_000).unwrap();
        assert_eq!((v.title.as_str(), v.mark, v.claude, v.action_tip), ("deskbar", Mark::App("Claude".into()), true, "Afficher le terminal"));
        let v = shown(None, Some(&act), 66_000).unwrap();
        assert_eq!((v.title.as_str(), v.body.as_str(), v.ringing), ("Montre", "En charge · 1:05", false));
        let ring = Activity { attention: true, ..act };
        assert!(shown(None, Some(&ring), 0).unwrap().ringing, "appel entrant : l'îlot sonne");
        assert_eq!(shown(None, None, 0), None);
    }

    #[test]
    fn tooltips_of_the_actions() {
        assert_eq!(action_tip(Some(&Action::Update)), "Installer et redémarrer Horizon");
        let c = Action::Capture { path: "p".into(), session: "s".into() };
        assert_eq!(action_tip(Some(&c)), "Coller le chemin dans le terminal (sans l’envoyer)");
        assert_eq!(action_tip(None), "");
        let a = Alert::icon("timer", Severity::Attention, "Minuteur terminé".into(), "25 min écoulées".into());
        let v = alert_view(&a);
        assert_eq!((v.mark, v.claude, v.action_tip, v.ringing), (Mark::Icon("timer"), false, "", false));
    }
}
