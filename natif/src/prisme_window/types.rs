//! Formes JSON échangées avec le moteur Prisme (repris de `apps/prisme/lib/types.ts` ; côté
//! moteur : `crates/daemon/src/{ipc,config}.rs` du dépôt `rgb-engine`). La configuration reste
//! un `serde_json::Value` (voir `config.rs`) : ce que l'interface ne connaît pas est rendu tel
//! quel au moteur.

use serde::Deserialize;

/// État du moteur (`{"event":"state","data":{"daemon":…}}`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DaemonState {
    pub scene: String,
    pub effect: String,
    pub reason: String,
    pub override_scene: Option<String>,
    pub brightness: f32,
    pub monitor: Option<String>,
    pub foreground: Option<String>,
    pub video: Option<String>,
    pub music: Option<String>,
    pub scenes: Vec<String>,
    pub screens: Vec<Screen>,
    /// Révision de la config (change à chaque modification, d'où qu'elle vienne).
    pub config_rev: u64,
}

/// Écran dans le plan du bureau (unité = largeur de l'écran principal).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Screen {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub primary: bool,
}

impl Screen {
    /// Écran de repli quand le moteur n'en annonce aucun (16/9, principal).
    pub fn fallback() -> Screen {
        Screen { name: String::new(), x: 0.0, y: 0.0, w: 1.0, h: 9.0 / 16.0, primary: true }
    }
}

/// État d'un appareil (`data.devices` de l'événement `state`).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeviceStatus {
    pub id: String,
    /// Nom choisi par l'utilisateur (vide = aucun).
    pub label: String,
    /// Nom annoncé par le matériel.
    pub name: String,
    pub driver: String,
    pub enabled: bool,
    pub connected: bool,
    pub error: Option<String>,
    pub leds: u32,
    /// (nom, nombre de LED)
    pub zones: Vec<(String, u32)>,
    pub fps: f32,
}

/// Image de prévisualisation (`{"event":"preview"}`, ≤ 15 par seconde).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Preview {
    /// Positions dans le plan du bureau.
    pub positions: Vec<[f64; 2]>,
    pub screens: Vec<Screen>,
    pub active_screen: Option<String>,
    pub colors: Vec<[u8; 3]>,
    /// (id, début, fin)
    pub devices: Vec<(String, usize, usize)>,
}
