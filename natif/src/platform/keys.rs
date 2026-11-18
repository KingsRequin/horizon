//! Clavier Windows → texte des événements clavier de Slint (logique pure, testée).
//!
//! Slint décrit une touche par le texte qu'elle produit : un caractère ordinaire, ou un code
//! réservé pour les touches spéciales (`slint::platform::Key`). Les modificateurs (Maj, Ctrl,
//! Alt, Windows) sont eux-mêmes des touches : Slint tient leur état d'après leurs appuis et
//! relâchements.
//!
//! Répartition : `WM_KEYDOWN` donne les touches spéciales (flèches, Entrée, Échap…) et, Ctrl ou
//! Alt tenus, la lettre ou le chiffre du raccourci (`Ctrl+Z` → « z ») ; `WM_CHAR` donne le
//! texte tapé (accents, touches mortes, AltGr compris), sans les caractères de contrôle que
//! Windows y ajoute pour les touches déjà traitées.

use slint::platform::Key;

/// Touches virtuelles utilisées (winuser.h).
pub mod vk {
    pub const BACK: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0D;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const MENU: u16 = 0x12;
    pub const PAUSE: u16 = 0x13;
    pub const CAPITAL: u16 = 0x14;
    pub const ESCAPE: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PRIOR: u16 = 0x21;
    pub const NEXT: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const INSERT: u16 = 0x2D;
    pub const DELETE: u16 = 0x2E;
    pub const LWIN: u16 = 0x5B;
    pub const RWIN: u16 = 0x5C;
    pub const F1: u16 = 0x70;
    pub const F24: u16 = 0x87;
    pub const SCROLL: u16 = 0x91;
    pub const LSHIFT: u16 = 0xA0;
    pub const RSHIFT: u16 = 0xA1;
    pub const LCONTROL: u16 = 0xA2;
    pub const RCONTROL: u16 = 0xA3;
    pub const LMENU: u16 = 0xA4;
    pub const RMENU: u16 = 0xA5;
}

/// Code de balayage de la touche Maj de droite (`VK_SHIFT` ne dit pas laquelle).
pub const RIGHT_SHIFT_SCAN: u32 = 0x36;

/// Touche spéciale de Slint pour la touche virtuelle `code` ; `right` : touche de droite (Maj,
/// Ctrl, Alt). Alt de droite est AltGr (claviers européens). `None` : touche de texte.
pub fn special(code: u16, right: bool) -> Option<Key> {
    Some(match code {
        vk::BACK => Key::Backspace,
        vk::TAB => Key::Tab,
        vk::RETURN => Key::Return,
        vk::ESCAPE => Key::Escape,
        vk::DELETE => Key::Delete,
        vk::SHIFT | vk::LSHIFT | vk::RSHIFT => {
            if right || code == vk::RSHIFT {
                Key::ShiftR
            } else {
                Key::Shift
            }
        }
        vk::CONTROL | vk::LCONTROL | vk::RCONTROL => {
            if right || code == vk::RCONTROL {
                Key::ControlR
            } else {
                Key::Control
            }
        }
        vk::MENU | vk::LMENU | vk::RMENU => {
            if right || code == vk::RMENU {
                Key::AltGr
            } else {
                Key::Alt
            }
        }
        vk::LWIN => Key::Meta,
        vk::RWIN => Key::MetaR,
        vk::CAPITAL => Key::CapsLock,
        vk::UP => Key::UpArrow,
        vk::DOWN => Key::DownArrow,
        vk::LEFT => Key::LeftArrow,
        vk::RIGHT => Key::RightArrow,
        vk::INSERT => Key::Insert,
        vk::HOME => Key::Home,
        vk::END => Key::End,
        vk::PRIOR => Key::PageUp,
        vk::NEXT => Key::PageDown,
        vk::SCROLL => Key::ScrollLock,
        vk::PAUSE => Key::Pause,
        vk::F1..=vk::F24 => return function_key(code - vk::F1),
        _ => return None,
    })
}

/// F1 (0) à F24 (23).
fn function_key(n: u16) -> Option<Key> {
    const KEYS: [Key; 24] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
        Key::F13,
        Key::F14,
        Key::F15,
        Key::F16,
        Key::F17,
        Key::F18,
        Key::F19,
        Key::F20,
        Key::F21,
        Key::F22,
        Key::F23,
        Key::F24,
    ];
    KEYS.get(n as usize).cloned()
}

/// Touche de modificateur (Maj, Ctrl, Alt, AltGr, Windows).
pub fn is_modifier(k: &Key) -> bool {
    matches!(
        k,
        Key::Shift | Key::ShiftR | Key::Control | Key::ControlR | Key::Alt | Key::AltGr | Key::Meta | Key::MetaR
    )
}

/// Texte d'un raccourci (Ctrl ou Alt tenus, sans AltGr) : la lettre (minuscule, majuscule avec
/// Maj) ou le chiffre de la touche, quelle que soit la disposition du clavier pour les chiffres
/// (`Ctrl+1` sur un clavier AZERTY). `None` : pas une touche de raccourci.
pub fn shortcut_char(code: u16, shift: bool) -> Option<char> {
    let c = char::from_u32(code as u32)?;
    match c {
        'A'..='Z' => Some(if shift { c } else { c.to_ascii_lowercase() }),
        '0'..='9' => Some(c),
        _ if code == vk::SPACE => Some(' '),
        _ => None,
    }
}

/// Caractère reçu par `WM_CHAR` à transmettre comme texte : pas les caractères de contrôle
/// (Retour arrière, Tab, Entrée, Échap, `Ctrl+lettre`), déjà donnés par `WM_KEYDOWN`.
pub fn is_text(c: char) -> bool {
    !c.is_control()
}

/// Recompose les caractères hors du plan de base, que `WM_CHAR` envoie en deux moitiés UTF-16.
#[derive(Default)]
pub struct Utf16 {
    high: Option<u16>,
}

impl Utf16 {
    /// Une unité UTF-16 de plus ; le caractère s'il est complet.
    pub fn push(&mut self, unit: u16) -> Option<char> {
        match unit {
            0xD800..=0xDBFF => {
                self.high = Some(unit);
                None
            }
            0xDC00..=0xDFFF => {
                let high = self.high.take()?;
                char::decode_utf16([high, unit]).next()?.ok()
            }
            _ => {
                self.high = None;
                char::from_u32(unit as u32)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(k: Key) -> String {
        slint::SharedString::from(k).to_string()
    }

    #[test]
    fn special_keys_map_to_slint_codes() {
        assert_eq!(special(vk::RETURN, false), Some(Key::Return));
        assert_eq!(special(vk::ESCAPE, false), Some(Key::Escape));
        assert_eq!(special(vk::LEFT, false), Some(Key::LeftArrow));
        assert_eq!(special(vk::F1, false), Some(Key::F1));
        assert_eq!(special(vk::F1 + 11, false), Some(Key::F12));
        assert_eq!(special(vk::F24, false), Some(Key::F24));
        assert_eq!(special(vk::DELETE, false), Some(Key::Delete));
        assert_eq!(special(b'A' as u16, false), None, "lettre : texte de WM_CHAR");
        assert_eq!(special(vk::SPACE, false), None, "espace : texte de WM_CHAR");
        assert_eq!(text(Key::Return), "\n");
    }

    #[test]
    fn modifiers_left_and_right() {
        assert_eq!(special(vk::SHIFT, false), Some(Key::Shift));
        assert_eq!(special(vk::SHIFT, true), Some(Key::ShiftR));
        assert_eq!(special(vk::CONTROL, true), Some(Key::ControlR));
        assert_eq!(special(vk::MENU, false), Some(Key::Alt));
        assert_eq!(special(vk::MENU, true), Some(Key::AltGr));
        assert_eq!(special(vk::LWIN, false), Some(Key::Meta));
        assert!(is_modifier(&Key::AltGr));
        assert!(!is_modifier(&Key::Tab));
    }

    #[test]
    fn shortcut_letters_follow_shift() {
        assert_eq!(shortcut_char(b'Z' as u16, false), Some('z'), "Ctrl+Z = annuler pour Slint");
        assert_eq!(shortcut_char(b'Z' as u16, true), Some('Z'));
        assert_eq!(shortcut_char(b'1' as u16, false), Some('1'));
        assert_eq!(shortcut_char(vk::SPACE, false), Some(' '));
        assert_eq!(shortcut_char(vk::LEFT, false), None);
    }

    #[test]
    fn control_characters_are_not_text() {
        assert!(is_text('é'));
        assert!(is_text(' '));
        assert!(!is_text('\u{8}'), "Retour arrière déjà donné par WM_KEYDOWN");
        assert!(!is_text('\r'));
        assert!(!is_text('\u{1a}'), "Ctrl+Z");
        assert!(!is_text('\u{7f}'));
    }

    #[test]
    fn surrogate_pairs_are_joined() {
        let mut d = Utf16::default();
        assert_eq!(d.push('é' as u16), Some('é'));
        let mut units = [0u16; 2];
        '😀'.encode_utf16(&mut units);
        assert_eq!(d.push(units[0]), None);
        assert_eq!(d.push(units[1]), Some('😀'));
        assert_eq!(d.push(units[1]), None, "moitié basse orpheline ignorée");
    }
}
