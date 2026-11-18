//! Palette de commandes : recherche floue (sous-séquence, bonus de début de mot et de
//! lettres qui se suivent, fréquence d'usage), mémoire des usages et raccourci clavier global.

use crate::geometry::{MonitorInfo, Rect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Lettre comparable : minuscule, sans accent (« É » → « e », « ç » → « c »).
pub fn fold(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }
}

/// Correspondance d'une recherche dans un texte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Positions (en caractères) des lettres trouvées, pour les surligner.
    pub hits: Vec<usize>,
}

const MATCH: i32 = 1;
const WORD_START: i32 = 8;
const CONSECUTIVE: i32 = 6;
const PREFIX: i32 = 6;
/// Pénalité par lettre sautée entre deux lettres trouvées, et son plafond.
const GAP: i32 = 1;
const GAP_CAP: i32 = 6;

/// La position `i` commence un mot : début du texte, après un séparateur, ou passage
/// minuscule → majuscule (« WindowsTerminal »).
fn word_start(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let (p, c) = (chars[i - 1], chars[i]);
    !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic())
}

/// Meilleure façon de trouver les lettres de `query` (espaces ignorés), dans l'ordre, dans
/// `text`. `None` si une lettre manque.
pub(crate) fn fuzzy(query: &str, text: &str) -> Option<Match> {
    let q: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).map(fold).collect();
    if q.is_empty() {
        return Some(Match { score: 0, hits: Vec::new() });
    }
    let raw: Vec<char> = text.chars().collect();
    let t: Vec<char> = raw.iter().map(|&c| fold(c)).collect();
    let (m, n) = (q.len(), t.len());
    if m > n {
        return None;
    }
    // best[i][j] : meilleur score avec la lettre i de la recherche posée sur la lettre j du texte.
    const NONE: i32 = i32::MIN / 2;
    let mut best = vec![vec![NONE; n]; m];
    let mut from = vec![vec![usize::MAX; n]; m];
    for i in 0..m {
        for j in i..n {
            if t[j] != q[i] {
                continue;
            }
            let mut bonus = MATCH;
            if word_start(&raw, j) {
                bonus += WORD_START;
            }
            if i == 0 {
                best[i][j] = bonus + if j == 0 { PREFIX } else { -(j as i32).min(GAP_CAP) * GAP };
                continue;
            }
            for k in (i - 1)..j {
                if best[i - 1][k] == NONE {
                    continue;
                }
                let link = if k + 1 == j { CONSECUTIVE } else { -((j - k - 1) as i32).min(GAP_CAP) * GAP };
                let s = best[i - 1][k] + bonus + link;
                if s > best[i][j] {
                    best[i][j] = s;
                    from[i][j] = k;
                }
            }
        }
    }
    let (mut j, score) = best[m - 1].iter().enumerate().filter(|(_, s)| **s != NONE).max_by_key(|(j, s)| (**s, std::cmp::Reverse(*j)))?;
    let score = *score;
    let mut hits = vec![0; m];
    for i in (0..m).rev() {
        hits[i] = j;
        if i > 0 {
            j = from[i][j];
        }
    }
    Some(Match { score, hits })
}

/// Bonus d'usage : croît avec le nombre d'utilisations, mais lentement et plafonné, pour
/// qu'une entrée très utilisée ne passe pas devant une bien meilleure correspondance.
pub(crate) fn usage_bonus(uses: u32) -> i32 {
    ((f64::from(uses) + 1.0).ln() * 4.0).round().min(12.0) as i32
}

/// Écart retiré à une correspondance trouvée dans les mots-clés plutôt que dans le titre.
const KEYWORD_MALUS: i32 = 10;

/// Entrée candidate au classement.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub title: &'a str,
    /// Texte secondaire cherché aussi (sous-titre, cible, genre d'entrée).
    pub keywords: &'a str,
    pub uses: u32,
}

/// Résultat classé : indice dans la liste d'entrée et lettres du titre à surligner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    pub index: usize,
    pub hits: Vec<usize>,
}

/// Classe les candidats pour `query` (au plus `limit`). Recherche vide : les plus utilisés,
/// puis l'ordre d'origine (les sources sont déjà rangées par intérêt).
pub fn rank(query: &str, candidates: &[Candidate], limit: usize) -> Vec<Ranked> {
    let mut scored: Vec<(i32, usize, Vec<usize>)> = if query.trim().is_empty() {
        candidates.iter().enumerate().map(|(i, c)| (usage_bonus(c.uses), i, Vec::new())).collect()
    } else {
        candidates
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let title = fuzzy(query, c.title);
                let keyword = fuzzy(query, c.keywords).map(|m| m.score - KEYWORD_MALUS);
                let (score, hits) = match (title, keyword) {
                    (Some(t), Some(k)) if k > t.score => (k, Vec::new()),
                    (Some(t), _) => (t.score, t.hits),
                    (None, Some(k)) => (k, Vec::new()),
                    (None, None) => return None,
                };
                Some((score + usage_bonus(c.uses), i, hits))
            })
            .collect()
    };
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| candidates[a.1].title.chars().count().cmp(&candidates[b.1].title.chars().count()))
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.into_iter().take(limit).map(|(_, index, hits)| Ranked { index, hits }).collect()
}

/// Nombre d'utilisations de chaque entrée (`%APPDATA%\Horizon\palette.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub counts: HashMap<String, u32>,
}

impl Usage {
    /// Au-delà, les compteurs sont divisés par deux : les vieilles habitudes s'effacent.
    pub(crate) const CAP: u32 = 200;

    pub fn uses(&self, id: &str) -> u32 {
        self.counts.get(id).copied().unwrap_or(0)
    }

    pub fn record(&mut self, id: &str) {
        let n = self.counts.entry(id.to_owned()).or_insert(0);
        *n += 1;
        if *n > Self::CAP {
            self.counts.values_mut().for_each(|v| *v /= 2);
            self.counts.retain(|_, v| *v > 0);
        }
    }
}

/// Section `[palette]` de `settings.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteSettings {
    /// Raccourci global, par exemple « Ctrl+Alt+P » ; vide = pas de raccourci.
    pub hotkey: String,
}

impl Default for PaletteSettings {
    fn default() -> Self {
        PaletteSettings { hotkey: "Ctrl+Alt+P".into() }
    }
}

/// Place de la fenêtre de la palette (taille logique `w` × `h`) : sur l'écran de la souris
/// (à défaut le principal), centrée, au cinquième haut de sa zone de travail.
pub fn window_rect(monitors: &[MonitorInfo], cursor: (i32, i32), w: u32, h: u32) -> Option<Rect> {
    let inside = |m: &&MonitorInfo| {
        let r = m.rect;
        (r.x..r.x + r.w).contains(&cursor.0) && (r.y..r.y + r.h).contains(&cursor.1)
    };
    let m = monitors.iter().find(inside).or_else(|| monitors.iter().find(|m| m.primary)).or(monitors.first())?;
    let scale = if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 };
    let (pw, ph) = ((w as f32 * scale).round() as i32, (h as f32 * scale).round() as i32);
    let work = m.work;
    Some(Rect { x: work.x + (work.w - pw) / 2, y: work.y + work.h / 5, w: pw, h: ph })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cands<'a>(titles: &[&'a str]) -> Vec<Candidate<'a>> {
        titles.iter().map(|t| Candidate { title: t, keywords: "", uses: 0 }).collect()
    }

    fn order(query: &str, c: &[Candidate]) -> Vec<String> {
        rank(query, c, 10).into_iter().map(|r| c[r.index].title.to_owned()).collect()
    }

    #[test]
    fn subsequence_is_required() {
        assert!(fuzzy("dsc", "Discord").is_some());
        assert!(fuzzy("dcs", "Discord").is_none());
        assert!(fuzzy("discordx", "Discord").is_none());
        assert_eq!(fuzzy("", "Discord").unwrap().score, 0);
    }

    #[test]
    fn accents_case_and_spaces_are_ignored() {
        let m = fuzzy("reglages", "Ouvrir les Réglages").unwrap();
        assert_eq!(m.hits, (11..19).collect::<Vec<_>>());
        assert!(fuzzy("SORTIE AUDIO", "Sortie audio suivante").is_some());
    }

    #[test]
    fn hits_prefer_word_starts() {
        // « vs » : V de Visual, S de Studio (et non le s de « Visual »).
        assert_eq!(fuzzy("vs", "Visual Studio").unwrap().hits, [0, 7]);
        // « wt » : début de mot en CamelCase.
        assert_eq!(fuzzy("wt", "WindowsTerminal").unwrap().hits, [0, 7]);
    }

    #[test]
    fn word_starts_and_runs_beat_scattered_letters() {
        let c = cands(&["Photoshop", "Spotify", "Paramètres de son"]);
        assert_eq!(order("spo", &c)[0], "Spotify");
        let c = cands(&["Microsoft Teams", "Mozilla Thunderbird", "Mise en veille"]);
        assert_eq!(order("mt", &c)[..2], ["Microsoft Teams", "Mozilla Thunderbird"]);
        let c = cands(&["Couper le micro", "Compteur", "Configuration du micro"]);
        assert_eq!(order("micro", &c)[..2], ["Couper le micro", "Configuration du micro"]);
    }

    #[test]
    fn prefix_wins() {
        let c = cands(&["Terminal SSH : nas", "nas"]);
        assert_eq!(order("nas", &c), ["nas", "Terminal SSH : nas"]);
    }

    #[test]
    fn usage_breaks_ties_but_not_quality() {
        let mut c = cands(&["Steam", "Stellarium"]);
        c[1].uses = 30;
        assert_eq!(order("ste", &c), ["Stellarium", "Steam"]);
        // Correspondance nettement meilleure : l'usage ne la dépasse pas.
        let mut c = cands(&["Discord", "Disque dur externe et son contrôleur"]);
        c[1].uses = 10_000;
        assert_eq!(order("discord", &c)[0], "Discord");
        assert!(usage_bonus(u32::MAX) <= 12);
        assert_eq!(usage_bonus(0), 0);
    }

    #[test]
    fn keywords_match_without_highlight() {
        let c = [
            Candidate { title: "Profil : Jeu", keywords: "profil activer", uses: 0 },
            Candidate { title: "Musique", keywords: "éclairage scène prisme", uses: 0 },
        ];
        let r = rank("prisme", &c, 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].index, 1);
        assert!(r[0].hits.is_empty());
    }

    #[test]
    fn empty_query_lists_most_used_then_source_order() {
        let mut c = cands(&["a", "b", "c", "d"]);
        c[2].uses = 5;
        assert_eq!(order("", &c), ["c", "a", "b", "d"]);
        assert_eq!(rank("", &c, 2).len(), 2);
    }

    #[test]
    fn usage_counts_and_decay() {
        let mut u = Usage::default();
        u.record("x");
        u.record("x");
        u.record("y");
        assert_eq!(u.uses("x"), 2);
        assert_eq!(u.uses("z"), 0);
        for _ in 0..Usage::CAP {
            u.record("x");
        }
        assert!(u.uses("x") <= Usage::CAP);
        // « y » (1) est tombé à 0 et a été oublié.
        assert_eq!(u.uses("y"), 0);
        assert!(!u.counts.contains_key("y"));
    }

    #[test]
    fn window_on_the_mouse_screen() {
        let mut a = crate::layout::tests::mon("a", 0, 0, 1920, 1080, true);
        a.work = Rect { x: 0, y: 40, w: 1920, h: 1040 };
        let mut b = crate::layout::tests::mon("b", 1920, -400, 2560, 1440, false);
        b.scale = 1.5;
        b.work = b.rect;
        let all = [a, b];
        assert_eq!(window_rect(&all, (100, 100), 600, 400), Some(Rect { x: 660, y: 40 + 208, w: 600, h: 400 }));
        assert_eq!(window_rect(&all, (2000, 0), 600, 400), Some(Rect { x: 1920 + (2560 - 900) / 2, y: -400 + 288, w: 900, h: 600 }));
        // Souris hors de tout écran : écran principal.
        assert_eq!(window_rect(&all, (-5000, 0), 600, 400).unwrap().x, 660);
        assert_eq!(window_rect(&[], (0, 0), 600, 400), None);
    }
}
