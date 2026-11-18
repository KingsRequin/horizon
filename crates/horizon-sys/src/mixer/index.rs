//! Index des applis du menu Démarrer : pour chaque exe lancé par un raccourci, son nom affiché
//! (nom du raccourci) et l'AUMID que déclare ce raccourci. Sert à nommer les sessions audio et à
//! relier la lecture en cours (dont l'identifiant est souvent cet AUMID) à son exe.
//!
//! Construit une fois, sur un fil neuf en appartement mono-fil (objets du shell).

use horizon_core::apps::lnk_exe_key;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub struct AppIndex {
    by_key: HashMap<String, Entry>,
}

#[derive(Debug, Default)]
struct Entry {
    name: String,
    aumids: Vec<String>,
}

impl AppIndex {
    pub fn name(&self, key: &str) -> Option<&str> {
        self.by_key.get(key).map(|e| e.name.as_str())
    }

    pub fn aumids(&self, key: &str) -> &[String] {
        self.by_key.get(key).map_or(&[], |e| &e.aumids)
    }
}

/// Parcourt le menu Démarrer (sur le fil shell : raccourcis lus par COM).
fn scan() -> AppIndex {
    let mut idx = AppIndex::default();
    for lnk in crate::startmenu::shortcuts() {
        let path = lnk.to_string_lossy();
        let Some(stem) = lnk.file_stem().map(|s| s.to_string_lossy().to_string()) else { continue };
        if horizon_core::apps::is_noise(&stem) {
            continue;
        }
        let Some(key) = crate::foreground::lnk_info(&path).and_then(|(t, a)| lnk_exe_key(&t, &a)) else {
            continue;
        };
        let e = idx.by_key.entry(key).or_default();
        if e.name.is_empty() {
            e.name = stem;
        }
        if let Some(a) = crate::foreground::lnk_aumid(&path)
            && !e.aumids.contains(&a)
        {
            e.aumids.push(a);
        }
    }
    idx
}

/// Construit l'index sur le fil shell et attend le résultat.
pub fn build() -> AppIndex {
    crate::shell::run(scan).unwrap_or_default()
}
