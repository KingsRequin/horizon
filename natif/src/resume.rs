//! Sortie de veille : avis de Windows (`PowerRegisterSuspendResumeNotification`) ou, à défaut,
//! saut de l'horloge murale. Après un court délai laissé au réseau (réessayé avec un délai
//! croissant), tout ce qui vit de relectures périodiques est relancé :
//!  * les fils Rust qui dorment avec `resume::sleep` se réveillent (quota, batteries,
//!    météo) et les clients WebSocket se reconnectent ;
//!  * les caches en ligne repèrent le changement d'`epoch` ;
//!  * les fenêtres reçoivent `horizon://resume` (agenda).
//!
//! Repris tel quel de `app/src-tauri/src/resume.rs` (seul l'import de l'hôte change).

use crate::events::EV_RESUME;
use crate::util::{lock, start_thread};
use horizon_core::resume::{FIRST_WAIT, ResumeGate, ResumePlan, Step, clock_jumped};
use horizon_sys::power::PowerEvent;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};
use crate::host::{AppHandle, Emitter};

/// Pas de la surveillance de l'horloge.
const TICK: Duration = Duration::from_secs(10);
/// Hôte essayé pour savoir si le réseau est revenu (celui de la météo).
const PROBE: (&str, u16) = ("api.open-meteo.com", 443);

/// Numéro de la dernière reprise (0 = aucune depuis le démarrage).
static EPOCH: Mutex<u64> = Mutex::new(0);
static WAKE: Condvar = Condvar::new();

pub fn epoch() -> u64 {
    *lock(&EPOCH)
}

/// Dort `d`, ou moins si une reprise arrive entre-temps : renvoie alors `true`.
pub fn sleep(d: Duration) -> bool {
    let g = lock(&EPOCH);
    let start = *g;
    let (g, _) = WAKE.wait_timeout_while(g, d, |e| *e == start).expect("verrou de reprise");
    *g != start
}

fn network_ready() -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let Ok(mut addrs) = PROBE.to_socket_addrs() else { return false };
    addrs.next().is_some_and(|a| TcpStream::connect_timeout(&a, Duration::from_secs(3)).is_ok())
}

fn fire(app: &AppHandle) {
    *lock(&EPOCH) += 1;
    WAKE.notify_all();
    let _ = app.emit(EV_RESUME, ());
}

/// Démarre la surveillance sur un fil dédié.
pub fn start(app: &AppHandle) -> anyhow::Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded();
    if let Err(e) = horizon_sys::power::watch(tx) {
        log::warn!("veille : {e:#} ; seule l'horloge servira à repérer les reprises");
    }
    let app = app.clone();
    start_thread("resume", move || {
        let mut gate = ResumeGate::default();
        loop {
            let wall = SystemTime::now();
            let started = Instant::now();
            let (notice, slept) = match rx.recv_timeout(TICK) {
                Ok(PowerEvent::Resume) => (true, started.elapsed()),
                Ok(PowerEvent::Suspend) => {
                    log::info!("mise en veille");
                    (false, started.elapsed())
                }
                // Délai écoulé : le fil a dormi `TICK` de temps actif (l'horloge
                // monotone peut, elle, avoir compté la veille).
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => (false, TICK),
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(TICK);
                    (false, TICK)
                }
            };
            let jumped = clock_jumped(slept, SystemTime::now().duration_since(wall).ok());
            if !(notice || jumped) || !gate.accept(Instant::now()) {
                continue;
            }
            log::info!("sortie de veille ({})", if notice { "avis de Windows" } else { "saut d'horloge" });
            std::thread::sleep(FIRST_WAIT);
            let mut plan = ResumePlan::new();
            while let Step::Wait(d) = plan.after_check(network_ready()) {
                std::thread::sleep(d);
            }
            // Avis arrivés pendant l'attente : ils appartiennent à cette reprise.
            while rx.try_recv().is_ok() {}
            log::info!("reprise : relance des données (n° {})", epoch() + 1);
            fire(&app);
        }
    })
}

