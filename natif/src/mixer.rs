//! Mixeur audio partagé par les panneaux (volume de l'appli dans le panneau Lecture en cours,
//! panneau Audio du widget Micro et Krisp) : état du fil audio envoyé à la fenêtre surgissante,
//! favoris épinglés (`%APPDATA%\Horizon\audio.json`), commandes.
//!
//! Repris de `app/src-tauri/src/mixer.rs`. Différences : les commandes Tauri sont des fonctions ;
//! le suivi (`watch`) compte les panneaux qui s'en servent (Tauri : `useMixer` de
//! lib/mixer.svelte.ts).

use crate::events::EV_MIXER;
use crate::host::{AppHandle, Manager};
use crate::util::{Published, lock};
use horizon_core::mixer::{Device, MixerSnapshot, Pins, media_app, next_device, sort_devices};
use horizon_sys::mixer::{MixerCommand, MixerHandle};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Mixer {
    handle: Mutex<Option<MixerHandle>>,
    /// Dernier état brut reçu du fil audio (avant favoris).
    raw: Mutex<MixerSnapshot>,
    /// État envoyé à la fenêtre surgissante (seulement s'il change).
    shown: Published<MixerView>,
    pins: Mutex<Option<Pins>>,
    pins_writer: horizon_core::config::OrderedWriter,
    /// Panneaux ouverts qui affichent l'état (le fil ne relit l'état que pendant ce temps).
    watchers: Mutex<u32>,
}

impl Default for Mixer {
    fn default() -> Self {
        Mixer {
            handle: Mutex::default(),
            raw: Mutex::default(),
            shown: Published::to(EV_MIXER, crate::popup::LABEL, MixerView::default()),
            pins: Mutex::default(),
            pins_writer: Default::default(),
            watchers: Mutex::default(),
        }
    }
}

/// État envoyé aux panneaux.
#[derive(Clone, Default, PartialEq, Serialize)]
pub struct MixerView {
    #[serde(flatten)]
    pub snapshot: MixerSnapshot,
    /// Appli (clé d'exe) qui porte la lecture en cours, si elle a pu être reliée à sa session
    /// audio ; sinon le panneau média n'affiche pas de curseur.
    pub media_key: Option<String>,
}

impl MixerView {
    /// Appli de la lecture en cours (volume du panneau média).
    pub fn media_app(&self) -> Option<&horizon_core::mixer::AppAudio> {
        let key = self.media_key.as_deref()?;
        self.snapshot.apps.iter().find(|a| a.key == key)
    }
}

fn pins_path() -> PathBuf {
    crate::state::app_dir().join("audio.json")
}

impl Mixer {
    /// Favoris (lus au premier appel), sous leur verrou.
    fn with_pins<T>(&self, f: impl FnOnce(&mut Pins) -> T) -> T {
        let mut p = lock(&self.pins);
        f(p.get_or_insert_with(|| {
            std::fs::read_to_string(pins_path())
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        }))
    }

    fn pins(&self) -> Pins {
        self.with_pins(|p| p.clone())
    }

    /// Envoie une commande au fil audio ; erreur s'il n'a pas pu démarrer.
    fn send(&self, c: MixerCommand) -> Result<(), String> {
        let handle = lock(&self.handle);
        let h = handle.as_ref().ok_or("panneau audio indisponible")?;
        h.send(c);
        Ok(())
    }
}

/// État affiché : favoris en tête, appli de la lecture en cours reliée.
pub fn view(app: &AppHandle) -> MixerView {
    let m = app.state::<Mixer>();
    let raw = lock(&m.raw).clone();
    view_of(raw, &m.pins().pinned, crate::widgets::media::current_app_id(app).as_deref())
}

/// Vue d'un état brut : favoris `pins` en tête, appli de la lecture en cours (AUMID SMTC
/// `media_app_id`) reliée à sa session audio si possible.
fn view_of(mut s: MixerSnapshot, pins: &[String], media_app_id: Option<&str>) -> MixerView {
    sort_devices(&mut s.outputs, pins);
    sort_devices(&mut s.inputs, pins);
    let media_key = media_app_id.and_then(|id| media_app(id, &s.apps).map(|a| a.key.clone()));
    MixerView { snapshot: s, media_key }
}

/// Envoie l'état au panneau s'il a changé (aussi quand la lecture en cours change d'appli).
pub fn publish(app: &AppHandle) {
    app.state::<Mixer>().shown.set(app, view(app));
    // Volume général et mixeur, pour les téléphones qui les regardent (module Téléphone).
    crate::widgets::phone::mixer_changed(app);
}

/// Sorties (ordre d'affichage, favoris en tête) et sortie par défaut, d'après la dernière
/// lecture ; une relecture est demandée pour la fois suivante.
pub fn outputs(app: &AppHandle) -> (Vec<Device>, Option<String>) {
    let v = view(app);
    let _ = app.state::<Mixer>().send(MixerCommand::Refresh);
    (v.snapshot.outputs, v.snapshot.default_output)
}

/// Micros (favoris en tête) et micro par défaut, comme `outputs`.
pub fn inputs(app: &AppHandle) -> (Vec<Device>, Option<String>) {
    let v = view(app);
    let _ = app.state::<Mixer>().send(MixerCommand::Refresh);
    (v.snapshot.inputs, v.snapshot.default_input)
}

/// Ce périphérique devient le défaut ; l'état est relu aussitôt.
pub fn set_default(app: &AppHandle, id: String) -> Result<(), String> {
    let m = app.state::<Mixer>();
    m.send(MixerCommand::SetDefault(id))?;
    m.send(MixerCommand::Refresh)
}

/// Passe à la sortie suivante (favoris s'il y en a au moins deux) ; renvoie son nom.
pub fn next_output(app: &AppHandle) -> Result<String, String> {
    let v = view(app);
    let next = next_device(&v.snapshot.outputs, v.snapshot.default_output.as_deref()).ok_or("pas d’autre sortie audio")?;
    set_default(app, next.id.clone())?;
    Ok(next.name.clone())
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let a = app.clone();
    let h = horizon_sys::mixer::start(move |s| {
        *lock(&a.state::<Mixer>().raw) = s;
        publish(&a);
    })?;
    // Première lecture des périphériques (palette de commandes).
    h.send(MixerCommand::Refresh);
    *lock(&app.state::<Mixer>().handle) = Some(h);
    Ok(())
}

/// Un panneau qui affiche l'état audio s'ouvre (`true`) ou se ferme : le fil audio ne relit
/// l'état que tant qu'au moins un panneau s'en sert.
pub fn watch(app: &AppHandle, on: bool) {
    let m = app.state::<Mixer>();
    let mut n = lock(&m.watchers);
    let before = *n;
    *n = if on { before + 1 } else { before.saturating_sub(1) };
    if (before == 0) != (*n == 0)
        && let Err(e) = m.send(MixerCommand::Watch(*n > 0))
    {
        log::warn!("mixeur : {e}");
    }
}

pub fn set_master(app: &AppHandle, volume: f32) -> Result<(), String> {
    app.state::<Mixer>().send(MixerCommand::SetMasterVolume(volume))
}

pub fn set_master_mute(app: &AppHandle, muted: bool) -> Result<(), String> {
    app.state::<Mixer>().send(MixerCommand::SetMasterMute(muted))
}

pub fn set_app_volume(app: &AppHandle, key: String, volume: f32) -> Result<(), String> {
    app.state::<Mixer>().send(MixerCommand::SetAppVolume { key, volume })
}

pub fn set_app_mute(app: &AppHandle, key: String, muted: bool) -> Result<(), String> {
    app.state::<Mixer>().send(MixerCommand::SetAppMute { key, muted })
}

/// Épingle (ou désépingle) un périphérique en tête de sa liste.
pub fn pin(app: &AppHandle, id: String, pinned: bool) -> Result<(), String> {
    let m = app.state::<Mixer>();
    // Modifiés sous leur verrou (deux épingles simultanées ne s'écrasent pas), écrits après.
    let saved = m.with_pins(|pins| {
        pins.set(&id, pinned).then(|| (m.pins_writer.ticket(), serde_json::to_string_pretty(pins)))
    });
    let Some((ticket, text)) = saved else { return Ok(()) };
    publish(app);
    let text = text.map_err(|e| e.to_string())?;
    m.pins_writer.write(&pins_path(), ticket, text.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_core::mixer::{AppAudio, Flow, Level};

    fn dev(id: &str, name: &str) -> Device {
        Device { id: id.into(), name: name.into(), flow: Flow::Output, pinned: false }
    }

    #[test]
    fn view_puts_pins_first_and_links_the_playing_app() {
        let raw = MixerSnapshot {
            outputs: vec![dev("a", "Alpha"), dev("b", "Bêta"), dev("c", "Casque")],
            apps: vec![AppAudio {
                key: "spotify.exe".into(),
                name: "Spotify".into(),
                exe: String::new(),
                level: Level { volume: 1.0, muted: false },
                active: true,
                aumids: vec![],
            }],
            ..MixerSnapshot::default()
        };
        let v = view_of(raw.clone(), &["c".into()], Some("Spotify.exe"));
        let ids: Vec<&str> = v.snapshot.outputs.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
        assert!(v.snapshot.outputs[0].pinned);
        assert_eq!(v.media_key.as_deref(), Some("spotify.exe"));
        assert_eq!(v.media_app().map(|a| a.name.as_str()), Some("Spotify"));
        let none = view_of(raw, &[], None);
        assert_eq!(none.media_key, None);
        assert!(none.media_app().is_none());
    }
}
