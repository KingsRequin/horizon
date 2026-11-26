//! Page « Téléphone » de la fenêtre de réglages (`ui/phone/page.slint`) : autorisation du
//! pare-feu, QR code d'appairage, téléphones appairés, seuil d'alerte.
//!
//! Le décompte du code ne tourne que pendant qu'une offre est affichée ; quitter la page
//! annule l'offre. Un retrait de téléphone est différé (bandeau d'annulation de la fenêtre) :
//! son jeton ne redevient pas valable une fois retiré.

use super::{PhonePageData, PhoneRow, SettingsWindow};
use crate::host::{self, AppHandle};
use crate::settings_window::LABEL;
use crate::undoable::Removal;
use crate::widgets::phone::{self, EV_PHONE_LINK, LinkView, PhoneView};
use horizon_core::time::now_ms;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::time::Duration;

/// Où trouver la dernière version de l'appli.
const APP_URL: &str = "https://github.com/KingsRequin/horizon-mobile/releases/latest";
/// Un QR code se lit sombre sur clair.
const DARK: [u8; 4] = [0x11, 0x11, 0x1b, 0xff];
const LIGHT: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
/// Marge blanche autour du code, en modules (celle que demande la norme).
const QUIET: usize = 4;
/// Pixels par module : l'image est ensuite agrandie sans lissage par la page.
const SCALE: u32 = 4;

#[derive(Default)]
struct Page {
    app: Option<AppHandle>,
    window: Option<slint::Weak<SettingsWindow>>,
    shown: bool,
    /// Téléphones en cours de retrait (délai d'annulation) : cachés.
    leaving: Vec<String>,
}

thread_local! {
    static PAGE: RefCell<Page> = RefCell::default();
    /// Décompte de l'offre d'appairage affichée.
    static TICK: slint::Timer = slint::Timer::default();
}

fn with<R>(f: impl FnOnce(&mut Page) -> R) -> R {
    PAGE.with_borrow_mut(f)
}

fn window() -> Option<SettingsWindow> {
    with(|p| p.window.as_ref().and_then(slint::Weak::upgrade))
}

fn app() -> Option<AppHandle> {
    with(|p| p.app.clone())
}

/// Pixels RVBA du QR code de `payload` : (largeur, hauteur, octets). `None` si le texte ne
/// tient pas dans un QR code.
pub fn qr_pixels(payload: &str, scale: u32) -> Option<(u32, u32, Vec<u8>)> {
    let code = qrcode::QrCode::new(payload).ok()?;
    let modules = code.width();
    let colors = code.into_colors();
    let scale = scale.max(1) as usize;
    let side = (modules + 2 * QUIET) * scale;
    let mut px = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            // Dans la marge, la soustraction déborde : hors du code, donc clair.
            let (mx, my) = ((x / scale).wrapping_sub(QUIET), (y / scale).wrapping_sub(QUIET));
            let dark = mx < modules && my < modules && colors[my * modules + mx] == qrcode::Color::Dark;
            px.extend_from_slice(if dark { &DARK } else { &LIGHT });
        }
    }
    Some((side as u32, side as u32, px))
}

/// Image du QR code de `payload` (vide si le texte est trop long).
pub fn qr_image(payload: &str, scale: u32) -> slint::Image {
    match qr_pixels(payload, scale) {
        Some((w, h, px)) => slint::Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&px, w, h)),
        None => slint::Image::default(),
    }
}

/// Temps restant d'une offre d'appairage, en clair.
pub fn remaining(ms: u64) -> String {
    match ms / 1000 {
        _ if ms == 0 => "Code expiré".to_owned(),
        s if s >= 60 => format!("Valable encore {} min {:02}", s / 60, s % 60),
        s => format!("Valable encore {s} s"),
    }
}

/// « Connecté » ou « Vu il y a 3 h ».
pub fn phone_state(p: &PhoneView, now_ms: u64) -> String {
    if p.connected { "Connecté".to_owned() } else { format!("Vu {}", super::time::ago(p.last_seen_ms, now_ms)) }
}

/// Ce que dit la ligne « Réseau local » selon l'état du pare-feu.
pub fn firewall_hint(firewall: &str, port: u16) -> String {
    match firewall {
        "allowed" => format!("Horizon écoute sur le port {port}, en réseau privé seulement."),
        "missing" => "Windows doit autoriser Horizon à recevoir ton téléphone. Il va te demander une autorisation d’administrateur.".to_owned(),
        "blocked" => "Le pare-feu de Windows bloque Horizon. Le bouton remet la règle en ordre, avec une autorisation d’administrateur.".to_owned(),
        _ => format!("Le pare-feu de Windows est coupé ou illisible : un autre pare-feu décide. Horizon écoute sur le port {port}."),
    }
}

/// Pose l'état de la liaison dans la page.
fn refresh(view: &LinkView) {
    let Some(w) = window() else { return };
    let g = w.global::<PhonePageData>();
    g.set_enabled(crate::modules::enabled("phone"));
    g.set_firewall(view.firewall.as_str().into());
    g.set_firewall_hint(firewall_hint(&view.firewall, view.port).into());
    g.set_listening(view.listening);
    let (now, leaving) = (now_ms(), with(|p| p.leaving.clone()));
    let rows: Vec<PhoneRow> = view
        .phones
        .iter()
        .filter(|p| !leaving.contains(&p.id))
        .map(|p| PhoneRow { id: p.id.as_str().into(), name: p.name.as_str().into(), state: phone_state(p, now).into() })
        .collect();
    super::model::sync_rc(&g.get_phones(), rows, |m| g.set_phones(m));
    // L'offre a été consommée (un téléphone vient d'être appairé) ou annulée : plus de code.
    if view.pairing_ms == 0 && g.get_pairing() {
        end_pairing(&g, "");
    }
}

fn end_pairing(g: &PhonePageData, note: &str) {
    TICK.with(slint::Timer::stop);
    g.set_pairing(false);
    g.set_pair_qr(slint::Image::default());
    g.set_remaining(note.into());
}

/// Une seconde de plus : le temps restant, ou la fin de l'offre.
fn tick() {
    let (Some(app), Some(w)) = (app(), window()) else { return };
    let g = w.global::<PhonePageData>();
    match phone::pairing_left(&app) {
        0 => end_pairing(&g, "Code expiré. Affiches-en un nouveau."),
        left => g.set_remaining(remaining(left).into()),
    }
}

fn set_error(text: &str) {
    if let Some(w) = window() {
        w.global::<PhonePageData>().set_error(text.into());
    }
}

/// Retrait différé d'un téléphone : caché tout de suite, retiré à la fin du délai d'annulation.
fn remove(id: String) {
    let Some(app) = app() else { return };
    let name = phone::link(&app).phones.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default();
    with(|p| p.leaving.push(id.clone()));
    refresh(&phone::link(&app));
    let (undo_app, undo_id) = (app.clone(), id.clone());
    super::settings_window::offer_undo(Removal {
        label: format!("Téléphone « {name} » retiré"),
        undo: Box::new(move || {
            with(|p| p.leaving.retain(|l| *l != undo_id));
            refresh(&phone::link(&undo_app));
        }),
        commit: Some(Box::new(move || {
            phone::remove_phone(&app, &id);
            with(|p| p.leaving.retain(|l| *l != id));
        })),
    });
}

fn allow(app: &AppHandle) {
    if let Some(w) = window() {
        w.global::<PhonePageData>().set_busy(true);
    }
    set_error("");
    let a = app.clone();
    // L'élévation bloque jusqu'à la réponse de l'utilisateur : hors du fil de l'interface.
    crate::util::spawn_named("telephone-pare-feu", move || {
        let result = phone::allow_firewall(&a);
        let _ = a.run_on_main_thread(move || {
            if let Some(w) = window() {
                w.global::<PhonePageData>().set_busy(false);
            }
            set_error(result.err().as_deref().unwrap_or(""));
        });
    });
}

fn pair(app: &AppHandle) {
    let Some(w) = window() else { return };
    match phone::offer_pairing(app) {
        Ok(payload) => {
            // En debug seulement : le lien sert au faux téléphone du contrôle en vrai. Il porte
            // un jeton à usage unique, jamais journalisé dans une version publiée.
            if cfg!(debug_assertions) {
                log::info!("téléphone : lien d'appairage (debug) : {payload}");
            }
            let g = w.global::<PhonePageData>();
            g.set_pair_qr(qr_image(&payload, SCALE));
            g.set_remaining(remaining(phone::pairing_left(app)).into());
            g.set_pairing(true);
            g.set_error("".into());
            TICK.with(|t| t.start(slint::TimerMode::Repeated, Duration::from_secs(1), tick));
        }
        Err(e) => set_error(&e),
    }
}

fn set_threshold(app: &AppHandle, value: i32) {
    let value = value.clamp(0, 100);
    let mut settings = crate::state::settings();
    settings.alerts.phone_low = Some(value as u8);
    match crate::options::options_set(app, settings) {
        Ok(_) => {
            if let Some(w) = window() {
                w.global::<PhonePageData>().set_threshold(value);
            }
            phone::threshold_changed(app);
        }
        Err(e) => set_error(&e),
    }
}

pub fn bind(app: &AppHandle, window: &SettingsWindow) {
    with(|p| {
        p.app = Some(app.clone());
        p.window = Some(window.as_weak());
    });
    let g = window.global::<PhonePageData>();
    let a = app.clone();
    g.on_allow(move || allow(&a));
    let a = app.clone();
    g.on_pair(move || pair(&a));
    let a = app.clone();
    g.on_cancel(move || {
        phone::cancel_pairing(&a);
        if let Some(w) = self::window() {
            end_pairing(&w.global::<PhonePageData>(), "");
        }
    });
    g.on_remove(|id| remove(id.to_string()));
    let a = app.clone();
    g.on_set_threshold(move |v| set_threshold(&a, v));
    host::listen(EV_PHONE_LINK, Some(LABEL), |e| {
        if let Some(view) = e.payload::<LinkView>()
            && with(|p| p.shown)
        {
            refresh(view);
        }
    });
}

/// La page s'affiche.
pub fn show(app: &AppHandle, window: &SettingsWindow) {
    with(|p| {
        p.shown = true;
        p.leaving.clear();
    });
    let g = window.global::<PhonePageData>();
    g.set_app_qr(qr_image(APP_URL, SCALE));
    g.set_threshold(i32::from(crate::state::settings().alerts.phone_threshold().unwrap_or(0)));
    g.set_error("".into());
    g.set_busy(false);
    g.set_remaining("".into());
    window.invoke_scroll_top();
    refresh(&phone::link(app));
    // L'état du pare-feu a pu changer depuis le dernier relevé : relu hors du fil de l'interface.
    let a = app.clone();
    crate::util::spawn_named("telephone-page", move || phone::refresh(&a));
}

/// La page n'est plus affichée : l'offre d'appairage affichée tombe.
pub fn leave() {
    let was_shown = with(|p| std::mem::replace(&mut p.shown, false));
    if !was_shown {
        return;
    }
    if let (Some(app), Some(w)) = (app(), window()) {
        let g = w.global::<PhonePageData>();
        if g.get_pairing() {
            phone::cancel_pairing(&app);
            end_pairing(&g, "");
        }
    }
}

/// La fenêtre se ferme.
pub fn closing(_app: &AppHandle) {
    leave();
    with(|p| p.window = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_has_a_quiet_zone_and_scales() {
        let (w, h, px) = qr_pixels("horizon://pair?v=1&h=192.168.1.49&n=Bureau&p=28680&fp=x&t=y", 4).unwrap();
        assert_eq!(w, h);
        assert_eq!(w % 4, 0);
        assert_eq!(px.len(), (w * h * 4) as usize);
        assert!(px[..(w * 16 * 4) as usize].chunks(4).all(|p| p == [0xff, 0xff, 0xff, 0xff]), "quatre modules blancs en haut");
        assert!(px.chunks(4).any(|p| p == [0x11, 0x11, 0x1b, 0xff]));
        assert!(px.chunks(4).all(|p| p == [0xff, 0xff, 0xff, 0xff] || p == [0x11, 0x11, 0x1b, 0xff]), "deux couleurs seulement");
        let (w2, ..) = qr_pixels("horizon://pair?v=1&h=192.168.1.49&n=Bureau&p=28680&fp=x&t=y", 2).unwrap();
        assert_eq!(w, 2 * w2, "l'échelle multiplie la taille");
    }

    #[test]
    fn remaining_time_reads_in_plain_words() {
        assert_eq!(remaining(272_000), "Valable encore 4 min 32");
        assert_eq!(remaining(60_000), "Valable encore 1 min 00");
        assert_eq!(remaining(9_400), "Valable encore 9 s");
        assert_eq!(remaining(0), "Code expiré");
    }

    #[test]
    fn a_phone_row_says_when_it_was_last_seen() {
        let phone = |connected| PhoneView { id: "t".into(), name: "Pixel".into(), connected, last_seen_ms: 0, granted: Vec::new() };
        assert_eq!(phone_state(&phone(true), 3 * 3_600_000), "Connecté");
        assert_eq!(phone_state(&phone(false), 3 * 3_600_000), "Vu il y a 3 h");
        assert_eq!(phone_state(&phone(false), 20_000), "Vu à l’instant");
    }

    #[test]
    fn the_firewall_line_tells_what_to_do() {
        assert_eq!(firewall_hint("allowed", 28680), "Horizon écoute sur le port 28680, en réseau privé seulement.");
        assert!(firewall_hint("missing", 0).contains("administrateur"));
        assert!(firewall_hint("blocked", 0).contains("bloque"));
        assert!(firewall_hint("unknown", 28681).contains("28681"));
    }
}
