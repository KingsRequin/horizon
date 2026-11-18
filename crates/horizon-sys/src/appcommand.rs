//! Touches multimédia (volume, lecture) vues par le crochet du shell (`RegisterShellHookWindow`,
//! message `HSHELL_APPCOMMAND`) : une fenêtre invisible reçoit ce que Windows annonce au shell,
//! sans crochet clavier global (Defender a déjà bloqué une appli de l'utilisateur pour ça).
//! Clavier et boutons de casque compris.

use crossbeam_channel::Sender;
use std::sync::OnceLock;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW,
    RegisterShellHookWindow, RegisterWindowMessageW, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

/// Touche multimédia reçue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppCommand {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    PlayPause,
    Next,
    Previous,
    Stop,
}

/// `wParam` du message du shell : une commande d'appli que la fenêtre active n'a pas traitée.
const HSHELL_APPCOMMAND: usize = 12;

impl AppCommand {
    /// Commande tirée du `lParam` de `HSHELL_APPCOMMAND` (`GET_APPCOMMAND_LPARAM`).
    pub fn from_lparam(lparam: isize) -> Option<AppCommand> {
        let cmd = ((lparam as usize >> 16) & 0x0FFF) as u32;
        Some(match cmd {
            8 => AppCommand::VolumeMute,
            9 => AppCommand::VolumeDown,
            10 => AppCommand::VolumeUp,
            11 => AppCommand::Next,
            12 => AppCommand::Previous,
            13 => AppCommand::Stop,
            // Lecture/pause, lecture seule, pause seule.
            14 | 46 | 47 => AppCommand::PlayPause,
            _ => return None,
        })
    }

    pub fn is_volume(self) -> bool {
        matches!(self, AppCommand::VolumeUp | AppCommand::VolumeDown | AppCommand::VolumeMute)
    }
}

static SINK: OnceLock<Sender<AppCommand>> = OnceLock::new();
static SHELLHOOK: OnceLock<u32> = OnceLock::new();

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if SHELLHOOK.get() == Some(&msg) && wp.0 == HSHELL_APPCOMMAND {
        if let (Some(cmd), Some(tx)) = (AppCommand::from_lparam(lp.0), SINK.get()) {
            let _ = tx.send(cmd);
        }
        // Le shell continue de traiter la touche (le volume change bien).
        return LRESULT(0);
    }
    // SAFETY : traitement par défaut des autres messages.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Démarre l'écoute (une seule fois par processus) sur un fil à elle.
pub fn watch(tx: Sender<AppCommand>) -> anyhow::Result<()> {
    SINK.set(tx).map_err(|_| anyhow::anyhow!("touches multimédia déjà suivies"))?;
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    std::thread::Builder::new().name("touches-multimedia".into()).spawn(move || {
        // SAFETY : fenêtre créée et utilisée sur ce seul fil ; boucle de messages jusqu'à la
        // fin du processus.
        unsafe {
            let result = (|| -> anyhow::Result<HWND> {
                let instance = GetModuleHandleW(None)?;
                let class = w!("HorizonShellHook");
                let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance.into(), lpszClassName: class, ..Default::default() };
                RegisterClassW(&wc);
                // Fenêtre de premier niveau invisible (une fenêtre « message seulement » ne reçoit
                // pas les messages du shell).
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                    class,
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )?;
                let _ = SHELLHOOK.set(RegisterWindowMessageW(w!("SHELLHOOK")));
                anyhow::ensure!(RegisterShellHookWindow(hwnd).as_bool(), "crochet du shell refusé");
                Ok(hwnd)
            })();
            let ok = result.is_ok();
            let _ = ready_tx.send(result.map(|_| ()));
            if !ok {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    })?;
    ready_rx.recv()?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lparam(cmd: u32, device: u32) -> isize {
        (((device | cmd) as usize) << 16) as isize
    }

    #[test]
    fn decodes_command_and_ignores_device_bits() {
        // FAPPCOMMAND_KEY = 0, FAPPCOMMAND_OEM = 0x1000 (casque).
        assert_eq!(AppCommand::from_lparam(lparam(10, 0)), Some(AppCommand::VolumeUp));
        assert_eq!(AppCommand::from_lparam(lparam(9, 0x1000)), Some(AppCommand::VolumeDown));
        assert_eq!(AppCommand::from_lparam(lparam(14, 0)), Some(AppCommand::PlayPause));
        assert_eq!(AppCommand::from_lparam(lparam(47, 0)), Some(AppCommand::PlayPause));
        assert_eq!(AppCommand::from_lparam(lparam(1, 0)), None, "page précédente du navigateur");
        assert!(AppCommand::VolumeMute.is_volume() && !AppCommand::Next.is_volume());
    }
}
