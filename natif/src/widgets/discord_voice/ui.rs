//! Partie interface de Discord vocal (`ui/widgets/discord_voice/widget.slint`) : salon,
//! participants (ceux qui parlent d'abord, 5 au plus) et boutons micro / son
//! (`DiscordVoiceData`). Les avatars (PNG du CDN de Discord, adresse donnée par le plugin) sont
//! téléchargés une fois hors du fil de l'interface, découpés en disque et gardés ; un avatar
//! illisible laisse les initiales (`onerror` de VoiceAvatar.svelte).

use super::{EV_VOICE, Voice, voice_state, voice_toggle};
use crate::host::{self, AppHandle};
use crate::ui::model;
use crate::ui::{BarWindow, DiscordVoiceData, VoiceAvatarRow};
use horizon_core::layout::BarLayout;
use serde_json::Value;
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// Avatars montrés au plus (`MAX_AVATARS`).
const MAX_AVATARS: usize = 5;
/// Taille maximale d'un avatar téléchargé.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

enum Avatar {
    Loading,
    Ready(slint::Image),
    Broken,
}

struct Bound {
    window: slint::Weak<BarWindow>,
    users: Rc<VecModel<VoiceAvatarRow>>,
}

thread_local! {
    static BOUND: RefCell<Vec<Bound>> = RefCell::default();
    static LAST: RefCell<Option<Voice>> = const { RefCell::new(None) };
    static AVATARS: RefCell<HashMap<String, Avatar>> = RefCell::default();
}

/// Initiales d'un participant sans avatar : « Sarah K » → « SK ».
pub fn initials(name: &str) -> String {
    name.split_whitespace().filter_map(|w| w.chars().next()).flat_map(char::to_uppercase).take(2).collect()
}

/// Participant tel que l'envoie le plugin (champs dont le widget se sert).
struct User {
    id: String,
    name: String,
    avatar: Option<String>,
    speaking: bool,
    muted: bool,
}

fn users(v: &Voice) -> Vec<User> {
    let list = v.rest.get("users").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    list.iter()
        .map(|u| User {
            id: u["id"].as_str().unwrap_or_default().to_owned(),
            name: u["name"].as_str().unwrap_or_default().to_owned(),
            avatar: u["avatar"].as_str().filter(|a| a.starts_with("https://")).map(str::to_owned),
            speaking: u["speaking"] == true,
            muted: u["mute"] == true || u["deaf"] == true,
        })
        .collect()
}

/// Participants montrés : ceux qui parlent d'abord (ordre gardé sinon), 5 au plus ; et le
/// nombre de ceux qui restent.
fn shown(mut list: Vec<User>) -> (Vec<User>, usize) {
    let total = list.len();
    list.sort_by_key(|u| !u.speaking);
    list.truncate(MAX_AVATARS);
    (list, total.saturating_sub(MAX_AVATARS))
}

/// Avatar déjà là ; sinon son téléchargement part (une seule fois par adresse).
pub(super) fn avatar(url: &str) -> Option<slint::Image> {
    let start = AVATARS.with_borrow_mut(|a| match a.get(url) {
        Some(Avatar::Ready(img)) => Err(Some(img.clone())),
        Some(_) => Err(None),
        None => {
            a.insert(url.to_owned(), Avatar::Loading);
            Ok(())
        }
    });
    if let Err(ready) = start {
        return ready;
    }
    let url = url.to_owned();
    crate::util::spawn_named("discord-avatar", move || {
        let pixels = fetch(&url).inspect_err(|e| log::debug!("avatar {url} : {e}")).ok();
        let _ = slint::invoke_from_event_loop(move || {
            let slot = match pixels {
                Some(p) => Avatar::Ready(slint::Image::from_rgba8_premultiplied(p)),
                None => Avatar::Broken,
            };
            AVATARS.with_borrow_mut(|a| a.insert(url, slot));
            LAST.with_borrow(|l| l.iter().for_each(show_all));
            super::panel::refresh();
        });
    });
    None
}

/// Télécharge un avatar PNG et le découpe en disque (bord adouci), prémultiplié.
fn fetch(url: &str) -> anyhow::Result<SharedPixelBuffer<Rgba8Pixel>> {
    let mut resp = ureq::get(url)
        .header("User-Agent", "Horizon")
        .config()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .call()?;
    let bytes = resp.body_mut().with_config().limit(MAX_BYTES).read_to_vec()?;
    let (w, h, rgba) = decode_png(&bytes)?;
    Ok(round(w, h, rgba))
}

/// PNG en RGBA 8 bits (palette, gris, 16 bits convertis).
fn decode_png(bytes: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| anyhow::anyhow!("image trop grande"))?];
    let info = reader.next_frame(&mut buf)?;
    let px = (info.width * info.height) as usize;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3].chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2].chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => anyhow::bail!("palette non développée"),
    };
    Ok((info.width, info.height, rgba))
}

/// Découpe en disque inscrit (bord adouci d'un pixel), en alpha prémultiplié.
fn round(w: u32, h: u32, rgba: Vec<u8>) -> SharedPixelBuffer<Rgba8Pixel> {
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let r = w.min(h) as f32 / 2.0;
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for (i, (px, src)) in out.make_mut_slice().iter_mut().zip(rgba.chunks_exact(4)).enumerate() {
        let (x, y) = ((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5);
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        let cover = (r - d + 0.5).clamp(0.0, 1.0);
        let a = src[3] as f32 / 255.0 * cover;
        let pm = |c: u8| (c as f32 * a).round() as u8;
        *px = Rgba8Pixel { r: pm(src[0]), g: pm(src[1]), b: pm(src[2]), a: (a * 255.0).round() as u8 };
    }
    out
}

fn show_all(v: &Voice) {
    let in_call = v.available && v.in_call();
    let channel = v.channel.as_ref().and_then(|c| c["name"].as_str()).unwrap_or_default().to_owned();
    // Infobulle « serveur — salon » (`v.guild?.name ?? 'Discord'`).
    let guild = v.rest.get("guild").and_then(|g| g["name"].as_str()).unwrap_or("Discord").to_owned();
    let (list, more) = if in_call { shown(users(v)) } else { (Vec::new(), 0) };
    let rows: Vec<VoiceAvatarRow> = list
        .iter()
        .map(|u| {
            let img = u.avatar.as_deref().and_then(avatar);
            VoiceAvatarRow {
                id: u.id.as_str().into(),
                initials: initials(&u.name).into(),
                has_avatar: img.is_some(),
                avatar: img.unwrap_or_default(),
                speaking: u.speaking,
                muted: u.muted,
            }
        })
        .collect();
    BOUND.with_borrow_mut(|b| {
        b.retain(|x| x.window.upgrade().is_some());
        for x in b.iter() {
            let Some(w) = x.window.upgrade() else { continue };
            let d = w.global::<DiscordVoiceData>();
            d.set_in_call(in_call);
            d.set_channel(channel.as_str().into());
            d.set_guild(guild.as_str().into());
            d.set_more(more as i32);
            d.set_mute(v.mute.unwrap_or(false));
            d.set_deaf(v.deaf.unwrap_or(false));
            model::sync(&x.users, rows.clone());
        }
    });
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, _layout: &BarLayout) {
    let data = window.global::<DiscordVoiceData>();
    let (users, rc) = model::new();
    data.set_users(rc);
    let a = app.clone();
    data.on_toggle(move |what| {
        if let Err(e) = voice_toggle(a.clone(), what.to_string()) {
            log::info!("Discord vocal : {e}");
        }
    });
    BOUND.with_borrow_mut(|b| b.push(Bound { window: window.as_weak(), users }));
    if LAST.with_borrow(Option::is_none) {
        LAST.set(Some(voice_state(app.clone())));
    }
    LAST.with_borrow(|l| l.iter().for_each(show_all));
    host::listen(EV_VOICE, Some(label), move |e| {
        if let Some(v) = e.payload::<Voice>() {
            let changed = LAST.with_borrow(|l| l.as_ref() != Some(v));
            if changed {
                LAST.set(Some(v.clone()));
                show_all(v);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initials_of_names() {
        assert_eq!(initials("Sarah K"), "SK");
        assert_eq!(initials("léo"), "L");
        assert_eq!(initials("a b c"), "AB");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn speakers_first_and_five_at_most() {
        let users: Vec<Value> = (0..7).map(|i| json!({ "id": i.to_string(), "name": "x", "speaking": i == 6 })).collect();
        let v: Voice = serde_json::from_value(json!({ "available": true, "channel": { "name": "Général" }, "users": users })).unwrap();
        let (list, more) = shown(super::users(&v));
        assert_eq!(list.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["6", "0", "1", "2", "3"]);
        assert_eq!(more, 2);
    }

    #[test]
    fn avatars_are_cut_into_a_disc() {
        let px = round(8, 8, vec![255; 8 * 8 * 4]);
        let s = px.as_slice();
        assert_eq!(s[0].a, 0, "coin transparent");
        assert_eq!((s[4 * 8 + 4].a, s[4 * 8 + 4].r), (255, 255), "centre plein");
    }
}
