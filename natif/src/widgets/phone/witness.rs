//! Saisie à distance (clavier et souris d'un téléphone) : application par `SendInput`, et sa
//! garde. Une séance commence à la première saisie, ou à la reprise après 10 s de silence : elle
//! est annoncée dans l'îlot (même pendant le NPD), et un témoin reste sur les barres tant
//! qu'elle dure. « Couper » (témoin ou alerte) retire le droit `input` du téléphone.
//! Décisions pures : `horizon_core::phone::input`.

use super::PhoneState;
use crate::alerts::{Action, Alert, Severity};
use crate::host::{AppHandle, Manager};
use crate::util::{Published, lock};
use horizon_core::dnd::AlertKind;
use horizon_core::phone::input::{ButtonAction, InputSessions, MouseButton};
use horizon_core::phone::remote::{Command, INPUT};
use horizon_core::time::now_ms;
use horizon_sys::input::{self as sys, Button};
use std::sync::Mutex;
use std::time::Duration;

/// Noms des téléphones qui saisissent en ce moment (témoin des barres).
pub const EV_PHONE_INPUT: &str = "horizon://phone-input";

/// Séances en cours, et le fil qui efface le témoin quand elles se taisent.
pub struct Witness {
    sessions: Mutex<Watch>,
    shown: Published<Vec<String>>,
}

#[derive(Default)]
struct Watch {
    sessions: InputSessions,
    /// Le fil d'effacement tourne (décidé sous le verrou : un seul à la fois, et jamais aucun
    /// pendant qu'une séance dure).
    running: bool,
}

impl Default for Witness {
    fn default() -> Witness {
        Witness { sessions: Mutex::default(), shown: Published::new(EV_PHONE_INPUT, Vec::new()) }
    }
}

/// Applique une saisie déjà autorisée et bornée.
pub fn apply(cmd: &Command) {
    match cmd {
        Command::Move(dx, dy) => sys::move_by(*dx, *dy),
        Command::Button(button, action) => {
            let button = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            };
            let (down, up) = match action {
                ButtonAction::Click => (true, true),
                ButtonAction::Down => (true, false),
                ButtonAction::Up => (false, true),
            };
            sys::button(button, down, up);
        }
        Command::Scroll(dx, dy) => sys::wheel(*dx, *dy),
        Command::Zoom(delta) => sys::zoom(*delta),
        Command::Text(text) => sys::text(text),
        Command::Key(vk, mods) => sys::key(*vk, &mods.iter().map(|m| m.code()).collect::<Vec<_>>()),
        _ => {}
    }
}

/// Nom donné au téléphone `id` à l'appairage (l'identifiant s'il est inconnu).
fn phone_name(app: &AppHandle, id: &str) -> String {
    let core = app.state::<PhoneState>().core.get().cloned();
    core.and_then(|c| c.phones(&[], 0).into_iter().find(|p| p.id == id).map(|p| p.name)).unwrap_or_else(|| id.to_owned())
}

/// Pose le témoin : noms des téléphones dont la séance dure.
fn publish(app: &AppHandle, ids: &[String]) {
    let names = ids.iter().map(|id| phone_name(app, id)).collect();
    app.state::<PhoneState>().witness.shown.set(app, names);
}

/// `phone_id` vient de saisir : une séance qui commence est annoncée et pose le témoin.
pub fn seen(app: &AppHandle, phone_id: &str) {
    let state = app.state::<PhoneState>();
    let (active, start_thread) = {
        let mut watch = lock(&state.witness.sessions);
        if !watch.sessions.seen(phone_id, now_ms()) {
            return;
        }
        let start = !watch.running;
        watch.running = true;
        (watch.sessions.active(now_ms()), start)
    };
    let name = phone_name(app, phone_id);
    log::info!("téléphone : {phone_id} ({name}) prend le clavier et la souris");
    // Titre court (l'îlot coupe vite) ; l'infobulle de l'action dit que le clic coupe.
    let alert = Alert::icon("keyboard", Severity::Attention, "Clavier et souris".into(), name.clone())
        .with_action(Action::CutInput { phone: phone_id.to_owned() });
    crate::alerts::show(app, AlertKind::RemoteInput, alert);
    publish(app, &active);
    if start_thread {
        let a = app.clone();
        if let Err(e) = crate::util::start_thread("phone-input-witness", move || fade(&a)) {
            log::warn!("téléphone : {e}");
            lock(&state.witness.sessions).running = false;
        }
    }
}

/// Efface le témoin des séances qui se taisent, puis s'arrête quand il n'en reste aucune.
fn fade(app: &AppHandle) {
    let state = app.state::<PhoneState>();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let active = {
            let mut watch = lock(&state.witness.sessions);
            let active = watch.sessions.active(now_ms());
            watch.running = !active.is_empty();
            active
        };
        publish(app, &active);
        if active.is_empty() {
            return;
        }
    }
}

/// Coupe le clavier et la souris de `phone_id` : son droit `input` est retiré (le téléphone
/// l'apprend), sa séance close.
pub fn cut(app: &AppHandle, phone_id: &str) {
    log::info!("téléphone : clavier et souris coupés pour {phone_id}");
    super::set_right(app, phone_id, INPUT, false);
    let state = app.state::<PhoneState>();
    let active = {
        let mut watch = lock(&state.witness.sessions);
        watch.sessions.end(phone_id);
        watch.sessions.active(now_ms())
    };
    publish(app, &active);
}

/// Coupe tous les téléphones qui saisissent (clic sur le témoin).
pub fn cut_all(app: &AppHandle) {
    let active = lock(&app.state::<PhoneState>().witness.sessions).sessions.active(now_ms());
    for id in active {
        cut(app, &id);
    }
}

/// Noms des téléphones qui saisissent en ce moment (témoin d'une barre qui naît).
pub fn active_names(app: &AppHandle) -> Vec<String> {
    app.state::<PhoneState>().witness.shown.get()
}
