//! Capture de la sortie son (WASAPI loopback) et analyse spectrale : la bibliothèque partagée
//! `audio-commun` (aussi utilisée par Prisme et Essaim), sous les noms d'Horizon.
//!
//! Les événements (grosses caisses…) sont des compteurs : les lire avec [`EventCounter`].

use std::sync::{Arc, Mutex};

pub use audio_commun::spectre::{AudioFeatures, EventCounter, Options as AudioOptions};

/// Analyse en cours ; l'arrêter = la `drop`.
pub type AudioAnalyzer = audio_commun::capture::Capture;

/// Lance capture et analyse ; `sink` reçoit chaque image d'analyse.
pub fn start(opts: AudioOptions, sink: Arc<Mutex<AudioFeatures>>) -> anyhow::Result<AudioAnalyzer> {
    audio_commun::spectre::start("audio", opts, sink)
}
