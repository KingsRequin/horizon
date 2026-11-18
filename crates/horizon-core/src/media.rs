//! Lecture en cours : position extrapolée, changement de morceau, îlot rétracté.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Identifiant de l'appli source (AUMID).
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// Position au moment `timeline_at_ms` (millisecondes Unix).
    pub position_ms: u64,
    pub duration_ms: u64,
    pub timeline_at_ms: u64,
}

/// Position à l'instant `now_ms` : elle avance pendant la lecture, sans dépasser la durée.
pub fn position_at(info: &MediaInfo, now_ms: u64) -> u64 {
    let elapsed = if info.playing { now_ms.saturating_sub(info.timeline_at_ms) } else { 0 };
    let pos = info.position_ms + elapsed;
    if info.duration_ms > 0 { pos.min(info.duration_ms) } else { pos }
}

/// Écart toléré entre la position que les fenêtres extrapolent et celle lue : en deçà, une
/// relecture n'est pas renvoyée.
pub const DRIFT_MS: u64 = 1_500;

/// Une relecture mérite d'être envoyée aux fenêtres : autre morceau, lecture ou pause, durée,
/// ou position qui s'écarte de plus de `DRIFT_MS` de celle extrapolée depuis le dernier envoi.
pub fn worth_sending(sent: Option<&MediaInfo>, next: Option<&MediaInfo>, now_ms: u64) -> bool {
    match (sent, next) {
        (None, None) => false,
        (Some(a), Some(b)) => {
            let same = a.app_id == b.app_id
                && a.title == b.title
                && a.artist == b.artist
                && a.playing == b.playing
                && a.duration_ms == b.duration_ms;
            !same || position_at(a, now_ms).abs_diff(position_at(b, now_ms)) > DRIFT_MS
        }
        _ => true,
    }
}

/// Rien ne joue (aucune session, ou session sans titre) : l'îlot se rétracte.
pub fn collapsed(info: Option<&MediaInfo>) -> bool {
    info.is_none_or(|i| i.title.trim().is_empty())
}

/// Nouvel état lu. Certaines applis signalent le nouveau titre avant la nouvelle position :
/// si le titre change mais que la chronologie est restée identique, la position repart de 0.
pub fn merge(prev: Option<&MediaInfo>, mut next: MediaInfo, now_ms: u64) -> MediaInfo {
    if let Some(p) = prev
        && p.title != next.title
        && p.position_ms == next.position_ms
        && p.timeline_at_ms == next.timeline_at_ms
    {
        next.position_ms = 0;
        next.timeline_at_ms = now_ms;
    }
    next
}

/// Point d'une chronologie WinRT (unités de 100 ns) → millisecondes depuis son début.
pub fn timeline_ms(ticks: i64, start: i64) -> u64 {
    ((ticks - start).max(0) / 10_000) as u64
}

/// Position à demander (unités de 100 ns de la chronologie qui commence à `start`) pour avancer
/// de `delta_ms` depuis la position courante, sans sortir du morceau.
pub fn seek_target(info: &MediaInfo, now_ms: u64, delta_ms: i64, start: i64) -> i64 {
    let pos = position_at(info, now_ms) as i64 + delta_ms;
    let max = if info.duration_ms > 0 { info.duration_ms as i64 } else { i64::MAX / 10_000 };
    start + pos.clamp(0, max) * 10_000
}

/// Un autre morceau commence : titre ou artiste différents d'un morceau déjà connu (le premier
/// relevé, au démarrage, ne compte pas).
pub fn is_new_track(before: Option<&MediaInfo>, now: Option<&MediaInfo>) -> bool {
    match (before, now) {
        (Some(b), Some(n)) => !n.title.is_empty() && (b.title != n.title || b.artist != n.artist),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn timeline_and_seek_stay_within_the_track() {
        assert_eq!(timeline_ms(50_000 + 25_000_000, 50_000), 2_500);
        assert_eq!(timeline_ms(0, 50_000), 0);
        let info = MediaInfo {
            app_id: "a".into(),
            title: "t".into(),
            artist: String::new(),
            playing: false,
            position_ms: 10_000,
            duration_ms: 60_000,
            timeline_at_ms: 0,
        };
        assert_eq!(seek_target(&info, 0, 5_000, 7), 7 + 15_000 * 10_000);
        assert_eq!(seek_target(&info, 0, -20_000, 7), 7, "pas avant le début");
        assert_eq!(seek_target(&info, 0, 90_000, 0), 60_000 * 10_000, "pas après la fin");
    }

    use super::*;

    fn info(title: &str, playing: bool, position_ms: u64, at: u64) -> MediaInfo {
        MediaInfo {
            app_id: "pear".into(),
            title: title.into(),
            artist: "Daft Punk".into(),
            playing,
            position_ms,
            duration_ms: 300_000,
            timeline_at_ms: at,
        }
    }

    #[test]
    fn media_state_none_collapses() {
        assert!(collapsed(None));
        assert!(collapsed(Some(&info("", true, 0, 0))));
        assert!(!collapsed(Some(&info("Veridis Quo", false, 0, 0))));
    }

    #[test]
    fn position_extrapolated_while_playing() {
        let i = info("Veridis Quo", true, 10_000, 1_000);
        assert_eq!(position_at(&i, 4_000), 13_000);
    }

    #[test]
    fn paused_position_does_not_move() {
        let i = info("Veridis Quo", false, 10_000, 1_000);
        assert_eq!(position_at(&i, 60_000), 10_000);
    }

    #[test]
    fn position_clamped_to_duration() {
        let i = info("Veridis Quo", true, 299_000, 0);
        assert_eq!(position_at(&i, 10_000), 300_000);
        // Durée inconnue (0) : pas de borne.
        let mut live = i.clone();
        live.duration_ms = 0;
        assert_eq!(position_at(&live, 10_000), 309_000);
        // Horloge en arrière : pas de dépassement négatif.
        assert_eq!(position_at(&info("x", true, 5_000, 10_000), 9_000), 5_000);
    }

    #[test]
    fn only_visible_changes_or_drift_are_sent() {
        let sent = info("Veridis Quo", true, 10_000, 1_000);
        // Relecture une seconde plus tard, position cohérente : rien à envoyer.
        let next = info("Veridis Quo", true, 11_000, 2_000);
        assert!(!worth_sending(Some(&sent), Some(&next), 2_000));
        // Petite dérive tolérée, grande dérive (avance rapide) envoyée.
        assert!(!worth_sending(Some(&sent), Some(&info("Veridis Quo", true, 12_400, 2_000)), 2_000));
        assert!(worth_sending(Some(&sent), Some(&info("Veridis Quo", true, 30_000, 2_000)), 2_000));
        // Pause, autre morceau, apparition et disparition : envoyés.
        assert!(worth_sending(Some(&sent), Some(&info("Veridis Quo", false, 11_000, 2_000)), 2_000));
        assert!(worth_sending(Some(&sent), Some(&info("Digital Love", true, 0, 2_000)), 2_000));
        assert!(worth_sending(None, Some(&next), 2_000));
        assert!(worth_sending(Some(&sent), None, 2_000));
        assert!(!worth_sending(None, None, 2_000));
    }

    #[test]
    fn title_change_resets_position() {
        let prev = info("Veridis Quo", true, 200_000, 1_000);
        let next = info("Digital Love", true, 200_000, 1_000);
        let m = merge(Some(&prev), next, 5_000);
        assert_eq!((m.position_ms, m.timeline_at_ms), (0, 5_000));
    }

    #[test]
    fn same_title_or_new_timeline_kept() {
        let prev = info("Veridis Quo", true, 200_000, 1_000);
        let same = merge(Some(&prev), info("Veridis Quo", true, 201_000, 2_000), 5_000);
        assert_eq!((same.position_ms, same.timeline_at_ms), (201_000, 2_000));
        let fresh = merge(Some(&prev), info("Digital Love", true, 3_000, 4_000), 5_000);
        assert_eq!((fresh.position_ms, fresh.timeline_at_ms), (3_000, 4_000));
        let first = merge(None, info("Digital Love", true, 3_000, 4_000), 5_000);
        assert_eq!(first.position_ms, 3_000);
    }

    #[test]
    fn new_track_needs_a_known_previous_one() {
        let (a, b) = (info("A", true, 0, 0), info("B", true, 0, 0));
        assert!(is_new_track(Some(&a), Some(&b)));
        assert!(!is_new_track(Some(&a), Some(&info("A", false, 5000, 9))), "même morceau, autre état");
        assert!(!is_new_track(None, Some(&a)), "premier relevé");
        assert!(!is_new_track(Some(&a), None));
        assert!(!is_new_track(Some(&a), Some(&info("", true, 0, 0))), "titre vide");
    }
}
