//! Suggestions selon l'usage : sites (historique Vivaldi) et applis (journal du premier plan),
//! proposés sur l'écran où l'utilisateur les ouvre habituellement à ce moment-là.

pub mod keys;
pub mod history;
pub mod score;
pub mod store;

pub use keys::ItemKey;
