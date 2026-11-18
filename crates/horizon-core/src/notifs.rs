//! Notifications reçues : liste récente, non lues, notification à faire apparaître dans l'îlot.

use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_id: String,
    pub title: String,
    pub body: String,
    /// Millisecondes depuis l'époque Unix.
    pub at_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NotifEvent {
    Added(Notification),
    Removed(u32),
    /// L'utilisateur n'a pas (ou plus) autorisé l'accès aux notifications.
    AccessDenied,
    /// Lecture impossible pour le moment (identité de paquet absente, erreur Windows) : reprise
    /// automatique.
    Unavailable,
}

/// Une notification plus vieille que ça à sa découverte (déjà là au démarrage) n'est ni
/// « non lue » ni affichée dans l'îlot.
pub(crate) const FRESH_MS: u64 = 15_000;
pub(crate) const CAPACITY: usize = 50;

/// Relecture de la liste quand l'utilisateur est là : l'îlot montre vite ce qui arrive.
const ACTIVE_POLL_MS: u64 = 2_000;
/// Relecture quand il ne touche plus à rien : chaque lecture fait analyser par Windows le XML
/// de toutes les notifications présentes. Plus court que `FRESH_MS` : une notification lue en
/// retard compte encore comme nouvelle.
const IDLE_POLL_MS: u64 = 10_000;
/// Sans clavier ni souris depuis ce délai, l'utilisateur est considéré comme inactif.
const IDLE_AFTER_MS: u64 = 60_000;
const _: () = assert!(IDLE_POLL_MS < FRESH_MS, "une notification lue en retard doit rester nouvelle");

/// Délai avant la prochaine lecture, selon le temps écoulé depuis la dernière saisie.
pub fn poll_period_ms(input_idle_ms: u64) -> u64 {
    if input_idle_ms >= IDLE_AFTER_MS { IDLE_POLL_MS } else { ACTIVE_POLL_MS }
}

#[derive(Debug, Default)]
pub struct NotifStore {
    /// Plus récente d'abord.
    list: Vec<Notification>,
    unread: HashSet<u32>,
    /// Applis coupées (identifiant d'appli) : leurs notifications restent dans la liste, sans
    /// îlot ni compte de non lues.
    muted: HashSet<String>,
    pub access_denied: bool,
    pub unavailable: bool,
}

impl NotifStore {
    /// Applique un événement ; renvoie la notification à montrer dans l'îlot, s'il y en a une.
    pub fn apply(&mut self, ev: NotifEvent, now_ms: u64) -> Option<Notification> {
        match ev {
            NotifEvent::Unavailable => {
                self.unavailable = true;
                None
            }
            NotifEvent::AccessDenied => {
                self.access_denied = true;
                None
            }
            NotifEvent::Removed(id) => {
                self.list.retain(|n| n.id != id);
                self.unread.remove(&id);
                None
            }
            NotifEvent::Added(n) => {
                self.access_denied = false;
                self.unavailable = false;
                if n.app_id.contains("Horizon") || self.list.iter().any(|x| x.id == n.id) {
                    return None;
                }
                let fresh = n.at_ms + FRESH_MS >= now_ms && !self.muted.contains(&n.app_id);
                if fresh {
                    self.unread.insert(n.id);
                }
                self.list.push(n.clone());
                self.list.sort_by_key(|n| std::cmp::Reverse(n.at_ms));
                for dropped in self.list.drain(CAPACITY.min(self.list.len())..) {
                    self.unread.remove(&dropped.id);
                }
                (fresh && self.unread.contains(&n.id)).then_some(n)
            }
        }
    }

    pub fn unread(&self) -> usize {
        self.unread.len()
    }

    pub fn recent(&self, n: usize) -> Vec<Notification> {
        self.list.iter().take(n).cloned().collect()
    }

    pub fn mark_all_read(&mut self) {
        self.unread.clear();
    }

    /// Notification ouverte depuis le panneau : lue. Renvoie vrai si elle ne l'était pas.
    pub fn mark_read(&mut self, id: u32) -> bool {
        self.unread.remove(&id)
    }

    /// Notification écartée (aussi retirée du centre de notifications de Windows).
    pub fn dismiss(&mut self, id: u32) -> bool {
        self.unread.remove(&id);
        let before = self.list.len();
        self.list.retain(|n| n.id != id);
        self.list.len() != before
    }

    /// Non lues, de la plus récente à la plus ancienne.
    pub fn unread_ids(&self) -> Vec<u32> {
        self.list.iter().filter(|n| self.unread.contains(&n.id)).map(|n| n.id).collect()
    }

    /// Coupe (ou rétablit) une appli ; ses notifications non lues deviennent lues.
    pub fn set_muted(&mut self, app_id: &str, muted: bool) {
        if muted {
            self.muted.insert(app_id.to_owned());
            let ids: Vec<u32> = self.list.iter().filter(|n| n.app_id == app_id).map(|n| n.id).collect();
            for id in ids {
                self.unread.remove(&id);
            }
        } else {
            self.muted.remove(app_id);
        }
    }

    /// Applis coupées, triées (enregistrées dans `notifs.json`).
    pub fn muted(&self) -> Vec<String> {
        let mut v: Vec<String> = self.muted.iter().cloned().collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000_000;

    fn n(id: u32, at_ms: u64) -> Notification {
        Notification {
            id,
            app_name: "Discord".into(),
            app_id: "com.squirrel.Discord.Discord".into(),
            title: "Léo".into(),
            body: "on lance une partie ?".into(),
            at_ms,
        }
    }

    #[test]
    fn fresh_notification_flashes_and_is_unread() {
        let mut s = NotifStore::default();
        assert_eq!(s.apply(NotifEvent::Added(n(1, NOW)), NOW).map(|x| x.id), Some(1));
        assert_eq!(s.unread(), 1);
    }

    #[test]
    fn old_notifications_not_unread_nor_flash() {
        let mut s = NotifStore::default();
        assert_eq!(s.apply(NotifEvent::Added(n(1, NOW - 60_000)), NOW), None);
        assert_eq!(s.unread(), 0);
        assert_eq!(s.recent(5).len(), 1);
    }

    #[test]
    fn dedup_same_id() {
        let mut s = NotifStore::default();
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        assert_eq!(s.apply(NotifEvent::Added(n(1, NOW)), NOW), None);
        assert_eq!(s.recent(10).len(), 1);
        assert_eq!(s.unread(), 1);
    }

    #[test]
    fn removed_decrements_unread() {
        let mut s = NotifStore::default();
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        s.apply(NotifEvent::Added(n(2, NOW)), NOW);
        s.apply(NotifEvent::Removed(1), NOW);
        assert_eq!(s.unread(), 1);
        assert_eq!(s.recent(10).iter().map(|x| x.id).collect::<Vec<_>>(), vec![2]);
        s.apply(NotifEvent::Removed(99), NOW);
        assert_eq!(s.unread(), 1);
    }

    #[test]
    fn cap_50_oldest_dropped() {
        let mut s = NotifStore::default();
        for i in 0..60 {
            s.apply(NotifEvent::Added(n(i, NOW + i as u64)), NOW);
        }
        let ids: Vec<u32> = s.recent(100).iter().map(|x| x.id).collect();
        assert_eq!(ids.len(), CAPACITY);
        assert_eq!(ids[0], 59, "plus récente d'abord");
        assert!(!ids.contains(&9));
        assert_eq!(s.unread(), CAPACITY);
    }

    #[test]
    fn mark_one_read_and_dismiss() {
        let mut s = NotifStore::default();
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        s.apply(NotifEvent::Added(n(2, NOW + 1)), NOW);
        assert_eq!(s.unread_ids(), vec![2, 1]);
        assert!(s.mark_read(1));
        assert!(!s.mark_read(1));
        assert_eq!(s.unread_ids(), vec![2]);
        assert!(s.dismiss(2));
        assert!(!s.dismiss(2));
        assert_eq!((s.unread(), s.recent(5).len()), (0, 1));
    }

    #[test]
    fn muted_app_neither_flashes_nor_counts() {
        let mut s = NotifStore::default();
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        s.set_muted("com.squirrel.Discord.Discord", true);
        assert_eq!(s.unread(), 0, "ses non lues deviennent lues");
        assert_eq!(s.apply(NotifEvent::Added(n(2, NOW)), NOW), None);
        assert_eq!((s.unread(), s.recent(5).len()), (0, 2), "gardée dans la liste");
        assert_eq!(s.muted(), vec!["com.squirrel.Discord.Discord".to_owned()]);
        s.set_muted("com.squirrel.Discord.Discord", false);
        assert_eq!(s.apply(NotifEvent::Added(n(3, NOW)), NOW).map(|x| x.id), Some(3));
    }

    #[test]
    fn mark_all_read_clears() {
        let mut s = NotifStore::default();
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        s.mark_all_read();
        assert_eq!(s.unread(), 0);
        assert_eq!(s.recent(5).len(), 1);
    }

    #[test]
    fn access_denied_reported_once() {
        let mut s = NotifStore::default();
        assert_eq!(s.apply(NotifEvent::AccessDenied, NOW), None);
        assert_eq!(s.apply(NotifEvent::AccessDenied, NOW), None);
        assert!(s.access_denied);
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        assert!(!s.access_denied, "l'accès est revenu");
    }

    #[test]
    fn unavailable_until_next_notification() {
        let mut s = NotifStore::default();
        assert_eq!(s.apply(NotifEvent::Unavailable, NOW), None);
        assert!(s.unavailable);
        s.apply(NotifEvent::Added(n(1, NOW)), NOW);
        assert!(!s.unavailable);
    }

    #[test]
    fn ignores_horizon_itself() {
        let mut s = NotifStore::default();
        let mut own = n(1, NOW);
        own.app_id = "Horizon".into();
        assert_eq!(s.apply(NotifEvent::Added(own), NOW), None);
        assert!(s.recent(5).is_empty());
    }

    #[test]
    fn polls_less_when_user_is_idle_but_stays_fresh() {
        assert_eq!(poll_period_ms(0), ACTIVE_POLL_MS);
        assert_eq!(poll_period_ms(IDLE_AFTER_MS - 1), ACTIVE_POLL_MS);
        assert_eq!(poll_period_ms(IDLE_AFTER_MS), IDLE_POLL_MS);
    }
}
