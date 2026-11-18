//! Panneau Discord vocal (global `DiscordVoicePanelData` de
//! `ui/widgets/discord_voice/panel.slint`) : salon, participants avec leur avatar (téléchargé une
//! fois, voir `ui::avatar`), boutons micro, son et « Quitter », volume et sourdine locale de
//! chaque participant (plugin 1.3, relais qui transmet). Fil de l'interface.

use super::{EV_VOICE, Voice, voice_leave, voice_state, voice_toggle, voice_user_mute, voice_user_volume};
use crate::host::{self, AppHandle};
use crate::popup::LABEL;
use crate::ui::panel_hosts::{self, Host};
use crate::ui::{DiscordVoicePanelData, DiscordVoicePanelUser, VoiceAvatarRow, model};
use crate::widgets::Panel;
use serde_json::Value;
use slint::{ModelRc, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) const PANEL: Panel = Panel::new(bind);

thread_local! {
    static USERS: Rc<VecModel<DiscordVoicePanelUser>> = Rc::new(VecModel::default());
    static LAST: RefCell<Option<Voice>> = const { RefCell::new(None) };
}

fn bind(app: &AppHandle, win: &Host) {
    let g = win.global::<DiscordVoicePanelData>();
    g.set_users(ModelRc::from(USERS.with(Rc::clone)));
    let a = app.clone();
    g.on_toggle(move |what| report(voice_toggle(a.clone(), what.to_string())));
    let a = app.clone();
    g.on_leave(move || report(voice_leave(a.clone())));
    // Le curseur va de 0 à 2 (200 %) ; Discord compte de 0 à 200.
    let a = app.clone();
    g.on_set_user_volume(move |id, v| report(voice_user_volume(a.clone(), id.to_string(), f64::from(v) * 100.0)));
    let a = app.clone();
    g.on_set_user_mute(move |id, m| report(voice_user_mute(a.clone(), id.to_string(), m)));

    LAST.set(Some(voice_state(app.clone())));
    refresh();
    // Une seule fois : les changements sont écrits dans chaque fenêtre qui montre le panneau.
    if win.is_popup() {
        host::listen(EV_VOICE, Some(LABEL), |e| {
            if let Some(v) = e.payload::<Voice>() {
                LAST.set(Some(v.clone()));
                refresh();
            }
        });
    }
}

fn report(r: Result<(), String>) {
    if let Err(e) = r {
        log::info!("panneau Discord vocal : {e}");
    }
}

/// Participant tel qu'affiché, sans son avatar (chargé à part).
#[derive(Debug, PartialEq)]
struct User {
    id: String,
    name: String,
    avatar: Option<String>,
    speaking: bool,
    mute: bool,
    deaf: bool,
    me: bool,
    /// Volume local (1 = 100 %) s'il est réglable ici.
    volume: Option<f32>,
    local_mute: bool,
}

/// Participants du message (`users`), avec le volume local seulement si la commande est
/// utilisable (`voiceCan(v, 'user_volume')`) et pour les autres que soi.
fn users(v: &Voice) -> Vec<User> {
    let can_volume = v.can.get("user_volume") == Some(&true);
    let list = v.rest.get("users").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    list.iter()
        .map(|u| {
            let me = u["self"] == true;
            User {
                id: u["id"].as_str().unwrap_or_default().to_owned(),
                name: u["name"].as_str().unwrap_or_default().to_owned(),
                avatar: u["avatar"].as_str().filter(|a| a.starts_with("https://")).map(str::to_owned),
                speaking: u["speaking"] == true,
                mute: u["mute"] == true,
                deaf: u["deaf"] == true,
                me,
                volume: u["volume"].as_f64().filter(|_| can_volume && !me).map(|x| (x / 100.0) as f32),
                local_mute: u["local_mute"] == true,
            }
        })
        .collect()
}

/// Affiche le dernier état (aussi quand un avatar arrive).
pub(super) fn refresh() {
    LAST.with_borrow(|l| {
        let Some(v) = l else { return };
        let in_call = v.available && v.in_call();
        panel_hosts::each("discord_voice", |h| {
            let g = h.global::<DiscordVoicePanelData>();
            g.set_in_call(in_call);
            g.set_guild(v.rest.get("guild").and_then(|g| g["name"].as_str()).unwrap_or("Messages privés").into());
            g.set_channel(v.channel.as_ref().and_then(|c| c["name"].as_str()).unwrap_or_default().into());
            g.set_mute(v.mute.unwrap_or(false));
            g.set_deaf(v.deaf.unwrap_or(false));
            g.set_can_leave(v.can.get("leave") == Some(&true));
        });
        let rows: Vec<DiscordVoicePanelUser> = if in_call { users(v) } else { Vec::new() }
            .into_iter()
            .map(|u| {
                let img = u.avatar.as_deref().and_then(super::ui::avatar);
                DiscordVoicePanelUser {
                    avatar: VoiceAvatarRow {
                        id: u.id.as_str().into(),
                        initials: super::ui::initials(&u.name).into(),
                        has_avatar: img.is_some(),
                        avatar: img.unwrap_or_default(),
                        speaking: u.speaking,
                        muted: u.mute || u.deaf,
                    },
                    name: u.name.as_str().into(),
                    me: u.me,
                    mute: u.mute,
                    deaf: u.deaf,
                    has_volume: u.volume.is_some(),
                    volume: u.volume.unwrap_or(1.0),
                    local_mute: u.local_mute,
                }
            })
            .collect();
        USERS.with(|m| model::sync(m, rows));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn local_volume_only_when_usable_and_for_others() {
        let msg = |can: bool| {
            json!({ "type": "voice", "available": true, "channel": { "id": "1", "name": "Général" },
                "can": { "user_volume": can },
                "users": [
                    { "id": "a", "name": "Moi", "self": true, "volume": 100 },
                    { "id": "b", "name": "Léo", "speaking": true, "volume": 140, "local_mute": true },
                    { "id": "c", "name": "Max", "mute": true, "deaf": true }
                ] })
        };
        let v: Voice = serde_json::from_value(msg(true)).unwrap();
        let u = users(&v);
        assert_eq!(u.len(), 3);
        assert!(u[0].me && u[0].volume.is_none(), "pas de volume pour soi");
        assert_eq!(u[1].volume, Some(1.4));
        assert!(u[1].local_mute && u[1].speaking);
        assert!(u[2].volume.is_none() && u[2].mute && u[2].deaf);
        let v: Voice = serde_json::from_value(msg(false)).unwrap();
        assert!(users(&v).iter().all(|u| u.volume.is_none()), "commande indisponible");
    }
}
