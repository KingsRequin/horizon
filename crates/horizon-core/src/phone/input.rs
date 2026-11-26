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
