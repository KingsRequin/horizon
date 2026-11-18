//! Raccourci clavier global (`RegisterHotKey`), écouté sur un fil dédié qui garde sa file de
//! messages pour recevoir `WM_HOTKEY`.

use horizon_core::hotkey::Hotkey;
use windows::Win32::Foundation::ERROR_HOTKEY_ALREADY_REGISTERED;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
};
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT};

/// Raccourci enregistré ; `stop` le libère (pour en enregistrer un autre à chaud).
#[derive(Debug)]
pub struct HotkeyHandle {
    thread: u32,
}

impl HotkeyHandle {
    /// Arrête l'écoute : le fil libère le raccourci et se termine.
    pub fn stop(self) {
        // SAFETY : message posté à la file du fil du raccourci (sans effet s'il est fini).
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, Default::default(), Default::default()) };
    }
}

/// Enregistre `key` et appelle `on_press` (sur le fil du raccourci) à chaque appui. Renvoie
/// une erreur lisible si le raccourci est déjà pris par une autre application.
pub fn watch(key: Hotkey, on_press: impl Fn() + Send + 'static) -> anyhow::Result<HotkeyHandle> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("hotkey".into()).spawn(move || {
        let mut mods = MOD_NOREPEAT;
        for (on, m) in [(key.ctrl, MOD_CONTROL), (key.alt, MOD_ALT), (key.shift, MOD_SHIFT), (key.win, MOD_WIN)] {
            if on {
                mods |= m;
            }
        }
        // SAFETY : enregistrement lié à ce fil (pas de fenêtre) ; libéré à la fin de sa boucle.
        let registered = unsafe { RegisterHotKey(None, 1, mods, key.vk) };
        let ok = registered.is_ok();
        // SAFETY : appel sans argument.
        let thread = unsafe { GetCurrentThreadId() };
        let _ = tx.send(registered.map(|()| HotkeyHandle { thread }).map_err(|e| {
            if e.code() == ERROR_HOTKEY_ALREADY_REGISTERED.to_hresult() {
                anyhow::anyhow!("déjà utilisé par une autre application")
            } else {
                anyhow::anyhow!("{e}")
            }
        }));
        if !ok {
            return;
        }
        let mut msg = MSG::default();
        // SAFETY : tampon de message valide ; boucle propre à ce fil.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            if msg.message == WM_HOTKEY {
                on_press();
            }
        }
        // SAFETY : raccourci enregistré par ce fil.
        let _ = unsafe { UnregisterHotKey(None, 1) };
    })?;
    rx.recv().map_err(|_| anyhow::anyhow!("fil du raccourci arrêté"))?
}
