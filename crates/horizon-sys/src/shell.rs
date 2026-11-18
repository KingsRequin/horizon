//! Fil « shell » unique : COM en appartement mono-fil, initialisé une fois. Tout ce qui parle
//! au shell de Windows (raccourcis `.lnk`, icônes, relance d'une appli par le menu Démarrer,
//! mise au premier plan, lancement) y passe, une tâche après l'autre : les listes d'images du
//! shell supportent mal les appels simultanés, et aucun fil du runtime n'est mis en STA.
//!
//! Appelé depuis le fil shell lui-même (une tâche qui en appelle une autre), le travail est
//! fait sur place.

use crossbeam_channel::{Sender, bounded, unbounded};
use std::cell::Cell;
use std::sync::OnceLock;

type Job = Box<dyn FnOnce() + Send>;

thread_local! {
    static ON_SHELL: Cell<bool> = const { Cell::new(false) };
}

fn queue() -> Option<&'static Sender<Job>> {
    static QUEUE: OnceLock<Option<Sender<Job>>> = OnceLock::new();
    QUEUE
        .get_or_init(|| {
            let (tx, rx) = unbounded::<Job>();
            let started = std::thread::Builder::new().name("shell".into()).spawn(move || {
                ON_SHELL.set(true);
                let _com = match crate::com::ComGuard::sta() {
                    Ok(g) => Some(g),
                    Err(e) => {
                        log::error!("fil shell : COM indisponible : {e}");
                        None
                    }
                };
                for job in rx {
                    // Une tâche qui panique ne fait pas tomber le fil.
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                        log::error!("fil shell : une tâche a paniqué");
                    }
                }
            });
            match started {
                Ok(_) => Some(tx),
                Err(e) => {
                    log::error!("fil shell : démarrage impossible : {e}");
                    None
                }
            }
        })
        .as_ref()
}

/// Exécute `f` sur le fil shell et attend son résultat (`None` si le fil est indisponible ou
/// si la tâche a paniqué). Bloquant : depuis du code asynchrone, passer par `spawn_blocking`.
pub fn run<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    if ON_SHELL.get() {
        return Some(f());
    }
    let (tx, rx) = bounded(1);
    queue()?.send(Box::new(move || drop(tx.send(f())))).ok()?;
    rx.recv().ok()
}

/// Met `f` dans la file du fil shell, sans attendre.
pub fn spawn(f: impl FnOnce() + Send + 'static) {
    if ON_SHELL.get() {
        return f();
    }
    if let Some(q) = queue() {
        let _ = q.send(Box::new(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_in_order_on_one_thread_and_can_nest() {
        let a = run(|| std::thread::current().name().map(str::to_owned)).flatten();
        assert_eq!(a.as_deref(), Some("shell"));
        // Une tâche qui en demande une autre ne s'attend pas elle-même.
        assert_eq!(run(|| run(|| 2).unwrap() + 1), Some(3));
        let (tx, rx) = unbounded();
        for i in 0..5 {
            let tx = tx.clone();
            spawn(move || tx.send(i).unwrap());
        }
        assert_eq!(run(|| ()), Some(()));
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
        assert_eq!(run(|| -> () { panic!("tâche qui panique") }), None);
        assert_eq!(run(|| 7), Some(7), "le fil survit à une panique");
    }
}
