//! Îlot dynamique dans les barres (partie « flash » de `app/src/lib/live.svelte.ts`) : l'alerte
//! reçue (`horizon://alert`) est montrée quelques secondes (3,2 s, ou sa durée propre), puis
//! s'efface en fondu avant le retour du contenu habituel ; un appel du téléphone
//! (`horizon://phone`) reste dans l'îlot tant qu'il dure (une alerte passe devant, puis l'appel
//! revient) ; clic sur une alerte qui a une action (terminal de la session, mode édition,
//! capture collée). Global `FlashData` de `ui/flash.slint`, le même pour toutes les barres ; le
//! widget qui le montre est choisi par la barre (`media-host`). Fil de l'interface.

use super::{BarWindow, FlashData, icons};
use crate::alerts::{Action, Alert, Pixels, Severity, Source};
use crate::events::{EV_ALERT, EV_PHONE};
use crate::host::{self, AppHandle};
use horizon_core::layout::{BarLayout, BarStyle};
use horizon_core::phone::{CallState, PhoneView};
use slint::{ComponentHandle, SharedPixelBuffer};
use std::cell::{Cell, RefCell};
use std::time::Duration;

/// Durée d'affichage d'une alerte (sans durée propre).
const FLASH_MS: u64 = 3200;
/// Fondu de sortie (`--dur-fast`).
const FADE_OUT: Duration = Duration::from_millis(120);
/// Numéro fixe de l'îlot d'appel : il ne rejoue pas son entrée à chaque seconde.
const PHONE_SEQ: i32 = -1;

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
        Some(Action::Edit) => "Voir en mode édition",
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

/// Appel du téléphone présenté comme une alerte qui dure (`phoneAlert` de live.svelte.ts).
fn phone_view(p: &PhoneView, now_ms: u64) -> View {
    let (icon, body, severity) = match p.state {
        CallState::Ringing => ("phoneIncoming", "Appel entrant…".to_owned(), Severity::Attention),
        CallState::Active => {
            ("phoneCall", format!("En appel · {}", super::format::clock(now_ms.saturating_sub(p.since_ms))), Severity::Info)
        }
        CallState::Ended | CallState::Idle => ("phoneDisconnect", "Appel terminé".to_owned(), Severity::Info),
    };
    View {
        seq: PHONE_SEQ,
        severity,
        mark: Mark::Icon(icon),
        title: p.label.clone(),
        body,
        claude: false,
        action_tip: "",
        ringing: p.state == CallState::Ringing,
    }
}

/// L'alerte passagère, sinon l'appel en cours.
fn shown(flash: Option<&Alert>, phone: Option<&PhoneView>, now_ms: u64) -> Option<View> {
    flash.map(alert_view).or_else(|| phone.map(|p| phone_view(p, now_ms)))
}

#[derive(Default)]
struct State {
    flash: Option<Alert>,
    /// Fondu de sortie en cours.
    out: bool,
    phone: Option<PhoneView>,
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
    let (view, out, thumbs) = STATE.with_borrow(|s| (shown(s.flash.as_ref(), s.phone.as_ref(), now), s.out, s.thumbs.clone()));
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

/// Appel du téléphone : îlot tant qu'il dure ; la durée avance chaque seconde pendant l'appel.
fn on_phone(p: Option<PhoneView>) {
    let active = p.as_ref().is_some_and(|p| p.state == CallState::Active);
    STATE.with_borrow_mut(|s| s.phone = p);
    CALL.with(|t| {
        if !active {
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
    apply_all();
}

/// Clic sur l'alerte montrée dans la barre `label` (seulement si elle a une action et que la
/// barre est cliquable).
fn run(app: &AppHandle, label: &str) {
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
        // « Nouveautés » : mode édition de cette barre.
        Some(Action::Edit) => {
            let edit = crate::edit::monitor_of(app, label).ok_or("barre inconnue".to_owned());
            if let Err(e) = edit.and_then(|m| crate::edit::edit_bar(app, &m, true)) {
                log::warn!("îlot, mode édition : {e}");
            }
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
pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, bar: &BarLayout) {
    if !WATCHING.replace(true) {
        host::listen(EV_ALERT, None, |e| {
            if let Some(a) = e.payload::<Alert>() {
                show_flash(a);
            }
        });
        host::listen(EV_PHONE, None, |e| {
            if let Some(p) = e.payload::<Option<PhoneView>>() {
                on_phone(p.clone());
            }
        });
        STATE.with_borrow_mut(|s| s.phone = crate::phone::phone_state(app));
    }
    let g = window.global::<FlashData>();
    relayout(window, bar);
    let (a, l) = (app.clone(), label.to_owned());
    g.on_run(move || run(&a, &l));
    BARS.with_borrow_mut(|b| b.push(window.as_weak()));
    let now = horizon_core::time::now_ms();
    STATE.with_borrow(|s| apply(window, shown(s.flash.as_ref(), s.phone.as_ref(), now).as_ref(), s.out, s.thumbs.as_ref()));
    // Appel déjà en cours au lancement : sa durée avance.
    if let Some(p) = STATE.with_borrow(|s| s.phone.clone()) {
        on_phone(Some(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(state: CallState) -> PhoneView {
        PhoneView { state, label: "Léo".into(), since_ms: 1_000 }
    }

    #[test]
    fn phone_is_shown_like_an_alert_that_lasts() {
        let v = phone_view(&call(CallState::Ringing), 5_000);
        assert_eq!((v.mark.clone(), v.body.as_str(), v.severity, v.ringing), (Mark::Icon("phoneIncoming"), "Appel entrant…", Severity::Attention, true));
        let v = phone_view(&call(CallState::Active), 66_000);
        assert_eq!((v.mark.clone(), v.body.as_str(), v.severity, v.ringing), (Mark::Icon("phoneCall"), "En appel · 1:05", Severity::Info, false));
        let v = phone_view(&call(CallState::Ended), 9_000);
        assert_eq!((v.mark, v.body.as_str(), v.seq), (Mark::Icon("phoneDisconnect"), "Appel terminé", PHONE_SEQ));
    }

    #[test]
    fn a_passing_alert_goes_in_front_of_the_call() {
        let a = Alert::app("Claude", "", Severity::Attention, "deskbar".into(), "attend".into())
            .with_action(Action::Session { id: "s".into() });
        let p = call(CallState::Active);
        let v = shown(Some(&a), Some(&p), 2_000).unwrap();
        assert_eq!((v.title.as_str(), v.mark, v.claude, v.action_tip), ("deskbar", Mark::App("Claude".into()), true, "Afficher le terminal"));
        assert_eq!(shown(None, Some(&p), 2_000).unwrap().title, "Léo");
        assert_eq!(shown(None, None, 0), None);
    }

    #[test]
    fn tooltips_of_the_actions() {
        assert_eq!(action_tip(Some(&Action::Edit)), "Voir en mode édition");
        let c = Action::Capture { path: "p".into(), session: "s".into() };
        assert_eq!(action_tip(Some(&c)), "Coller le chemin dans le terminal (sans l’envoyer)");
        assert_eq!(action_tip(None), "");
        let a = Alert::icon("timer", Severity::Attention, "Minuteur terminé".into(), "25 min écoulées".into());
        let v = alert_view(&a);
        assert_eq!((v.mark, v.claude, v.action_tip, v.ringing), (Mark::Icon("timer"), false, "", false));
    }
}
