//! Relance d'un fil système après une erreur Windows (service audio redémarré, WinRT qui ne
//! répond pas à temps…) : délai croissant (`Backoff`), remis à zéro après une période stable.
//! Chaque panne est notée (`faults`) pour la page « État » des réglages. Et relance d'Horizon
//! entier par Windows après un plantage du processus (`relaunch_after_crash`).

use horizon_core::backoff::Backoff;
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::System::Recovery::{RESTART_NO_PATCH, RESTART_NO_REBOOT, RegisterApplicationRestart};
use windows::core::PCWSTR;

/// Demande à Windows de relancer Horizon s'il plante ou ne répond plus (après au moins 60 s de
/// fonctionnement, règle de Windows). Un plantage dans du code natif (pilote, DLL système) ne
/// passe pas par le gestionnaire de panique : sans cela, les barres disparaissaient jusqu'au
/// prochain démarrage. Rien après une mise à jour ni un redémarrage du PC (le lancement au
/// démarrage s'en charge).
pub fn relaunch_after_crash() {
    // SAFETY : ligne de commande nulle (relance sans argument), drapeaux valides.
    if let Err(e) = unsafe { RegisterApplicationRestart(PCWSTR::null(), RESTART_NO_PATCH | RESTART_NO_REBOOT) } {
        log::warn!("relance automatique après plantage indisponible : {e}");
    }
}

/// Panne d'un fil système relancé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fault {
    /// Fonction concernée (« lecture en cours »…).
    pub what: String,
    pub error: String,
    /// Heure de la dernière panne (ms Unix).
    pub at_ms: u64,
    /// Pannes depuis le démarrage.
    pub count: u32,
    /// Vrai pendant l'attente avant la relance : la fonction est indisponible.
    pub down: bool,
}

static FAULTS: Mutex<Vec<Fault>> = Mutex::new(Vec::new());

fn note(what: &str, f: impl FnOnce(&mut Fault)) {
    let mut all = FAULTS.lock().unwrap_or_else(|e| e.into_inner());
    let i = match all.iter().position(|x| x.what == what) {
        Some(i) => i,
        None => {
            all.push(Fault { what: what.to_owned(), error: String::new(), at_ms: 0, count: 0, down: false });
            all.len() - 1
        }
    };
    f(&mut all[i]);
}

/// Pannes notées depuis le démarrage (fonctions relancées au moins une fois).
pub fn faults() -> Vec<Fault> {
    FAULTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Exécute `body` jusqu'à ce qu'il se termine sans erreur (fin normale : canal fermé…). Après
/// chaque erreur, `on_error` signale l'état « indisponible » (faux = arrêter), puis `body` est
/// relancé après un délai croissant.
pub(crate) fn with_restart(what: &str, mut body: impl FnMut() -> anyhow::Result<()>, mut on_error: impl FnMut() -> bool) {
    let mut backoff = Backoff::new();
    loop {
        let started = Instant::now();
        let Err(e) = body() else { return };
        backoff.connection_closed(started.elapsed());
        let delay = backoff.next_delay();
        log::warn!("{what} interrompu : {e:#} ; nouvel essai dans {} s", delay.as_secs());
        note(what, |f| {
            (f.error, f.at_ms, f.down) = (format!("{e:#}"), horizon_core::time::now_ms(), true);
            f.count += 1;
        });
        if !on_error() {
            return;
        }
        std::thread::sleep(delay);
        note(what, |f| f.down = false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_is_restarted_after_errors_until_it_ends() {
        let mut runs = 0;
        let mut errors = 0;
        with_restart(
            "essai",
            || {
                runs += 1;
                if runs < 2 { anyhow::bail!("panne") } else { Ok(()) }
            },
            || {
                errors += 1;
                true
            },
        );
        assert_eq!((runs, errors), (2, 1));
        let f = faults().into_iter().find(|f| f.what == "essai").unwrap();
        assert!(f.count >= 1 && f.error == "panne");
    }

    #[test]
    fn on_error_can_stop_the_loop() {
        let mut runs = 0;
        with_restart(
            "essai",
            || {
                runs += 1;
                anyhow::bail!("panne")
            },
            || false,
        );
        assert_eq!(runs, 1);
    }
}
