//! Raccourci clavier global écrit en texte (`[palette]` `hotkey` de `settings.toml`).

/// Raccourci clavier global : modificateurs et touche virtuelle Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Code de touche virtuelle (`VK_*`).
    pub vk: u32,
}

fn vk_of(key: &str) -> Option<u32> {
    let k = key.to_ascii_lowercase();
    let b = k.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_alphabetic() || b[0].is_ascii_digit()) {
        return Some(u32::from(b[0].to_ascii_uppercase()));
    }
    if let Some(n) = k.strip_prefix('f').and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x6F + n);
    }
    Some(match k.as_str() {
        "space" | "espace" => 0x20,
        "enter" | "entrée" | "entree" => 0x0D,
        "tab" => 0x09,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" | "fin" => 0x23,
        "home" | "début" | "debut" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" | "inser" => 0x2D,
        "delete" | "suppr" => 0x2E,
        _ => return None,
    })
}

impl Hotkey {
    /// Lit « Ctrl+Alt+Space » (casse et espaces libres). Il faut au moins Ctrl, Alt ou Win :
    /// sinon le raccourci volerait une touche de frappe ordinaire.
    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut h = Hotkey { ctrl: false, alt: false, shift: false, win: false, vk: 0 };
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mods) = parts.split_last()?;
        for m in mods {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => h.ctrl = true,
                "alt" => h.alt = true,
                "shift" | "maj" => h.shift = true,
                "win" | "super" | "meta" => h.win = true,
                _ => return None,
            }
        }
        h.vk = vk_of(key)?;
        (h.ctrl || h.alt || h.win).then_some(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys() {
        let h = Hotkey::parse("Ctrl+Alt+Space").unwrap();
        assert!(h.ctrl && h.alt && !h.shift && !h.win);
        assert_eq!(h.vk, 0x20);
        assert_eq!(Hotkey::parse(" win + shift + p ").unwrap().vk, u32::from(b'P'));
        assert_eq!(Hotkey::parse("Alt+F12").unwrap().vk, 0x7B);
        assert_eq!(Hotkey::parse("Ctrl+1").unwrap().vk, u32::from(b'1'));
        assert_eq!(Hotkey::parse("Shift+A"), None);
        assert_eq!(Hotkey::parse("Space"), None);
        assert_eq!(Hotkey::parse("Ctrl+Hyper+K"), None);
        assert_eq!(Hotkey::parse("Ctrl+F25"), None);
        assert_eq!(Hotkey::parse(""), None);
    }
}
