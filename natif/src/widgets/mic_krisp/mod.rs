//! Voyant micro (micro par défaut de Windows) et état de Krisp (relais Discord, voir `relay`).
//!
//! Repris de `app/src-tauri/src/widgets/mic_krisp.rs`. Partie interface : `ui`.

mod panel;
pub mod ui;

use crate::util::{Published, start_thread};
use horizon_sys::mic::MicState;
use serde::Serialize;
use crate::host::{AppHandle, Manager};
use crate::widgets::Backend;

/// Micro et Krisp.
pub const EV_MIC: &str = "horizon://mic";

pub const BACKEND: Backend = Backend {
    id: "mic_krisp",
    name: "micro",
    manage: |app| crate::widgets::manage(app, MicHub::default()),
    start: Some(start),
    bind: Some(ui::bind),
    panel: Some(panel::PANEL),
    ..Backend::NONE
};


#[derive(Clone, Serialize, PartialEq)]
pub struct MicPayload {
    #[serde(flatten)]
    pub mic: MicState,
    /// `None` : relais absent (Discord fermé) ou état inconnu.
    pub krisp: Option<bool>,
}

pub struct MicHub(Published<MicPayload>);

impl Default for MicHub {
    fn default() -> Self {
        let none = MicPayload { mic: MicState { muted: None, device: String::new() }, krisp: None };
        MicHub(Published::new(EV_MIC, none))
    }
}

fn hub(app: &AppHandle) -> &Published<MicPayload> {
    &app.state::<MicHub>().inner().0
}

/// État Krisp reçu du relais (`None` : relais absent, Discord fermé ou état inconnu).
pub fn on_krisp(app: &AppHandle, krisp: Option<bool>) {
    hub(app).update(app, |s| s.krisp = krisp);
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    horizon_sys::mic::watch(tx)?;
    let a = app.clone();
    start_thread("micro", move || {
        while let Ok(mic) = rx.recv() {
            hub(&a).update(&a, |s| s.mic = mic);
        }
    })
}


/// Micro coupé (`None` : aucun micro) et Krisp actif (`None` : indisponible).
pub fn status(app: &AppHandle) -> (Option<bool>, Option<bool>) {
    let s = hub(app).get();
    (s.mic.muted, s.krisp)
}

pub fn mic_state(app: AppHandle) -> MicPayload {
    hub(&app).get()
}

/// Coupe ou rouvre le micro par défaut.
pub fn mic_toggle(app: AppHandle) -> Result<(), String> {
    let Some(m) = status(&app).0 else { return Err("aucun micro".into()) };
    horizon_sys::mic::set_muted(!m).map_err(|e| e.to_string())
}

/// Bascule Krisp ; l'état affiché ne change qu'à la confirmation du relais.
pub fn krisp_toggle(app: AppHandle) -> Result<(), String> {
    let Some(k) = status(&app).1 else {
        return Err("Krisp indisponible (Discord fermé ?)".into());
    };
    crate::relay::send(&app, serde_json::json!({ "type": "set", "krisp": !k }));
    Ok(())
}
