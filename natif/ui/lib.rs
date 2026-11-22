//! Interface Slint d'Horizon (`main.slint` et ce qu'il importe), dans son propre paquet : le code
//! Rust que Slint en tire est de loin le plus long à compiler, et il n'est ainsi recompilé que
//! si un `.slint` change, pas à chaque changement de `horizon-natif`.
//!
//! Rien d'autre ici : la liaison aux services est dans `horizon-natif` (`src/ui`), qui réexporte
//! [`generated`] sous `crate::ui`.

/// Fenêtres, globaux et types exportés par `main.slint`.
pub mod generated {
    slint::include_modules!();
}

include!(concat!(env!("OUT_DIR"), "/panels.rs"));
