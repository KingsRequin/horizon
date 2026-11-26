//! Saisie à distance (clavier et souris du téléphone) : noms de touches, modificateurs, boutons
//! et débit. Le module `phone` d'Horizon applique ces décisions par `SendInput`.

/// Bouton de la souris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn parse(name: &str) -> Option<MouseButton> {
        Some(match name {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            _ => return None,
        })
    }
}

/// Ce que fait le bouton : un clic, ou l'appui et le relâchement séparés (glisser).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Click,
    Down,
    Up,
}

impl ButtonAction {
    pub fn parse(name: &str) -> Option<ButtonAction> {
        Some(match name {
            "click" => ButtonAction::Click,
            "down" => ButtonAction::Down,
            "up" => ButtonAction::Up,
            _ => return None,
        })
    }
}

/// Modificateur tenu pendant une touche.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mod {
    Ctrl,
    Alt,
    Shift,
    Win,
}

impl Mod {
    pub fn parse(name: &str) -> Option<Mod> {
        Some(match name {
            "ctrl" => Mod::Ctrl,
            "alt" => Mod::Alt,
            "shift" => Mod::Shift,
            "win" => Mod::Win,
            _ => return None,
        })
    }

    /// Code de touche virtuelle Windows.
    pub fn code(self) -> u16 {
        match self {
            Mod::Ctrl => 0x11,
            Mod::Alt => 0x12,
            Mod::Shift => 0x10,
            Mod::Win => 0x5B,
        }
    }
}

/// Code de touche virtuelle Windows d'une touche nommée (minuscules) : touches spéciales,
/// F1 à F12, lettres et chiffres (pour les raccourcis). `None` : touche inconnue.
pub fn key_code(name: &str) -> Option<u16> {
    let named = match name {
        "escape" => 0x1B,
        "tab" => 0x09,
        "enter" => 0x0D,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "page_up" => 0x21,
        "page_down" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "space" => 0x20,
        "win" => 0x5B,
        "print_screen" => 0x2C,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
        return (1..=12).contains(&n).then_some(0x6F + n);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c @ 'a'..='z'), None) => Some(c as u16 - 'a' as u16 + 0x41),
        (Some(c @ '0'..='9'), None) => Some(c as u16 - '0' as u16 + 0x30),
        _ => None,
    }
}

/// Silence au-delà duquel une saisie ouvre une nouvelle séance (témoin, alerte).
pub const SESSION_IDLE_MS: u64 = 10_000;

/// Séances de saisie des téléphones : le témoin reste tant qu'une saisie a eu lieu dans les
/// 10 dernières secondes, et chaque séance qui commence est annoncée.
#[derive(Debug, Clone, Default)]
pub struct InputSessions {
    last: std::collections::BTreeMap<String, u64>,
    /// Ce que le témoin montre (dernière relecture) : ce que « Couper » coupe.
    shown: Vec<String>,
}

impl InputSessions {
    /// Saisie de `phone_id` à `now_ms` ; vrai si elle ouvre une séance.
    pub fn seen(&mut self, phone_id: &str, now_ms: u64) -> bool {
        let started = self.last.get(phone_id).is_none_or(|&at| now_ms.saturating_sub(at) > SESSION_IDLE_MS);
        self.last.insert(phone_id.to_owned(), now_ms);
        started
    }

    /// Téléphones dont la séance dure encore à `now_ms`, triés : ce que le témoin montre désormais.
    pub fn active(&mut self, now_ms: u64) -> Vec<String> {
        self.shown = self.last.iter().filter(|(_, at)| now_ms.saturating_sub(**at) <= SESSION_IDLE_MS).map(|(id, _)| id.clone()).collect();
        self.shown.clone()
    }

    /// Ce que montre le témoin depuis la dernière relecture ([`active`](Self::active)) : un clic
    /// dessus coupe ces téléphones, même si leur séance vient de finir.
    pub fn shown(&self) -> Vec<String> {
        self.shown.clone()
    }

    /// Séances finies à `now_ms` (plus de 10 s de silence), rendues une seule fois et oubliées :
    /// les boutons qu'elles tiennent encore sont à relâcher.
    pub fn expire(&mut self, now_ms: u64) -> Vec<String> {
        let ended: Vec<String> = self.last.iter().filter(|(_, at)| now_ms.saturating_sub(**at) > SESSION_IDLE_MS).map(|(id, _)| id.clone()).collect();
        for id in &ended {
            self.last.remove(id);
        }
        ended
    }

    /// Séance de `phone_id` close (droit coupé).
    pub fn end(&mut self, phone_id: &str) {
        self.last.remove(phone_id);
    }
}

/// Messages de saisie par seconde et par téléphone, au plus (seau à jetons).
const RATE: f64 = 120.0;

/// Débit de la saisie d'un téléphone : 120 messages par seconde au plus, une rafale de 120
/// passe, le reste est jeté (pas de file qui gonfle).
#[derive(Debug, Clone)]
pub struct RateLimit {
    tokens: f64,
    at_ms: Option<u64>,
}

impl Default for RateLimit {
    fn default() -> RateLimit {
        RateLimit { tokens: RATE, at_ms: None }
    }
}

impl RateLimit {
    /// Vrai si un message reçu à `now_ms` passe.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        if let Some(at) = self.at_ms {
            self.tokens = (self.tokens + now_ms.saturating_sub(at) as f64 * RATE / 1000.0).min(RATE);
        }
        self.at_ms = Some(now_ms);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names_map_to_codes() {
        assert_eq!(key_code("escape"), Some(0x1B));
        assert_eq!(key_code("enter"), Some(0x0D));
        assert_eq!(key_code("backspace"), Some(0x08));
        assert_eq!(key_code("left"), Some(0x25));
        assert_eq!(key_code("page_down"), Some(0x22));
        assert_eq!(key_code("f1"), Some(0x70));
        assert_eq!(key_code("f12"), Some(0x7B));
        assert_eq!(key_code("a"), Some(0x41));
        assert_eq!(key_code("z"), Some(0x5A));
        assert_eq!(key_code("0"), Some(0x30));
        assert_eq!(key_code("win"), Some(0x5B));
        assert_eq!(key_code("f13"), None);
        assert_eq!(key_code("A"), None, "les noms sont en minuscules");
        assert_eq!(key_code(""), None);
        assert_eq!(Mod::parse("shift"), Some(Mod::Shift));
        assert_eq!(Mod::Ctrl.code(), 0x11);
        assert_eq!(Mod::parse("meta"), None);
    }

    #[test]
    fn a_session_starts_after_ten_seconds_of_silence() {
        let mut s = InputSessions::default();
        assert!(s.seen("tel-1", 1_000), "première saisie : une séance commence");
        assert!(!s.seen("tel-1", 5_000));
        assert!(!s.seen("tel-1", 15_000), "moins de 10 s depuis la dernière : même séance");
        assert_eq!(s.active(20_000), vec!["tel-1".to_owned()]);
        assert!(s.seen("tel-2", 20_000), "chaque téléphone a sa séance");
        assert_eq!(s.active(25_500), vec!["tel-2".to_owned()], "tel-1 se tait depuis plus de 10 s");
        assert!(s.seen("tel-1", 26_000), "reprise après le silence : nouvelle séance");
        s.end("tel-1");
        assert_eq!(s.active(26_000), vec!["tel-2".to_owned()]);
        assert!(s.seen("tel-1", 26_100), "coupée : la saisie suivante en ouvre une autre");
        assert!(s.active(40_000).is_empty());
    }

    #[test]
    fn a_session_that_falls_silent_is_reported_once() {
        let mut s = InputSessions::default();
        s.seen("tel-1", 1_000);
        s.seen("tel-2", 5_000);
        assert_eq!(s.expire(9_000), Vec::<String>::new());
        assert_eq!(s.expire(12_000), vec!["tel-1".to_owned()], "plus de 10 s : la séance est finie");
        assert_eq!(s.expire(12_500), Vec::<String>::new(), "une seule fois");
        assert_eq!(s.expire(16_000), vec!["tel-2".to_owned()]);
    }

    #[test]
    fn what_was_shown_stays_the_target_of_a_cut() {
        let mut s = InputSessions::default();
        s.seen("tel-1", 1_000);
        assert_eq!(s.active(5_000), vec!["tel-1".to_owned()]);
        assert_eq!(s.shown(), vec!["tel-1".to_owned()], "à 11,5 s, la séance est finie mais le témoin, relu à 5 s, la montre encore");
        assert_eq!(s.active(11_500), Vec::<String>::new());
        assert!(s.shown().is_empty());
    }

    #[test]
    fn the_rate_is_limited_to_120_a_second() {
        let mut limit = RateLimit::default();
        let passed = (0..200).filter(|_| limit.allow(1_000)).count();
        assert_eq!(passed, 120, "une rafale : 120 passent, le reste est jeté");
        assert!(!limit.allow(1_000));
        assert!(limit.allow(1_010), "le seau se remplit avec le temps (10 ms : un jeton)");
        let mut steady = RateLimit::default();
        let ok = (0..600).filter(|i| steady.allow(1_000 + i * 10)).count();
        assert_eq!(ok, 600, "100 par seconde passent tous");
    }
}
