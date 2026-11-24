//! Son en cours (WASAPI loopback) envoyé aux barres qui l'affichent, 20 fois par seconde au
//! plus : le niveau, la basse et le battement (grosse caisse, détecteur d'Essaim) pour toutes,
//! les 24 bandes seulement pour celles qui dessinent le spectre. La capture ne tourne que si une
//! barre visible est abonnée et que l'utilisateur est là (le fil dort sinon), hors mode jeu, et
//! rien n'est envoyé après une seconde de silence. Un son d'essai (`--musique-essai`) peut
//! remplacer la capture, pour voir les animations sans rien jouer.
//!
//! Repris de `app/src-tauri/src/widgets/media/audio.rs`. Différence : l'abonnement n'est plus
//! une commande de la page mais une fonction appelée par ce qui s'en sert dans une barre (le
//! widget, `ui.rs` ; la pulsation, `ui::pulse`), qui se nomme (`audio.svelte.ts` comptait ses
//! utilisateurs).

use crate::host::{AppHandle, Emitter, Manager};
use crate::util::{Waker, lock, start_thread};
use horizon_sys::audio::{AudioAnalyzer, AudioFeatures, AudioOptions, EventCounter};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Son en cours (niveau, bandes), aux seules barres abonnées.
pub const EV_AUDIO: &str = "horizon://audio";

pub const BANDS: usize = 24;
const RATE: f32 = 20.0;
const SILENCE_GRACE: Duration = Duration::from_secs(1);
/// Sans abonné, le fil dort jusqu'à un réveil (abonnement, barre réaffichée) ; ce délai n'est
/// qu'un filet de sécurité.
const IDLE_WAIT: Duration = Duration::from_secs(600);

#[derive(Default)]
pub struct AudioHub {
    /// Fenêtres abonnées : ce qui s'en sert dans chacune, et s'il veut les bandes (spectre).
    subscribers: Mutex<HashMap<String, HashMap<&'static str, bool>>>,
    /// Barres masquées (plein écran) : rien ne leur est envoyé.
    hidden: Mutex<HashSet<String>>,
    /// Son d'essai en cours (`--musique-essai`) : ce qu'il joue, son début et sa durée.
    essai: Mutex<Option<(horizon_core::music::Essai, Instant, Duration)>>,
    wake: Waker,
}

impl AudioHub {
    /// Remplace la capture par le son d'essai `kind` (`horizon_core::music::essai`) pendant
    /// `length`, mode jeu ou absence compris : rien n'est écouté pendant ce temps.
    pub fn essai(&self, kind: horizon_core::music::Essai, length: Duration) {
        *lock(&self.essai) = Some((kind, Instant::now(), length));
        self.wake.wake();
    }

    /// Son d'essai et son instant (secondes depuis son début), tant qu'il dure.
    fn essai_time(&self) -> Option<(horizon_core::music::Essai, f32)> {
        let mut essai = lock(&self.essai);
        let (kind, start, length) = (*essai)?;
        let t = start.elapsed();
        if t >= length {
            *essai = None;
            return None;
        }
        Some((kind, t.as_secs_f32()))
    }

    /// Fenêtres détruites : oubliées.
    pub fn forget(&self, labels: &[String]) {
        let mut s = lock(&self.subscribers);
        let mut h = lock(&self.hidden);
        for l in labels {
            s.remove(l);
            h.remove(l);
        }
    }

    /// La barre `label` est masquée (plein écran) : rien ne lui est envoyé, rien n'y est à dessiner.
    pub fn is_hidden(&self, label: &str) -> bool {
        lock(&self.hidden).contains(label)
    }

    pub fn set_hidden(&self, label: &str, hidden: bool) {
        let mut h = lock(&self.hidden);
        if hidden {
            h.insert(label.to_owned());
        } else {
            h.remove(label);
        }
        self.wake.wake();
    }

    /// Réveille le fil (retour de l'utilisateur, fin du mode jeu).
    pub fn wake(&self) {
        self.wake.wake();
    }

    /// `user` (« media », « pulse ») de la barre `label` s'abonne (ou se désabonne) au son ;
    /// `bands` : il dessine le spectre. La barre reste abonnée tant qu'un de ses utilisateurs
    /// l'est.
    pub fn subscribe(&self, label: &str, user: &'static str, on: bool, bands: bool) {
        {
            let mut subs = lock(&self.subscribers);
            if on {
                subs.entry(label.to_owned()).or_default().insert(user, bands);
            } else if let Some(users) = subs.get_mut(label) {
                users.remove(user);
                if users.is_empty() {
                    subs.remove(label);
                }
            }
        }
        self.wake.wake();
    }

    /// Abonnés visibles : (étiquette, veut les bandes).
    fn audience(&self) -> Vec<(String, bool)> {
        let s = lock(&self.subscribers);
        let h = lock(&self.hidden);
        s.iter().filter(|(l, _)| !h.contains(*l)).map(|(l, u)| (l.clone(), u.values().any(|b| *b))).collect()
    }
}

/// Une image du son.
#[derive(Clone, Serialize)]
pub struct AudioFrame {
    pub level: f32,
    /// Basse lissée (0–1).
    pub bass: f32,
    pub bands: Vec<f32>,
    pub beat: bool,
    /// Envoi de l'image ([`mono_ms`]) : la barre qui la reçoit sait si elle a pris du retard
    /// ([`Lateness`]).
    pub sent_ms: u64,
}

/// Millisecondes depuis le premier appel : une horloge qui ne recule ni ne saute quand l'heure
/// du système change (le retard d'une image de son se mesure avec elle).
pub fn mono_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Retard d'une barre sur le son. Une animation Slint relancée à chaque image de son ne
/// progresse que pendant les tours de boucle qui n'en reçoivent pas : interface chargée (un
/// tour plus long que l'écart entre deux images), elle resterait figée. Dès qu'une image arrive
/// en retard, le lissage est donc coupé (les valeurs sont posées telles quelles, 20 fois par
/// seconde), puis rétabli après un moment sans retard.
#[derive(Debug, Default)]
pub struct Lateness {
    last_late_ms: Option<u64>,
}

impl Lateness {
    /// Au-delà de ce délai entre l'envoi et la réception, l'image est en retard (reçue à
    /// l'heure : quelques millisecondes, 16 au plus pendant une animation).
    const LATE_MS: u64 = 35;
    /// Temps sans retard avant de rétablir le lissage.
    const CALM_MS: u64 = 2000;

    /// Image envoyée à `sent_ms` et reçue à `now_ms` : vrai si le lissage doit être coupé.
    pub fn observe(&mut self, sent_ms: u64, now_ms: u64) -> bool {
        if now_ms.saturating_sub(sent_ms) > Self::LATE_MS {
            self.last_late_ms = Some(now_ms);
        }
        self.last_late_ms.is_some_and(|t| now_ms.saturating_sub(t) < Self::CALM_MS)
    }
}

/// Envoie une image aux fenêtres de `labels`.
fn send(app: &AppHandle, labels: Vec<String>, frame: AudioFrame) {
    if labels.is_empty() {
        return;
    }
    let _ = app.emit_filter(EV_AUDIO, frame, move |l| labels.iter().any(|x| x == l));
}

pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let app = app.clone();
    start_thread("audio-hub", move || {
        let sink = Arc::new(Mutex::new(AudioFeatures::default()));
        let mut analyzer: Option<AudioAnalyzer> = None;
        // Grosses caisses vues depuis l'envoi précédent (compteur : aucune manquée ni vue deux
        // fois, l'analyse et cet envoi ne tournant pas en phase).
        let mut kicks = EventCounter::default();
        let mut quiet_since: Option<Instant> = None;
        let period = Duration::from_secs_f32(1.0 / RATE);
        let hub = app.state::<AudioHub>();
        loop {
            let audience = hub.audience();
            let essai = hub.essai_time();
            if audience.is_empty() || (essai.is_none() && (horizon_sys::presence::away() || crate::game::active())) {
                // Plus personne n'écoute (ou ne regarde, ou on joue) : capture et analyse arrêtées.
                analyzer = None;
                quiet_since = None;
                kicks = EventCounter::default();
                hub.wake.wait(IDLE_WAIT);
                continue;
            }
            std::thread::sleep(period);
            let frame = if let Some((kind, t)) = essai {
                // Son d'essai : rien n'est capturé.
                analyzer = None;
                quiet_since = None;
                kicks = EventCounter::default();
                let e = horizon_core::music::essai(kind, t, period.as_secs_f32(), BANDS);
                AudioFrame { level: e.level, bass: e.bass, bands: e.bands, beat: e.beat, sent_ms: mono_ms() }
            } else {
                if analyzer.is_none() {
                    let opts = AudioOptions { bands: BANDS, rate: RATE, kick: true };
                    match horizon_sys::audio::start(opts, sink.clone()) {
                        Ok(a) => analyzer = Some(a),
                        Err(e) => {
                            log::warn!("analyse audio impossible : {e:#}");
                            hub.wake.wait(Duration::from_secs(5));
                            continue;
                        }
                    }
                }
                let f = lock(&sink).clone();
                let beat = kicks.take(f.kick_hits) > 0;
                if f.silent {
                    let since = *quiet_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > SILENCE_GRACE {
                        continue;
                    }
                } else {
                    quiet_since = None;
                }
                AudioFrame { level: f.volume, bass: f.kick_bass, bands: f.bands, beat, sent_ms: mono_ms() }
            };
            let (spectrum, light): (Vec<_>, Vec<_>) = audience.into_iter().partition(|(_, bands)| *bands);
            let labels = |v: Vec<(String, bool)>| v.into_iter().map(|(l, _)| l).collect::<Vec<_>>();
            send(&app, labels(light), AudioFrame { bands: Vec::new(), ..frame.clone() });
            send(&app, labels(spectrum), frame);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bar_stays_subscribed_while_one_user_listens() {
        let hub = AudioHub::default();
        hub.subscribe("bar-1", "media", true, true);
        hub.subscribe("bar-1", "pulse", true, false);
        hub.subscribe("bar-2", "pulse", true, false);
        let mut a = hub.audience();
        a.sort();
        assert_eq!(a, vec![("bar-1".to_owned(), true), ("bar-2".to_owned(), false)]);
        hub.subscribe("bar-1", "media", false, false);
        a = hub.audience();
        a.sort();
        assert_eq!(a, vec![("bar-1".to_owned(), false), ("bar-2".to_owned(), false)], "plus de spectre");
        hub.set_hidden("bar-2", true);
        assert_eq!(hub.audience(), vec![("bar-1".to_owned(), false)], "barre masquée");
        hub.subscribe("bar-1", "pulse", false, false);
        assert!(hub.audience().is_empty());
    }

    #[test]
    fn smoothing_is_cut_while_frames_arrive_late() {
        let mut l = Lateness::default();
        assert!(!l.observe(1_000, 1_010), "reçue à l'heure");
        assert!(l.observe(1_050, 1_120), "70 ms de retard : lissage coupé");
        assert!(l.observe(1_150, 1_155), "encore coupé juste après");
        assert!(l.observe(3_000, 3_005), "moins de 2 s sans retard");
        assert!(!l.observe(3_200, 3_205), "2 s sans retard : rétabli");
        assert!(l.observe(4_000, 4_100) && l.observe(5_900, 5_905) && !l.observe(6_200, 6_201));
    }
}
