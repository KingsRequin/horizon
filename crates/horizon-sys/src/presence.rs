//! Présence de l'utilisateur : session verrouillée (`WTS_SESSION_LOCK` / `UNLOCK`) ou écran
//! éteint (`GUID_CONSOLE_DISPLAY_STATE`). Pendant une absence, les fils qui ne servent qu'à
//! l'affichage (mesures, sondages, masquage) s'arrêtent avec [`wait_while_away`] et reprennent
//! au retour.
//!
//! Les deux avis arrivent sur une fenêtre « message seulement » tenue par un fil dédié.

use crossbeam_channel::Sender;
use std::sync::{Condvar, Mutex, OnceLock};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    PBT_POWERSETTINGCHANGE, RegisterClassW, WINDOW_EX_STYLE, WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW,
    WS_OVERLAPPED,
};
use windows::core::w;

const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct State {
    locked: bool,
    display_off: bool,
}

impl State {
    fn away(self) -> bool {
        self.locked || self.display_off
    }
}

static STATE: Mutex<State> = Mutex::new(State { locked: false, display_off: false });
static CHANGED: Condvar = Condvar::new();
static SINK: OnceLock<Sender<bool>> = OnceLock::new();

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// L'utilisateur est absent (session verrouillée ou écran éteint).
pub fn away() -> bool {
    state().away()
}

/// Bloque tant que l'utilisateur est absent ; rend la main tout de suite sinon.
pub fn wait_while_away() {
    let g = state();
    let _g = CHANGED.wait_while(g, |s| s.away()).unwrap_or_else(|e| e.into_inner());
}

fn update(f: impl FnOnce(&mut State)) {
    let (before, after) = {
        let mut s = state();
        let before = s.away();
        f(&mut s);
        (before, s.away())
    };
    if before != after {
        CHANGED.notify_all();
        if let Some(tx) = SINK.get() {
            let _ = tx.send(after);
        }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_WTSSESSION_CHANGE => match wp.0 {
            WTS_SESSION_LOCK => update(|s| s.locked = true),
            WTS_SESSION_UNLOCK => update(|s| s.locked = false),
            _ => {}
        },
        WM_POWERBROADCAST if wp.0 as u32 == PBT_POWERSETTINGCHANGE && lp.0 != 0 => {
            // SAFETY : avec PBT_POWERSETTINGCHANGE, `lp` pointe vers un POWERBROADCAST_SETTING
            // valide le temps du message.
            let s = unsafe { &*(lp.0 as *const POWERBROADCAST_SETTING) };
            if s.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && s.DataLength >= 1 {
                // 0 = éteint, 1 = allumé, 2 = atténué (encore lisible).
                let off = s.Data[0] == 0;
                update(|st| st.display_off = off);
            }
        }
        _ => {}
    }
    // SAFETY : traitement par défaut des autres messages.
    unsafe { DefWindowProcW(h, msg, wp, lp) }
}

fn run() -> anyhow::Result<()> {
    // SAFETY : classe et fenêtre « message seulement » de ce fil, avec une procédure statique ;
    // les abonnements vivent autant que la fenêtre (celle du processus).
    unsafe {
        let class = WNDCLASSW { lpfnWndProc: Some(wndproc), lpszClassName: w!("HorizonPresence"), ..Default::default() };
        if RegisterClassW(&class) == 0 {
            anyhow::bail!("classe de fenêtre refusée");
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("HorizonPresence"),
            w!("Horizon"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )?;
        if let Err(e) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
            log::warn!("présence : avis de verrouillage indisponibles : {e}");
        }
        if let Err(e) =
            RegisterPowerSettingNotification(HANDLE(hwnd.0), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE)
        {
            log::warn!("présence : avis d'extinction de l'écran indisponibles : {e}");
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// Démarre la surveillance ; chaque passage absent (`true`) / présent (`false`) est aussi
/// envoyé sur `sink`.
pub fn watch(sink: Sender<bool>) -> anyhow::Result<()> {
    let _ = SINK.set(sink);
    std::thread::Builder::new().name("presence".into()).spawn(|| {
        if let Err(e) = run() {
            log::warn!("présence : surveillance impossible : {e:#}");
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn away_when_locked_or_display_off() {
        assert!(!State::default().away());
        assert!(State { locked: true, display_off: false }.away());
        assert!(State { locked: false, display_off: true }.away());
    }
}
