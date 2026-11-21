//! Temps processeur du fil appelant : ce que coûte un travail, sans le temps où le fil attendait
//! (préemption par un jeu, gel du pilote graphique, mémoire relue depuis le disque).
//!
//! `QueryThreadCycleTime` compte les cycles du compteur d'horodatage (TSC) passés par le fil ;
//! leur fréquence est mesurée contre l'horloge depuis le premier usage ([`calibrate`]). Tant
//! qu'elle n'est pas connue (moins de 200 ms), ou hors x86-64, le temps écoulé sert de mesure.

use std::time::{Duration, Instant};
use windows::Win32::System::Threading::GetCurrentThread;
use windows::Win32::System::WindowsProgramming::QueryThreadCycleTime;

/// Mesure commencée par [`CpuMeter::start`].
pub struct CpuMeter {
    wall: Instant,
    cycles: Option<u64>,
}

impl CpuMeter {
    pub fn start() -> CpuMeter {
        CpuMeter { cycles: thread_cycles(), wall: Instant::now() }
    }

    /// Temps processeur du fil depuis [`CpuMeter::start`], jamais plus que le temps écoulé.
    pub fn elapsed(&self) -> Duration {
        let wall = self.wall.elapsed();
        match (self.cycles, thread_cycles(), cycles_per_us()) {
            (Some(a), Some(b), Some(rate)) => Duration::from_micros((b.saturating_sub(a) as f64 / rate) as u64).min(wall),
            _ => wall,
        }
    }
}

/// Lance la mesure de la fréquence du TSC (à appeler tôt : elle sert après 200 ms).
pub fn calibrate() {
    let _ = cycles_per_us();
}

fn thread_cycles() -> Option<u64> {
    let mut cycles = 0u64;
    // SAFETY : pseudo-handle du fil courant, pointeur vers une variable locale.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) }.ok().map(|()| cycles)
}

/// Cycles du TSC par µs, mesurés depuis le premier appel (`None` les 200 premières ms).
fn cycles_per_us() -> Option<f64> {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::OnceLock;
        static ORIGIN: OnceLock<(u64, Instant)> = OnceLock::new();
        let (tsc0, t0) = *ORIGIN.get_or_init(|| (tsc(), Instant::now()));
        let us = t0.elapsed().as_micros();
        (us >= 200_000).then(|| tsc().saturating_sub(tsc0) as f64 / us as f64)
    }
    #[cfg(not(target_arch = "x86_64"))]
    None
}

#[cfg(target_arch = "x86_64")]
fn tsc() -> u64 {
    // SAFETY : instruction toujours présente en x86-64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_work_not_waiting() {
        calibrate();
        std::thread::sleep(Duration::from_millis(250));
        let m = CpuMeter::start();
        std::thread::sleep(Duration::from_millis(60));
        let waited = m.elapsed();
        assert!(waited < Duration::from_millis(20), "attente comptée : {waited:?}");

        let m = CpuMeter::start();
        let t = Instant::now();
        let mut x = 0u64;
        while t.elapsed() < Duration::from_millis(60) {
            x = std::hint::black_box(x.wrapping_add(1));
        }
        let worked = m.elapsed();
        assert!(worked > Duration::from_millis(30) && worked <= t.elapsed(), "travail mal compté : {worked:?}");
    }
}
