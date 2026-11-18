//! Réglages Agenda (`ui/widget-settings/calendar.slint`, app/src/widgets/calendar/Settings.svelte
//! et `calendar.ts`) : adresses iCal (`settings.urls`), nom donné (`settings.names`) et couleur
//! gardée (`settings.colors`) par adresse ; chaque agenda est lu à l'ouverture et par « Tester »
//! (`calendar::calendar_test`, hors du fil de l'interface) ; retrait annulable.

use crate::ui::settings_window::offer_undo;
use crate::ui::{CalendarRow, CalendarSettingsData, SettingsWindow};
use crate::undoable::Removal;
use crate::widgets::calendar::{CalendarTest, calendar_test, urls_of};
use serde_json::{Map, Value, json};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

/// Nombre de couleurs d'agenda (`Theme.calendars`).
const CAL_COLORS: u64 = 6;

/// Résultat du dernier test d'une adresse.
#[derive(Debug, Clone, PartialEq)]
enum Test {
    Pending,
    Ok(CalendarTest),
    Error(String),
}

thread_local! {
    static TESTS: RefCell<HashMap<String, Test>> = RefCell::default();
    static DRAFT: RefCell<String> = const { RefCell::new(String::new()) };
    /// Numéro de l'ouverture : un test d'une ouverture précédente est ignoré.
    static OPENING: Cell<u64> = const { Cell::new(0) };
}

/// Agendas réglés : adresses, noms et couleurs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Calendars {
    pub urls: Vec<String>,
    pub names: BTreeMap<String, String>,
    pub colors: BTreeMap<String, u64>,
}

impl Calendars {
    /// Lit les réglages ; une adresse sans couleur enregistrée prend celle de son rang
    /// (`colorsOf`).
    pub fn of(settings: &Value) -> Calendars {
        let urls = urls_of(settings);
        let names = settings["names"]
            .as_object()
            .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect())
            .unwrap_or_default();
        let colors = urls
            .iter()
            .enumerate()
            .map(|(rank, u)| (u.clone(), settings["colors"].get(u).and_then(Value::as_u64).unwrap_or(rank as u64 % CAL_COLORS)))
            .collect();
        Calendars { urls, names, colors }
    }

    /// Champs à enregistrer : seuls les noms et couleurs des adresses gardées.
    pub fn patch(&self) -> Value {
        let keep = |m: Map<String, Value>| Value::Object(m.into_iter().filter(|(u, _)| self.urls.contains(u)).collect());
        json!({
            "urls": self.urls,
            "names": keep(self.names.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
            "colors": keep(self.colors.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
        })
    }

    /// Première couleur libre pour un nouvel agenda (`freeColor`).
    pub fn free_color(&self) -> u64 {
        (0..CAL_COLORS).find(|n| !self.colors.values().any(|c| c == n)).unwrap_or(self.colors.len() as u64 % CAL_COLORS)
    }

    /// Ajoute une adresse (vide ou déjà là : refusée).
    pub fn add(&mut self, url: &str) -> bool {
        let u = url.trim();
        if u.is_empty() || self.urls.iter().any(|x| x == u) {
            return false;
        }
        let color = self.free_color();
        self.urls.push(u.to_owned());
        self.colors.insert(u.to_owned(), color);
        true
    }

    /// Nom donné à un agenda (vide : retiré, le nom lu dans l'agenda revient).
    pub fn rename(&mut self, url: &str, name: &str) {
        match name.trim() {
            "" => self.names.remove(url),
            n => self.names.insert(url.to_owned(), n.to_owned()),
        };
    }
}

/// L'adresse secrète donne accès à l'agenda : seuls l'hôte et la fin sont montrés.
pub fn masked(url: &str) -> String {
    let rest = ["webcal://", "https://", "http://"].iter().find_map(|p| url.strip_prefix(p)).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let chars: Vec<char> = url.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(10)..].iter().collect();
    format!("{host}/…{tail}")
}

/// État d'une lecture pour l'utilisateur.
fn status(t: Option<&Test>) -> String {
    match t {
        None | Some(Test::Pending) => "lecture…".into(),
        Some(Test::Error(e)) => e.clone(),
        Some(Test::Ok(r)) => match r.events {
            0 => "lu · aucun événement d’hier à dans 8 jours".into(),
            1 => "lu · 1 événement d’hier à dans 8 jours".into(),
            n => format!("lu · {n} événements d’hier à dans 8 jours"),
        },
    }
}

fn current() -> Calendars {
    Calendars::of(&super::settings())
}

fn refresh() {
    let Some(w) = super::window() else { return };
    let cals = current();
    let rows: Vec<CalendarRow> = TESTS.with_borrow(|tests| {
        cals.urls
            .iter()
            .enumerate()
            .map(|(i, u)| {
                let t = tests.get(u);
                let read_name = match t {
                    Some(Test::Ok(r)) => r.name.clone().filter(|n| !n.is_empty()),
                    _ => None,
                };
                let name = cals.names.get(u).cloned().unwrap_or_default();
                CalendarRow {
                    color: cals.colors.get(u).copied().unwrap_or(i as u64) as i32,
                    name: name.as_str().into(),
                    placeholder: read_name.unwrap_or_else(|| format!("Agenda {}", i + 1)).into(),
                    masked: masked(u).into(),
                    status: status(t).into(),
                    state: match t {
                        Some(Test::Ok(_)) => 1,
                        Some(Test::Error(_)) => 2,
                        _ => 0,
                    },
                    pending: matches!(t, Some(Test::Pending)),
                    what: format!("l’agenda {}", if name.is_empty() { (i + 1).to_string() } else { name }).into(),
                }
            })
            .collect()
    });
    let g = w.global::<CalendarSettingsData>();
    crate::ui::model::sync_rc(&g.get_rows(), rows, |m| g.set_rows(m));
    update_draft(&g, &cals);
}

fn update_draft(g: &CalendarSettingsData, cals: &Calendars) {
    let draft = DRAFT.with_borrow(|d| d.trim().to_owned());
    let duplicate = cals.urls.contains(&draft);
    g.set_duplicate(duplicate);
    g.set_can_add(!draft.is_empty() && !duplicate);
}

pub fn bind(window: &SettingsWindow) {
    let g = window.global::<CalendarSettingsData>();
    g.on_draft(|t| {
        DRAFT.set(t.to_string());
        if let Some(w) = super::window() {
            update_draft(&w.global::<CalendarSettingsData>(), &current());
        }
    });
    g.on_add(add);
    g.on_rename(|i, name| {
        let mut cals = current();
        if let Some(u) = cals.urls.get(i.max(0) as usize).cloned() {
            cals.rename(&u, &name);
            save(&cals);
        }
    });
    g.on_test(|i| {
        if let Some(u) = current().urls.get(i.max(0) as usize) {
            test(u);
        }
    });
    g.on_remove(|i| remove(i.max(0) as usize));
}

/// La page s'affiche : chaque agenda est lu une fois.
pub fn show(window: &SettingsWindow, settings: &Value) {
    OPENING.set(OPENING.get() + 1);
    TESTS.set(HashMap::new());
    DRAFT.set(String::new());
    let g = window.global::<CalendarSettingsData>();
    g.set_rows(ModelRc::new(VecModel::default()));
    g.set_cleared(g.get_cleared() + 1);
    for u in Calendars::of(settings).urls {
        test(&u);
    }
    refresh();
}

pub fn leave() {
    OPENING.set(OPENING.get() + 1);
}

fn save(cals: &Calendars) {
    super::save(cals.patch());
    refresh();
}

/// Lit l'agenda `url` tout de suite (hors du fil de l'interface).
fn test(url: &str) {
    TESTS.with_borrow_mut(|t| t.insert(url.to_owned(), Test::Pending));
    refresh();
    let opening = OPENING.get();
    let url = url.to_owned();
    crate::util::spawn_named("agenda-test", move || {
        let result = calendar_test(&url);
        let _ = slint::invoke_from_event_loop(move || {
            if OPENING.get() != opening {
                return;
            }
            let t = match result {
                Ok(r) => Test::Ok(r),
                Err(e) => Test::Error(e),
            };
            TESTS.with_borrow_mut(|tests| tests.insert(url, t));
            refresh();
        });
    });
}

fn add() {
    let draft = DRAFT.with_borrow(|d| d.trim().to_owned());
    let mut cals = current();
    if !cals.add(&draft) {
        return;
    }
    DRAFT.set(String::new());
    save(&cals);
    if let Some(w) = super::window() {
        let g = w.global::<CalendarSettingsData>();
        g.set_cleared(g.get_cleared() + 1);
    }
    test(&draft);
}

/// Retrait annulable : l'agenda revient avec son nom et sa couleur.
fn remove(i: usize) {
    let before = current();
    let Some(url) = before.urls.get(i).cloned() else { return };
    let read_name = TESTS.with_borrow(|t| match t.get(&url) {
        Some(Test::Ok(r)) => r.name.clone(),
        _ => None,
    });
    let label = before.names.get(&url).cloned().or(read_name);
    let mut next = before.clone();
    next.urls.remove(i);
    save(&next);
    offer_undo(Removal {
        label: match label {
            Some(l) => format!("Agenda « {l} » retiré"),
            None => format!("Agenda {} retiré", i + 1),
        },
        undo: Box::new(move || save(&before)),
        commit: None,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_colors_are_kept_per_address() {
        let s = json!({ "urls": ["https://a/x.ics", "https://b/y.ics"], "names": { "https://a/x.ics": "Perso", "https://zz": "vieux" }, "colors": { "https://b/y.ics": 4 } });
        let mut c = Calendars::of(&s);
        assert_eq!(c.colors["https://a/x.ics"], 0, "couleur du rang");
        assert_eq!(c.colors["https://b/y.ics"], 4);
        assert_eq!(c.free_color(), 1);
        assert!(c.add(" https://c/z.ics "));
        assert!(!c.add("https://c/z.ics"), "déjà là");
        assert!(!c.add("  "));
        assert_eq!(c.colors["https://c/z.ics"], 1);
        c.urls.remove(0);
        let p = c.patch();
        assert_eq!(p["urls"], json!(["https://b/y.ics", "https://c/z.ics"]));
        assert_eq!(p["names"], json!({}), "noms des adresses retirées oubliés");
        assert_eq!(p["colors"], json!({ "https://b/y.ics": 4, "https://c/z.ics": 1 }));
        c.rename("https://b/y.ics", "  Travail ");
        assert_eq!(c.names["https://b/y.ics"], "Travail");
        c.rename("https://b/y.ics", " ");
        assert!(!c.names.contains_key("https://b/y.ics"));
    }

    #[test]
    fn free_color_wraps_when_all_are_used() {
        let c = Calendars::of(&json!({ "urls": ["1", "2", "3", "4", "5", "6", "7"] }));
        assert_eq!(c.free_color(), 1, "7 agendas : 7 % 6");
    }

    #[test]
    fn secret_addresses_are_masked() {
        assert_eq!(masked("https://calendar.google.com/calendar/ical/abc/private-1234567890/basic.ics"), "calendar.google.com/…/basic.ics");
        assert_eq!(masked("webcal://h.fr/a"), "h.fr/…l://h.fr/a", "les 10 derniers caractères");
    }

    #[test]
    fn read_status_texts() {
        assert_eq!(status(None), "lecture…");
        assert_eq!(status(Some(&Test::Ok(CalendarTest { name: None, events: 0 }))), "lu · aucun événement d’hier à dans 8 jours");
        assert_eq!(status(Some(&Test::Ok(CalendarTest { name: None, events: 3 }))), "lu · 3 événements d’hier à dans 8 jours");
        assert_eq!(status(Some(&Test::Error("réponse 403".into()))), "réponse 403");
    }
}
