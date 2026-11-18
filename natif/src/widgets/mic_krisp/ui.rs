//! Partie interface de Micro et Krisp (`ui/widgets/mic_krisp/widget.slint`) : micro de Windows,
//! Krisp, appel Discord en cours (`MicKrispData`) et option `discordMute` de chaque exemplaire
//! (lue dans `layout.json`).

use super::{EV_MIC, MicPayload, krisp_toggle, mic_state, mic_toggle};
use crate::host::{self, AppHandle};
use crate::ui::{BarWindow, MicKrispData};
use crate::widgets::discord_voice::{EV_VOICE, Voice, voice_state, voice_toggle};
use horizon_core::layout::{BarLayout, WidgetKind};
use slint::ComponentHandle;

fn tri(v: Option<bool>) -> i32 {
    v.map_or(-1, i32::from)
}

/// « CABLE Output (VB-Audio Virtual Cable) » → « CABLE Output » (`shortName` du widget Tauri).
fn short_name(device: &str) -> &str {
    let short = match device.trim_end().strip_suffix(')').and_then(|d| d.find('(').map(|i| &d[..i])) {
        Some(head) => head.trim_end(),
        None => device,
    };
    if short.is_empty() { device } else { short }
}

fn show_mic(w: &BarWindow, m: &MicPayload) {
    let d = w.global::<MicKrispData>();
    d.set_muted(tri(m.mic.muted));
    d.set_krisp(tri(m.krisp));
    d.set_device(short_name(&m.mic.device).into());
}

fn show_voice(w: &BarWindow, v: &Voice) {
    let d = w.global::<MicKrispData>();
    d.set_in_call(v.available && v.in_call());
    d.set_voice_mute(v.mute.unwrap_or(false));
}

/// Exemplaires de la barre qui ont l'option « le bouton coupe le micro de Discord ».
fn discord_mute_ids(layout: &BarLayout) -> Vec<String> {
    [&layout.left, &layout.center, &layout.right]
        .into_iter()
        .flatten()
        .filter(|w| w.kind == WidgetKind::MIC_KRISP && w.settings["discordMute"] == true)
        .map(|w| w.id.clone())
        .collect()
}

pub fn bind(app: &AppHandle, window: &BarWindow, label: &str, layout: &BarLayout) {
    let data = window.global::<MicKrispData>();
    data.set_personal(crate::PERSONAL);
    let ids = discord_mute_ids(layout);
    data.on_discord_mute(move |id| ids.iter().any(|i| i == id.as_str()));
    let a = app.clone();
    data.on_toggle_mic(move |via_discord| {
        // Micro de Windows : COM, hors du fil de l'interface.
        let a = a.clone();
        crate::util::spawn_named("micro-bascule", move || {
            let r = if via_discord { voice_toggle(a, "mute".into()) } else { mic_toggle(a) };
            if let Err(e) = r {
                log::info!("bouton micro : {e}");
            }
        });
    });
    let a = app.clone();
    data.on_toggle_krisp(move || {
        if let Err(e) = krisp_toggle(a.clone()) {
            log::info!("Krisp : {e}");
        }
    });
    show_mic(window, &mic_state(app.clone()));
    show_voice(window, &voice_state(app.clone()));
    let w = window.as_weak();
    host::listen(EV_MIC, Some(label), move |e| {
        if let (Some(m), Some(w)) = (e.payload::<MicPayload>(), w.upgrade()) {
            show_mic(&w, m);
        }
    });
    let w = window.as_weak();
    host::listen(EV_VOICE, Some(label), move |e| {
        if let (Some(v), Some(w)) = (e.payload::<Voice>(), w.upgrade()) {
            show_voice(&w, v);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_are_shortened_like_the_widget() {
        assert_eq!(short_name("CABLE Output (VB-Audio Virtual Cable)"), "CABLE Output");
        assert_eq!(short_name("Micro (USB (2))"), "Micro");
        assert_eq!(short_name("Maono"), "Maono");
        assert_eq!(short_name("(Inconnu)"), "(Inconnu)", "rien avant la parenthèse : nom gardé");
    }

    #[test]
    fn discord_mute_option_is_read_per_instance() {
        let layout: BarLayout = serde_json::from_value(serde_json::json!({
            "monitor_id": "m", "monitor_pos": [0, 0], "role": "main", "style": "islands",
            "left": [
                { "id": "a", "kind": "mic_krisp", "settings": { "discordMute": true } },
                { "id": "b", "kind": "mic_krisp", "settings": null },
                { "id": "c", "kind": "clock", "settings": { "discordMute": true } }
            ],
            "center": [], "right": []
        }))
        .unwrap();
        assert_eq!(discord_mute_ids(&layout), ["a"]);
        assert_eq!(tri(None), -1);
        assert_eq!((tri(Some(false)), tri(Some(true))), (0, 1));
    }
}
