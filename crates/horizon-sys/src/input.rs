//! Saisie à distance (clavier et souris du téléphone) par `SendInput`. Les événements sont
//! construits par des fonctions sans effet, testées ; seules les fonctions publiques envoient.
//! Windows protège déjà l'écran de connexion, les demandes d'administrateur et les fenêtres
//! lancées en administrateur : une saisie simulée ne les atteint pas.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSE_EVENT_FLAGS, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};

/// Bouton de la souris.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

/// Touche Ctrl (code de touche virtuelle), tenue pendant un zoom.
const VK_CONTROL: u16 = 0x11;

/// Touches qui demandent `KEYEVENTF_EXTENDEDKEY` (flèches, pavé d'édition, Windows) : sans lui,
/// Windows les lit comme celles du pavé numérique.
fn extended(vk: u16) -> bool {
    matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B | 0x5C | 0x2C)
}

fn keyboard(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn mouse(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

/// Le texte en caractères Unicode, une unité UTF-16 à la fois (appui puis relâche) : toute langue,
/// quelle que soit la disposition du clavier du PC.
fn text_inputs(text: &str) -> Vec<INPUT> {
    text.encode_utf16()
        .flat_map(|unit| [keyboard(0, unit, KEYEVENTF_UNICODE), keyboard(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)])
        .collect()
}

/// Les modificateurs `mods` enfoncés, la touche `vk` appuyée puis relâchée, les modificateurs
/// relâchés dans l'ordre inverse.
fn key_inputs(vk: u16, mods: &[u16]) -> Vec<INPUT> {
    let flag = |code: u16| if extended(code) { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
    let mut out: Vec<INPUT> = mods.iter().map(|&m| keyboard(m, 0, flag(m))).collect();
    out.push(keyboard(vk, 0, flag(vk)));
    out.push(keyboard(vk, 0, flag(vk) | KEYEVENTF_KEYUP));
    out.extend(mods.iter().rev().map(|&m| keyboard(m, 0, flag(m) | KEYEVENTF_KEYUP)));
    out
}

fn mouse_move_input(dx: i32, dy: i32) -> INPUT {
    mouse(dx, dy, 0, MOUSEEVENTF_MOVE)
}

/// Molette : `delta` en unités (120 = un cran, positif = vers le haut ou vers la droite).
fn wheel_input(delta: i32, horizontal: bool) -> INPUT {
    mouse(0, 0, delta, if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL })
}

fn button_inputs(button: Button, down: bool, up: bool) -> Vec<INPUT> {
    let (press, release) = match button {
        Button::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        Button::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        Button::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let mut out = Vec::new();
    if down {
        out.push(mouse(0, 0, 0, press));
    }
    if up {
        out.push(mouse(0, 0, 0, release));
    }
    out
}

fn send(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    // SAFETY : tableau d'INPUT valides, taille de la structure exacte.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        log::debug!("saisie : {sent} événements sur {} acceptés par Windows", inputs.len());
    }
}

/// Déplace le pointeur de (`dx`, `dy`) pixels, comme une souris (accélération de Windows comprise).
pub fn move_by(dx: i32, dy: i32) {
    send(&[mouse_move_input(dx, dy)]);
}

/// Appuie (`down`), relâche (`up`) ou clique (les deux) le bouton `button`.
pub fn button(button: Button, down: bool, up: bool) {
    send(&button_inputs(button, down, up));
}

/// Fait défiler : `dy` vertical, `dx` horizontal, en unités de molette.
pub fn wheel(dx: i32, dy: i32) {
    let mut inputs = Vec::new();
    if dy != 0 {
        inputs.push(wheel_input(dy, false));
    }
    if dx != 0 {
        inputs.push(wheel_input(dx, true));
    }
    send(&inputs);
}

/// Zoome : Ctrl tenu pendant un tour de molette de `delta` unités.
pub fn zoom(delta: i32) {
    let mut inputs = vec![keyboard(VK_CONTROL, 0, KEYBD_EVENT_FLAGS(0)), wheel_input(delta, false)];
    inputs.push(keyboard(VK_CONTROL, 0, KEYEVENTF_KEYUP));
    send(&inputs);
}

/// Tape `text` en Unicode.
pub fn text(text: &str) {
    send(&text_inputs(text));
}

/// Appuie sur la touche `vk` avec les modificateurs `mods` (codes de touches virtuelles).
pub fn key(vk: u16, mods: &[u16]) {
    send(&key_inputs(vk, mods));
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::{INPUT_KEYBOARD, INPUT_MOUSE, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE};

    #[test]
    fn text_becomes_unicode_presses_and_releases() {
        let events = text_inputs("é☃");
        assert_eq!(events.len(), 4, "deux caractères, appui et relâche");
        let ki = unsafe { events[0].Anonymous.ki };
        assert_eq!(events[0].r#type, INPUT_KEYBOARD);
        assert_eq!((ki.wScan, ki.dwFlags), (u16::from(b'\xe9'), KEYEVENTF_UNICODE));
        assert_eq!(unsafe { events[1].Anonymous.ki }.dwFlags, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP);
        assert_eq!(text_inputs("😀").len(), 4, "hors du plan de base : deux unités UTF-16");
    }

    #[test]
    fn a_shortcut_holds_its_modifiers_around_the_key() {
        let events = key_inputs(0x43, &[0x11, 0x10]);
        let keys: Vec<(u16, bool)> = events.iter().map(|e| unsafe { (e.Anonymous.ki.wVk.0, e.Anonymous.ki.dwFlags.contains(KEYEVENTF_KEYUP)) }).collect();
        assert_eq!(keys, vec![(0x11, false), (0x10, false), (0x43, false), (0x43, true), (0x10, true), (0x11, true)]);
    }

    #[test]
    fn arrows_are_extended_keys() {
        let ki = unsafe { key_inputs(0x25, &[])[0].Anonymous.ki };
        assert!(ki.dwFlags.contains(windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY));
        let a = unsafe { key_inputs(0x41, &[])[0].Anonymous.ki };
        assert!(!a.dwFlags.contains(windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY));
    }

    #[test]
    fn mouse_events_carry_their_values() {
        let m = mouse_move_input(12, -5);
        assert_eq!(m.r#type, INPUT_MOUSE);
        let mi = unsafe { m.Anonymous.mi };
        assert_eq!((mi.dx, mi.dy), (12, -5));
        let w = unsafe { wheel_input(-120, false).Anonymous.mi };
        assert_eq!(w.mouseData as i32, -120);
        assert_eq!(button_inputs(Button::Right, true, true).len(), 2, "un clic : appui et relâche");
        assert_eq!(button_inputs(Button::Left, true, false).len(), 1, "appui seul (glisser)");
    }
}
