//! Plateforme Slint maison : fenêtres Win32 rendues par le moteur logiciel de Slint
//! (`window.rs`), boucle d'événements Win32 + minuteurs Slint, presse-papiers.
//!
//! Pourquoi : avec winit, le moteur logiciel passe par softbuffer (BitBlt GDI), sans alpha par
//! pixel ; les moteurs OpenGL coûtent un pilote de ~180 Mo. Une fenêtre layered n'a ni pilote GPU
//! ni chaîne d'échange (mesurée : ~50 Mo pour quatre barres, contre ~450 Mo avec WebView2).
//!
//! Une seule plateforme par processus : toutes les fenêtres d'Horizon passent par elle.
//! Boucle : `invoke_from_event_loop` dépose une fermeture dans une file et réveille le fil par
//! un message de fil (`WM_APP`) ; rien ne tourne entre deux événements (attente bloquante
//! jusqu'au prochain minuteur Slint), sauf pendant une animation Slint (~60 images/s, bornée par
//! la durée de l'animation).

mod clipboard;
mod dib;
mod ime;
pub mod keys;
mod window;

pub use window::{Frame, WindowSpec, activate, configure, hwnd, on_active, on_context, scale, set_focusable};

use slint::platform::{Clipboard, EventLoopProxy, Platform, WindowAdapter};
use slint::PlatformError;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetCaretBlinkTime, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, PostThreadMessageW, QS_ALLINPUT, TranslateMessage, WM_APP, WM_QUIT,
};

type Job = Box<dyn FnOnce() + Send>;

/// Fenêtre rendue dans un tampon (plugins : widget posé ensuite comme image).
pub type Offscreen = slint::platform::software_renderer::MinimalSoftwareWindow;

thread_local! {
    /// La prochaine fenêtre créée est hors écran (`offscreen`).
    static OFFSCREEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static LAST_OFFSCREEN: std::cell::RefCell<Option<Rc<Offscreen>>> = const { std::cell::RefCell::new(None) };
}

/// Crée ce que fait `f` (un composant) dans une fenêtre hors écran, rendue à la demande dans un
/// tampon ; `None` si `f` n'a créé aucune fenêtre.
pub fn offscreen<T>(f: impl FnOnce() -> T) -> (T, Option<Rc<Offscreen>>) {
    OFFSCREEN.set(true);
    let out = f();
    OFFSCREEN.set(false);
    (out, LAST_OFFSCREEN.with_borrow_mut(Option::take))
}

struct NativePlatform {
    start: Instant,
    thread: u32,
    jobs: Arc<Mutex<VecDeque<Job>>>,
}

/// Installe la plateforme ; le fil courant devient le fil de l'interface.
pub fn install() -> anyhow::Result<()> {
    // SAFETY : sans argument.
    let thread = unsafe { GetCurrentThreadId() };
    slint::platform::set_platform(Box::new(NativePlatform { start: Instant::now(), thread, jobs: Arc::default() }))
        .map_err(|e| anyhow::anyhow!("plateforme Slint : {e}"))?;
    window::register_class();
    Ok(())
}

impl NativePlatform {
    /// Minuteurs, travaux déposés, images ; renvoie vrai si une animation est en cours.
    fn turn(&self) -> bool {
        slint::platform::update_timers_and_animations();
        let jobs: Vec<Job> = self.jobs.lock().map(|mut q| q.drain(..).collect()).unwrap_or_default();
        for job in jobs {
            job();
        }
        let mut animating = false;
        for w in window::live() {
            w.render();
            animating |= w.window().has_active_animations();
        }
        animating
    }
}

impl Platform for NativePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        if OFFSCREEN.get() {
            let w = Offscreen::new(slint::platform::software_renderer::RepaintBufferType::NewBuffer);
            LAST_OFFSCREEN.with_borrow_mut(|l| *l = Some(w.clone()));
            return Ok(w);
        }
        Ok(window::new_adapter())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn click_interval(&self) -> Duration {
        // SAFETY : sans argument.
        Duration::from_millis(unsafe { GetDoubleClickTime() } as u64)
    }

    fn cursor_flash_cycle(&self) -> Duration {
        // SAFETY : sans argument. INFINITE (u32::MAX) = curseur fixe.
        match unsafe { GetCaretBlinkTime() } {
            0 | u32::MAX => Duration::ZERO,
            ms => Duration::from_millis(ms as u64 * 2),
        }
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(Proxy { thread: self.thread, jobs: self.jobs.clone() }))
    }

    fn set_clipboard_text(&self, text: &str, clipboard: Clipboard) {
        if clipboard == Clipboard::DefaultClipboard {
            clipboard::set_text(text);
        }
    }

    fn clipboard_text(&self, clipboard: Clipboard) -> Option<String> {
        (clipboard == Clipboard::DefaultClipboard).then(clipboard::text).flatten()
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let mut msg = MSG::default();
        loop {
            let animating = self.turn();
            let wait = if animating {
                Duration::from_millis(16)
            } else {
                slint::platform::duration_until_next_timer_update().unwrap_or(Duration::from_secs(3600))
            };
            // SAFETY : attente sans poignée ; messages de toutes sortes.
            unsafe {
                MsgWaitForMultipleObjectsEx(None, wait.as_millis().min(u32::MAX as u128 - 1) as u32, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        return Ok(());
                    }
                    if msg.hwnd.is_invalid() && msg.message == WM_APP {
                        continue; // réveil : la file est vidée au tour suivant
                    }
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

struct Proxy {
    thread: u32,
    jobs: Arc<Mutex<VecDeque<Job>>>,
}

impl Proxy {
    fn post(&self, msg: u32) -> Result<(), slint::EventLoopError> {
        // SAFETY : message de fil sans pointeur.
        unsafe { PostThreadMessageW(self.thread, msg, WPARAM(0), LPARAM(0)) }
            .map_err(|_| slint::EventLoopError::EventLoopTerminated)
    }
}

impl EventLoopProxy for Proxy {
    fn quit_event_loop(&self) -> Result<(), slint::EventLoopError> {
        self.post(WM_QUIT)
    }

    fn invoke_from_event_loop(&self, event: Box<dyn FnOnce() + Send>) -> Result<(), slint::EventLoopError> {
        self.jobs.lock().map_err(|_| slint::EventLoopError::EventLoopTerminated)?.push_back(event);
        self.post(WM_APP)
    }
}
