//! Une fenêtre Slint = une fenêtre Win32, rendue par le `SoftwareRenderer` de Slint dans une
//! section DIB.
//!
//! Deux présentations :
//! - [`Frame::Layered`] : sans cadre, alpha par pixel (`WS_EX_LAYERED`, `UpdateLayeredWindowIndirect`
//!   avec le seul rectangle modifié) : barres, panneaux, palette. Un pixel d'alpha nul laisse
//!   passer la souris : un fond « invisible » à capter doit avoir un alpha d'au moins 1/255.
//! - [`Frame::Normal`] : fenêtre Windows ordinaire (cadre, redimensionnable), opaque, copiée par
//!   `BitBlt` sur `WM_PAINT` : réglages, Prisme.
//!
//! Entrées : souris (survol, sortie, boutons, molette, capture), clavier (`keys.rs`), focus,
//! curseurs (forme demandée par Slint), DPI par écran (`WM_DPICHANGED`). Une fenêtre qui ne
//! prend pas le focus (`focusable: false`) ne s'active jamais, même cliquée (`WS_EX_NOACTIVATE`).

use super::dib::Dib;
use super::keys::{self, vk};
use i_slint_core::cursor::MouseCursorInner;
use i_slint_core::items::BuiltInMouseCursor;
use i_slint_core::window::WindowAdapterInternal;
use slint::platform::software_renderer::{RepaintBufferType, SoftwareRenderer};
use slint::platform::{PointerEventButton, Renderer, WindowAdapter, WindowEvent, WindowProperties};
use slint::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize, PlatformError, SharedString, WindowPosition, WindowSize};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, BeginPaint, BitBlt, EndPaint, GetMonitorInfoW, HDC, InvalidateRect,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SRCCOPY,
    ScreenToClient, UpdateWindow,
};
use windows::Win32::Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetMessageTime, GetWindowRect, HCURSOR, HTCLIENT,
    HWND_TOPMOST, IDC_APPSTARTING, IDC_ARROW, IDC_CROSS, IDC_HAND, IDC_HELP, IDC_IBEAM, IDC_NO, IDC_SIZEALL,
    IDC_SIZENESW, IDC_SIZENS, IDC_SIZENWSE, IDC_SIZEWE, IDC_WAIT, LoadCursorW, LoadIconW, MA_NOACTIVATE, MINMAXINFO, MSG,
    PM_NOREMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetCursor, SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, ULW_ALPHA,
    UPDATELAYEREDWINDOWINFO, UpdateLayeredWindowIndirect, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CHAR, WM_CLOSE,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETCURSOR, WM_SETFOCUS, WM_SIZE, WM_SYSCHAR, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_OVERLAPPEDWINDOW, WS_POPUP, GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WM_ACTIVATE, WA_INACTIVE,
};
use windows::core::{HSTRING, PCWSTR, w};

/// Absent des liaisons de `windows` pour ce module.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Pixels logiques par cran de molette (comme le moteur winit de Slint).
const WHEEL_PX: f32 = 60.0;

/// Présentation d'une fenêtre (voir l'en-tête du module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Layered,
    Normal,
}

/// Ce qu'est une fenêtre, fixé avant sa création (`configure`).
#[derive(Debug, Clone)]
pub struct WindowSpec {
    pub title: String,
    pub frame: Frame,
    /// Rectangle en pixels physiques (zone cliente) ; `None` : `size` (logique) au centre de la
    /// zone de travail de l'écran principal.
    pub rect: Option<(i32, i32, u32, u32)>,
    pub size: (f32, f32),
    /// Peut prendre le focus (saisie de texte, clavier). Faux : ne s'active jamais.
    pub focusable: bool,
    pub topmost: bool,
    /// Présente dans la barre des tâches et Alt+Tab.
    pub taskbar: bool,
}

impl Default for WindowSpec {
    fn default() -> Self {
        WindowSpec {
            title: "Horizon".into(),
            frame: Frame::Normal,
            rect: None,
            size: (640.0, 480.0),
            focusable: true,
            topmost: false,
            taskbar: true,
        }
    }
}

pub struct NativeWindow {
    window: slint::Window,
    renderer: SoftwareRenderer,
    hwnd: Cell<HWND>,
    spec: RefCell<WindowSpec>,
    /// Taille de la zone cliente, pixels physiques.
    size: Cell<PhysicalSize>,
    scale: Cell<f32>,
    dib: RefCell<Option<Dib>>,
    /// Slint a demandé un redessin.
    dirty: Cell<bool>,
    /// Image entière à refaire (première image, taille ou échelle changées).
    full: Cell<bool>,
    visible: Cell<bool>,
    /// Affichage demandé : fait après la prochaine image (une fenêtre layered sans image
    /// présentée n'a pas de contenu).
    show_pending: Cell<bool>,
    /// Sortie de la souris suivie (`TrackMouseEvent`).
    tracking: Cell<bool>,
    /// Boutons de la souris enfoncés (capture tant qu'il en reste).
    buttons: Cell<u8>,
    cursor: Cell<HCURSOR>,
    utf16: RefCell<keys::Utf16>,
    /// Touches enfoncées et texte envoyé à Slint à l'appui (le même au relâchement).
    pressed: RefCell<Vec<(u16, SharedString)>>,
    /// Taille minimale (logique) demandée par l'interface, pour une fenêtre `Normal`.
    min_size: Cell<Option<LogicalSize>>,
    title: RefCell<String>,
    /// Appelé quand la fenêtre est activée (`true`) ou désactivée (voir [`on_active`]).
    on_active: RefCell<Option<ActiveHook>>,
    /// Appelé après un clic droit (voir [`on_context`]).
    on_context: RefCell<Option<ContextHook>>,
}

/// Rappel d'activation d'une fenêtre (`on_active`).
type ActiveHook = Rc<dyn Fn(bool)>;
/// Rappel du clic droit d'une fenêtre (`on_context`) : position logique.
type ContextHook = Rc<dyn Fn(f32, f32)>;

thread_local! {
    /// Fenêtres vivantes du fil de l'interface.
    static WINDOWS: RefCell<Vec<Weak<NativeWindow>>> = const { RefCell::new(Vec::new()) };
}

const CLASS: PCWSTR = w!("HorizonNatifWindow");

pub(super) fn register_class() {
    // SAFETY : classe enregistrée une fois, procédure `wndproc` valide.
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            // Icône de l'exe (ressource 1 de `horizon.rc`) ; absente des tests : icône par défaut.
            hIcon: LoadIconW(Some(hinstance.into()), PCWSTR(1 as _)).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

/// Nouvelle fenêtre Slint (sans fenêtre native tant qu'elle n'est ni configurée ni affichée).
pub(super) fn new_adapter() -> Rc<NativeWindow> {
    let w = Rc::new_cyclic(|weak: &Weak<NativeWindow>| NativeWindow {
        window: slint::Window::new(weak.clone()),
        renderer: SoftwareRenderer::new_with_repaint_buffer_type(RepaintBufferType::ReusedBuffer),
        hwnd: Cell::new(HWND::default()),
        spec: RefCell::new(WindowSpec::default()),
        size: Cell::new(PhysicalSize::default()),
        scale: Cell::new(1.0),
        dib: RefCell::new(None),
        dirty: Cell::new(true),
        full: Cell::new(true),
        visible: Cell::new(false),
        show_pending: Cell::new(false),
        tracking: Cell::new(false),
        buttons: Cell::new(0),
        // SAFETY : curseur système.
        cursor: Cell::new(unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default()),
        utf16: RefCell::default(),
        pressed: RefCell::default(),
        min_size: Cell::new(None),
        title: RefCell::default(),
        on_active: RefCell::default(),
        on_context: RefCell::default(),
    });
    WINDOWS.with_borrow_mut(|v| {
        v.retain(|x| x.strong_count() > 0);
        v.push(Rc::downgrade(&w));
    });
    w
}

/// Fenêtres vivantes (copie : aucun emprunt n'est gardé pendant leur usage).
pub(super) fn live() -> Vec<Rc<NativeWindow>> {
    WINDOWS.with_borrow(|v| v.iter().filter_map(Weak::upgrade).collect())
}

fn find(hwnd: HWND) -> Option<Rc<NativeWindow>> {
    WINDOWS.try_with(|v| v.try_borrow().ok()?.iter().filter_map(Weak::upgrade).find(|w| w.hwnd.get() == hwnd)).ok()?
}

fn of(window: &slint::Window) -> Option<Rc<NativeWindow>> {
    live().into_iter().find(|w| std::ptr::eq(&w.window, window))
}

/// Donne à la fenêtre Slint `window` sa fenêtre native, décrite par `spec` (avant `show()`).
/// Renvoie son HWND. Une fenêtre déjà créée garde son cadre ; sa place, et le droit de prendre
/// le focus (`focusable` : panneau surgissant épinglé), suivent.
pub fn configure(window: &slint::Window, spec: WindowSpec) -> anyhow::Result<isize> {
    let w = of(window).ok_or_else(|| anyhow::anyhow!("fenêtre Slint inconnue de la plateforme"))?;
    let focusable = std::mem::replace(&mut *w.spec.borrow_mut(), spec).focusable;
    if w.hwnd.get().is_invalid() {
        w.create()?;
    } else {
        if focusable != w.spec.borrow().focusable {
            w.update_activation();
        }
        if let Some((x, y, width, height)) = w.spec.borrow().rect {
            w.place(x, y, width, height);
        }
    }
    Ok(w.hwnd.get().0 as isize)
}

/// `f(actif)` à chaque activation ou désactivation de la fenêtre (focus clavier : panneau
/// épinglé qui se ferme quand on clique ailleurs). Appelé pendant le message de Windows : `f` ne
/// doit ni cacher ni détruire de fenêtre (différer avec un minuteur).
pub fn on_active(window: &slint::Window, f: impl Fn(bool) + 'static) {
    if let Some(w) = of(window) {
        *w.on_active.borrow_mut() = Some(Rc::new(f));
    }
}

/// `f(x, y)` après chaque clic droit relâché dans la fenêtre (position logique), une fois
/// l'événement passé à l'interface : menu contextuel de toute la fenêtre (le `TouchArea` d'un
/// élément garde le clic pour lui, son parent ne le voit pas). Même règle que `on_active`.
pub fn on_context(window: &slint::Window, f: impl Fn(f32, f32) + 'static) {
    if let Some(w) = of(window) {
        *w.on_context.borrow_mut() = Some(Rc::new(f));
    }
}

/// Fenêtre déjà créée qui peut prendre le focus (`true` : mode édition d'une barre, qui a besoin
/// du clavier) ou jamais (`false`), sans toucher au reste de sa description.
pub fn set_focusable(window: &slint::Window, focusable: bool) {
    let Some(w) = of(window) else { return };
    let before = std::mem::replace(&mut w.spec.borrow_mut().focusable, focusable);
    if before != focusable && !w.hwnd.get().is_invalid() {
        w.update_activation();
    }
}

/// Met au premier plan une fenêtre affichée qui peut prendre le focus (sinon rien : une fenêtre
/// en attente d'affichage est activée en apparaissant).
pub fn activate(window: &slint::Window) {
    if let Some(w) = of(window)
        && w.visible.get()
        && !w.show_pending.get()
        && w.spec.borrow().focusable
    {
        horizon_sys::window_focus::bring_to_front(w.hwnd.get().0 as isize);
    }
}

/// HWND de la fenêtre Slint `window`, si elle a sa fenêtre native.
pub fn hwnd(window: &slint::Window) -> Option<isize> {
    of(window).map(|w| w.hwnd.get()).filter(|h| !h.is_invalid()).map(|h| h.0 as isize)
}

/// Échelle de l'écran de la fenêtre (1,5 à 150 %).
pub fn scale(window: &slint::Window) -> Option<f32> {
    of(window).map(|w| w.scale.get())
}

fn x_of(lparam: LPARAM) -> i32 {
    (lparam.0 & 0xffff) as i16 as i32
}

fn y_of(lparam: LPARAM) -> i32 {
    ((lparam.0 >> 16) & 0xffff) as i16 as i32
}

fn key_down(code: u16) -> bool {
    // SAFETY : lecture de l'état du clavier du fil.
    unsafe { GetKeyState(code as i32) < 0 }
}

impl NativeWindow {
    fn styles(spec: &WindowSpec) -> (WINDOW_EX_STYLE, WINDOW_STYLE) {
        let (mut ex, style) = match spec.frame {
            Frame::Layered => (WS_EX_LAYERED, WS_POPUP),
            Frame::Normal => (WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW),
        };
        if !spec.taskbar {
            ex |= WS_EX_TOOLWINDOW;
        }
        if !spec.focusable {
            ex |= WS_EX_NOACTIVATE;
        }
        if spec.topmost {
            ex |= WS_EX_TOPMOST;
        }
        (ex, style)
    }

    /// Rectangle extérieur (cadre compris) pour une zone cliente donnée, à `dpi`.
    fn outer(&self, client: RECT, dpi: u32) -> RECT {
        let spec = self.spec.borrow();
        if spec.frame == Frame::Layered {
            return client;
        }
        let (ex, style) = Self::styles(&spec);
        let mut r = client;
        // SAFETY : rectangle local.
        let _ = unsafe { AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi) };
        r
    }

    fn create(&self) -> anyhow::Result<()> {
        let spec = self.spec.borrow().clone();
        let (ex, style) = Self::styles(&spec);
        let (client, dpi) = match spec.rect {
            Some((x, y, w, h)) => {
                let client = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
                // Fenêtre à cadre : le rectangle donné est sa zone cliente, à l'échelle de son écran.
                (client, (spec.frame == Frame::Normal).then(|| dpi_at(x + w as i32 / 2, y + h as i32 / 2)))
            }
            None => centered(spec.size),
        };
        let r = match dpi {
            Some(d) => self.outer(client, d),
            None => client,
        };
        // SAFETY : classe enregistrée par `install` ; fenêtre de ce fil.
        let hwnd = unsafe {
            CreateWindowExW(
                ex,
                CLASS,
                &HSTRING::from(spec.title.as_str()),
                style,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                None,
                None,
                GetModuleHandleW(None).ok().map(Into::into),
                None,
            )?
        };
        self.hwnd.set(hwnd);
        *self.title.borrow_mut() = spec.title.clone();
        if spec.frame == Frame::Normal {
            // Barre de titre sombre quand Windows l'est (comme les fenêtres de la version Tauri).
            let dark = windows::core::BOOL::from(apps_dark());
            // SAFETY : attribut DWMWA_USE_IMMERSIVE_DARK_MODE (20) d'une fenêtre de ce fil.
            let _ = unsafe {
                DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), (&raw const dark).cast(), size_of::<windows::core::BOOL>() as u32)
            };
        }
        // SAFETY : fenêtre vivante.
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        self.set_scale(scale);
        if spec.frame == Frame::Normal && dpi.is_some_and(|d| d as f32 / 96.0 != scale) {
            // Créée sur un écran d'une autre échelle que prévu : zone cliente recalculée.
            match spec.rect {
                Some((_, _, w, h)) => {
                    let k = scale / (dpi.unwrap_or(96) as f32 / 96.0);
                    self.resize_client((w as f32 * k).round() as u32, (h as f32 * k).round() as u32);
                }
                None => {
                    let (w, h) = spec.size;
                    self.resize_client((w * scale).round() as u32, (h * scale).round() as u32);
                }
            }
        }
        self.sync_client_size();
        Ok(())
    }

    /// `WS_EX_NOACTIVATE` suit `spec.focusable` (fenêtre déjà créée).
    fn update_activation(&self) {
        let hwnd = self.hwnd.get();
        let focusable = self.spec.borrow().focusable;
        // SAFETY : fenêtre de ce fil ; seul le style étendu change.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            let next = if focusable { ex & !flag } else { ex | flag };
            if next != ex {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            }
        }
    }

    fn set_scale(&self, scale: f32) {
        if scale > 0.0 && scale != self.scale.get() {
            self.scale.set(scale);
            self.window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        }
        self.full.set(true);
    }

    /// Zone cliente relue après un changement de taille.
    fn sync_client_size(&self) {
        let mut r = RECT::default();
        // SAFETY : sortie valide.
        let _ = unsafe { GetClientRect(self.hwnd.get(), &mut r) };
        let size = PhysicalSize::new((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32);
        if size != self.size.get() {
            self.size.set(size);
            self.full.set(true);
        }
        // Toujours annoncée : la taille logique change aussi avec l'échelle.
        self.window.dispatch_event(WindowEvent::Resized { size: size.to_logical(self.scale.get()) });
    }

    fn resize_client(&self, width: u32, height: u32) {
        let hwnd = self.hwnd.get();
        // SAFETY : fenêtre vivante.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let r = self.outer(RECT { left: 0, top: 0, right: width as i32, bottom: height as i32 }, dpi);
        // SAFETY : fenêtre de ce fil.
        let _ = unsafe {
            SetWindowPos(hwnd, None, 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE)
        };
    }

    fn place(&self, x: i32, y: i32, width: u32, height: u32) {
        let hwnd = self.hwnd.get();
        // SAFETY : fenêtre vivante.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let r = self.outer(RECT { left: x, top: y, right: x + width as i32, bottom: y + height as i32 }, dpi);
        // SAFETY : fenêtre de ce fil.
        let _ = unsafe {
            SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE)
        };
    }

    /// Dessine ce qui a changé et le présente ; affiche la fenêtre si c'était demandé.
    pub(super) fn render(&self) {
        let hwnd = self.hwnd.get();
        if hwnd.is_invalid() || !self.visible.get() {
            return;
        }
        let show = self.show_pending.get();
        if !(self.dirty.get() || self.full.get() || show) {
            return;
        }
        let size = self.size.get();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let mut slot = self.dib.borrow_mut();
        if slot.as_ref().is_none_or(|d| d.width != size.width || d.height != size.height) {
            match Dib::new(size.width, size.height) {
                Ok(d) => *slot = Some(d),
                Err(e) => {
                    log::error!("section DIB {}×{} : {e:#}", size.width, size.height);
                    return;
                }
            }
            self.full.set(true);
        }
        let Some(dib) = slot.as_mut() else { return };
        let full = self.full.replace(false);
        self.dirty.set(false);
        if full {
            self.renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
        }
        let region = self.renderer.render(dib.pixels(), size.width as usize);
        if full {
            self.renderer.set_repaint_buffer_type(RepaintBufferType::ReusedBuffer);
        }
        let whole = RECT { left: 0, top: 0, right: size.width as i32, bottom: size.height as i32 };
        let (o, s) = (region.bounding_box_origin(), region.bounding_box_size());
        let dirty = if full {
            Some(whole)
        } else if s.width > 0 && s.height > 0 {
            Some(RECT { left: o.x, top: o.y, right: o.x + s.width as i32, bottom: o.y + s.height as i32 })
        } else {
            show.then_some(whole)
        };
        if let Some(dirty) = dirty {
            self.present(dib, size, dirty);
        }
        drop(slot);
        if show {
            self.show_pending.set(false);
            self.show_now();
        }
    }

    fn present(&self, dib: &Dib, size: PhysicalSize, dirty: RECT) {
        let hwnd = self.hwnd.get();
        if self.spec.borrow().frame == Frame::Normal {
            // SAFETY : fenêtre de ce fil ; WM_PAINT copiera la section.
            let _ = unsafe { InvalidateRect(Some(hwnd), Some(&dirty), false) };
            return;
        }
        let psize = SIZE { cx: size.width as i32, cy: size.height as i32 };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            // Position inchangée : la fenêtre est placée par CreateWindowEx / SetWindowPos.
            pptDst: std::ptr::null(),
            psize: &psize,
            hdcSrc: dib.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: &dirty,
        };
        // SAFETY : pointeurs vers des locaux vivants pendant l'appel ; fenêtre layered à nous.
        if !unsafe { UpdateLayeredWindowIndirect(hwnd, &info) }.as_bool() {
            log::warn!("UpdateLayeredWindowIndirect : {}", windows::core::Error::from_thread());
        }
    }

    fn show_now(&self) {
        let hwnd = self.hwnd.get();
        let spec = self.spec.borrow();
        // SAFETY : fenêtre de ce fil.
        unsafe {
            let _ = ShowWindow(hwnd, if spec.focusable { SW_SHOW } else { SW_SHOWNOACTIVATE });
            if spec.topmost {
                let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
        if spec.focusable {
            // Par la voie qui passe le verrou de premier plan de Windows (panneau épinglé ouvert
            // par un clic sur une barre, qui ne s'active jamais).
            drop(spec);
            horizon_sys::window_focus::bring_to_front(hwnd.0 as isize);
        }
    }

    fn dispatch(&self, event: WindowEvent) {
        let _ = self.window.dispatch_event_with_result(event);
    }

    fn position(&self, lparam: LPARAM) -> LogicalPosition {
        let s = self.scale.get();
        LogicalPosition::new(x_of(lparam) as f32 / s, y_of(lparam) as f32 / s)
    }

    fn press(&self, button: PointerEventButton, lparam: LPARAM) {
        let hwnd = self.hwnd.get();
        if self.buttons.replace(self.buttons.get() + 1) == 0 {
            // SAFETY : fenêtre de ce fil.
            unsafe { SetCapture(hwnd) };
        }
        self.dispatch(WindowEvent::PointerPressed { position: self.position(lparam), button });
    }

    fn release(&self, button: PointerEventButton, lparam: LPARAM) {
        let left = self.buttons.get().saturating_sub(1);
        self.buttons.set(left);
        if left == 0 {
            // SAFETY : sans argument.
            let _ = unsafe { ReleaseCapture() };
        }
        self.dispatch(WindowEvent::PointerReleased { position: self.position(lparam), button });
    }

    fn wheel(&self, lparam: LPARAM, wparam: WPARAM, horizontal: bool) {
        // Position de la molette en coordonnées de l'écran.
        let mut p = POINT { x: x_of(lparam), y: y_of(lparam) };
        // SAFETY : fenêtre vivante, point local.
        let _ = unsafe { ScreenToClient(self.hwnd.get(), &mut p) };
        let s = self.scale.get();
        let notches = ((wparam.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
        let (delta_x, delta_y) = if horizontal { (-notches * WHEEL_PX, 0.0) } else { (0.0, notches * WHEEL_PX) };
        self.dispatch(WindowEvent::PointerScrolled {
            position: LogicalPosition::new(p.x as f32 / s, p.y as f32 / s),
            delta_x,
            delta_y,
        });
    }

    fn key_pressed(&self, code: u16, text: SharedString, repeat: bool) {
        if repeat {
            self.dispatch(WindowEvent::KeyPressRepeated { text });
            return;
        }
        self.pressed.borrow_mut().push((code, text.clone()));
        self.dispatch(WindowEvent::KeyPressed { text });
    }

    /// `WM_KEYDOWN` / `WM_SYSKEYDOWN` ; vrai si la touche a été prise.
    fn key_down(&self, code: u16, lparam: LPARAM) -> bool {
        let extended = (lparam.0 >> 24) & 1 == 1;
        let repeat = (lparam.0 >> 30) & 1 == 1;
        if code == vk::CONTROL && !extended && altgr_follows() {
            // Faux Ctrl gauche que Windows envoie avant AltGr : ignoré (sinon le texte tapé avec
            // AltGr passerait pour un raccourci).
            return true;
        }
        let right = if code == vk::SHIFT { (lparam.0 >> 16) as u32 & 0xff == keys::RIGHT_SHIFT_SCAN } else { extended };
        if let Some(key) = keys::special(code, right) {
            if !(repeat && keys::is_modifier(&key)) {
                self.key_pressed(code, key.into(), repeat);
            }
            return true;
        }
        let altgr = key_down(vk::RMENU);
        if (key_down(vk::CONTROL) || key_down(vk::MENU)) && !altgr
            && let Some(c) = keys::shortcut_char(code, key_down(vk::SHIFT))
        {
            self.key_pressed(code, c.to_string().into(), repeat);
            return true;
        }
        false
    }

    /// `WM_KEYUP` / `WM_SYSKEYUP` : relâche ce que l'appui avait envoyé.
    fn key_up(&self, code: u16) -> bool {
        let released: Vec<SharedString> = {
            let mut p = self.pressed.borrow_mut();
            let (gone, kept): (Vec<_>, Vec<_>) = p.drain(..).partition(|(c, _)| *c == code);
            *p = kept;
            gone.into_iter().map(|(_, t)| t).collect()
        };
        let any = !released.is_empty();
        for text in released {
            self.dispatch(WindowEvent::KeyReleased { text });
        }
        any
    }

    fn char_typed(&self, unit: u16) {
        let c = self.utf16.borrow_mut().push(unit);
        if let Some(c) = c.filter(|c| keys::is_text(*c)) {
            let text: SharedString = c.to_string().into();
            self.dispatch(WindowEvent::KeyPressed { text: text.clone() });
            self.dispatch(WindowEvent::KeyReleased { text });
        }
    }

    /// Focus reçu : Slint apprend les modificateurs déjà tenus.
    fn focus_in(&self) {
        let held = [
            (vk::LSHIFT, slint::platform::Key::Shift),
            (vk::RSHIFT, slint::platform::Key::ShiftR),
            (vk::LCONTROL, slint::platform::Key::Control),
            (vk::RCONTROL, slint::platform::Key::ControlR),
            (vk::LMENU, slint::platform::Key::Alt),
            (vk::LWIN, slint::platform::Key::Meta),
        ];
        for (code, key) in held {
            if key_down(code) {
                let base = match code {
                    vk::LSHIFT | vk::RSHIFT => vk::SHIFT,
                    vk::LCONTROL | vk::RCONTROL => vk::CONTROL,
                    vk::LMENU => vk::MENU,
                    c => c,
                };
                self.key_pressed(base, key.into(), false);
            }
        }
        self.dispatch(WindowEvent::WindowActiveChanged(true));
    }

    /// Focus perdu : tout ce qui était enfoncé est relâché (les relâchements iront ailleurs).
    fn focus_out(&self) {
        let all: Vec<SharedString> = self.pressed.borrow_mut().drain(..).map(|(_, t)| t).collect();
        for text in all {
            self.dispatch(WindowEvent::KeyReleased { text });
        }
        self.dispatch(WindowEvent::WindowActiveChanged(false));
    }

    fn paint(&self) {
        let hwnd = self.hwnd.get();
        let mut ps = PAINTSTRUCT::default();
        // SAFETY : WM_PAINT de notre fenêtre ; la section DIB vit pendant la copie.
        unsafe {
            let hdc = BeginPaint(hwnd, &mut ps);
            if let Some(d) = self.dib.borrow().as_ref() {
                let r = ps.rcPaint;
                let _ = BitBlt(hdc, r.left, r.top, r.right - r.left, r.bottom - r.top, Some(d.dc), r.left, r.top, SRCCOPY);
            }
            let _ = EndPaint(hwnd, &ps);
        }
    }

    fn handle(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let hwnd = self.hwnd.get();
        match msg {
            WM_MOUSEMOVE => {
                if !self.tracking.replace(true) {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY : structure initialisée, fenêtre à nous.
                    let _ = unsafe { TrackMouseEvent(&mut tme) };
                }
                self.dispatch(WindowEvent::PointerMoved { position: self.position(lparam) });
            }
            WM_MOUSELEAVE => {
                self.tracking.set(false);
                self.dispatch(WindowEvent::PointerExited);
            }
            WM_LBUTTONDOWN => self.press(PointerEventButton::Left, lparam),
            WM_LBUTTONUP => self.release(PointerEventButton::Left, lparam),
            WM_RBUTTONDOWN => self.press(PointerEventButton::Right, lparam),
            WM_RBUTTONUP => {
                self.release(PointerEventButton::Right, lparam);
                let hook = self.on_context.borrow().clone();
                if let Some(f) = hook {
                    let p = self.position(lparam);
                    f(p.x, p.y);
                }
            }
            WM_MBUTTONDOWN => self.press(PointerEventButton::Middle, lparam),
            WM_MBUTTONUP => self.release(PointerEventButton::Middle, lparam),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let button =
                    if (wparam.0 >> 16) & 0xffff == 1 { PointerEventButton::Back } else { PointerEventButton::Forward };
                if msg == WM_XBUTTONDOWN {
                    self.press(button, lparam);
                } else {
                    self.release(button, lparam);
                }
                return Some(LRESULT(1));
            }
            WM_MOUSEWHEEL => self.wheel(lparam, wparam, false),
            WM_MOUSEHWHEEL => self.wheel(lparam, wparam, true),
            WM_SETCURSOR if (lparam.0 & 0xffff) as u32 == HTCLIENT => {
                // SAFETY : curseur chargé par `set_mouse_cursor` (ou aucun).
                unsafe { SetCursor(Some(self.cursor.get())) };
                return Some(LRESULT(1));
            }
            WM_MOUSEACTIVATE if !self.spec.borrow().focusable => return Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let code = wparam.0 as u16;
                // Alt+F4 ferme une fenêtre ordinaire (WM_CLOSE par le traitement par défaut).
                let alt_f4 = msg == WM_SYSKEYDOWN && code == vk::F1 + 3;
                if !alt_f4 && self.key_down(code, lparam) {
                    return Some(LRESULT(0));
                }
                return None;
            }
            WM_KEYUP | WM_SYSKEYUP => {
                // Alt relâché : pas de menu système (la fenêtre n'en a pas l'usage).
                return if self.key_up(wparam.0 as u16) { Some(LRESULT(0)) } else { None };
            }
            WM_CHAR => self.char_typed(wparam.0 as u16),
            // Alt+lettre : pas de bip (déjà donné en raccourci par WM_SYSKEYDOWN).
            WM_SYSCHAR => {}
            WM_ACTIVATE => {
                let active = (wparam.0 & 0xffff) as u32 != WA_INACTIVE;
                let hook = self.on_active.borrow().clone();
                if let Some(f) = hook {
                    f(active);
                }
                return None;
            }
            WM_SETFOCUS => self.focus_in(),
            WM_KILLFOCUS => self.focus_out(),
            WM_SIZE => {
                self.sync_client_size();
                if self.spec.borrow().frame == Frame::Normal {
                    // Redimensionnement à la souris : Windows tient sa propre boucle, l'image est
                    // refaite ici.
                    self.render();
                    // SAFETY : fenêtre de ce fil.
                    let _ = unsafe { UpdateWindow(hwnd) };
                }
            }
            WM_DPICHANGED => {
                self.set_scale((wparam.0 & 0xffff) as f32 / 96.0);
                if self.spec.borrow().frame == Frame::Normal {
                    // SAFETY : Windows passe le rectangle conseillé dans lParam.
                    let r = unsafe { *(lparam.0 as *const RECT) };
                    // SAFETY : fenêtre de ce fil.
                    let _ = unsafe {
                        SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE)
                    };
                }
                // Une fenêtre sans cadre (barre) est replacée par son propriétaire.
                self.sync_client_size();
            }
            WM_GETMINMAXINFO => {
                if let Some(min) = self.min_size.get() {
                    let s = self.scale.get();
                    // SAFETY : fenêtre vivante.
                    let dpi = unsafe { GetDpiForWindow(hwnd) };
                    let r = self.outer(
                        RECT { left: 0, top: 0, right: (min.width * s).ceil() as i32, bottom: (min.height * s).ceil() as i32 },
                        dpi,
                    );
                    // SAFETY : Windows passe une MINMAXINFO dans lParam.
                    let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                    info.ptMinTrackSize = POINT { x: r.right - r.left, y: r.bottom - r.top };
                    return Some(LRESULT(0));
                }
                return None;
            }
            WM_PAINT if self.spec.borrow().frame == Frame::Normal => self.paint(),
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_CLOSE => self.dispatch(WindowEvent::CloseRequested),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

/// Le message suivant est-il l'appui d'Alt de droite, envoyé au même instant (AltGr) ?
fn altgr_follows() -> bool {
    let mut next = MSG::default();
    // SAFETY : lecture sans retrait de la file du fil.
    unsafe {
        PeekMessageW(&mut next, None, WM_KEYDOWN, WM_SYSKEYDOWN, PM_NOREMOVE).as_bool()
            && (next.message == WM_KEYDOWN || next.message == WM_SYSKEYDOWN)
            && next.wParam.0 as u16 == vk::MENU
            && (next.lParam.0 >> 24) & 1 == 1
            && next.time == GetMessageTime() as u32
    }
}

/// Points par pouce de l'écran qui contient le point physique (`x`, `y`).
fn dpi_at(x: i32, y: i32) -> u32 {
    // SAFETY : appels de lecture ; sorties locales.
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        dx
    }
}

/// Windows est-il en thème sombre pour les applis ? (`ShouldAppsUseDarkMode` d'uxtheme, numéro
/// d'ordre 132, comme muda et tao, et comme les menus de `ui::menu`.)
fn apps_dark() -> bool {
    // SAFETY : bibliothèque système ; fonction sans argument lue par son numéro d'ordre
    // (Windows 10 1903 et suivants).
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("uxtheme.dll")) else { return false };
        let Some(f) = GetProcAddress(lib, windows::core::PCSTR(132usize as *const u8)) else { return false };
        let f: unsafe extern "system" fn() -> bool = std::mem::transmute(f);
        f()
    }
}

/// Zone cliente de `size` (logique) au centre de la zone de travail de l'écran principal, et ses
/// points par pouce.
fn centered(size: (f32, f32)) -> (RECT, Option<u32>) {
    // SAFETY : appels de lecture ; sorties locales.
    unsafe {
        let mon = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let (w, h) = ((size.0 * s).round() as i32, (size.1 * s).round() as i32);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - w) / 2;
        let y = work.top + ((work.bottom - work.top) - h) / 2;
        (RECT { left: x, top: y, right: x + w, bottom: y + h }, Some(dx))
    }
}

/// Curseur Windows pour la forme demandée par Slint.
fn cursor_for(c: &MouseCursorInner) -> HCURSOR {
    let id = match c {
        MouseCursorInner::BuiltIn(b) => match b {
            BuiltInMouseCursor::None => return HCURSOR::default(),
            BuiltInMouseCursor::Pointer => IDC_HAND,
            BuiltInMouseCursor::Text => IDC_IBEAM,
            BuiltInMouseCursor::Wait => IDC_WAIT,
            BuiltInMouseCursor::Progress => IDC_APPSTARTING,
            BuiltInMouseCursor::Crosshair => IDC_CROSS,
            BuiltInMouseCursor::Help => IDC_HELP,
            BuiltInMouseCursor::Move | BuiltInMouseCursor::Grab | BuiltInMouseCursor::Grabbing => IDC_SIZEALL,
            BuiltInMouseCursor::NoDrop | BuiltInMouseCursor::NotAllowed => IDC_NO,
            BuiltInMouseCursor::ColResize
            | BuiltInMouseCursor::EResize
            | BuiltInMouseCursor::WResize
            | BuiltInMouseCursor::EwResize => IDC_SIZEWE,
            BuiltInMouseCursor::RowResize
            | BuiltInMouseCursor::NResize
            | BuiltInMouseCursor::SResize
            | BuiltInMouseCursor::NsResize => IDC_SIZENS,
            BuiltInMouseCursor::NeResize | BuiltInMouseCursor::SwResize | BuiltInMouseCursor::NeswResize => IDC_SIZENESW,
            BuiltInMouseCursor::NwResize | BuiltInMouseCursor::SeResize | BuiltInMouseCursor::NwseResize => IDC_SIZENWSE,
            _ => IDC_ARROW,
        },
        // Curseurs dessinés : pas encore (aucun dans Horizon).
        _ => IDC_ARROW,
    };
    // SAFETY : curseur système partagé (jamais détruit).
    unsafe { LoadCursorW(None, id) }.unwrap_or_default()
}

impl WindowAdapter for NativeWindow {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn set_visible(&self, visible: bool) -> Result<(), PlatformError> {
        if visible {
            if self.hwnd.get().is_invalid() {
                self.create().map_err(|e| PlatformError::Other(format!("fenêtre native : {e:#}")))?;
            }
            if !self.visible.replace(true) {
                self.show_pending.set(true);
            }
        } else if self.visible.replace(false) {
            self.show_pending.set(false);
            // SAFETY : fenêtre de ce fil.
            let _ = unsafe { ShowWindow(self.hwnd.get(), SW_HIDE) };
            // La souris n'y est plus : sans cela, un survol resterait « en cours » dans Slint (et
            // le suivi de la sortie armé) jusqu'au prochain affichage.
            if self.tracking.replace(false) {
                self.dispatch(WindowEvent::PointerExited);
            }
        }
        Ok(())
    }

    fn position(&self) -> Option<PhysicalPosition> {
        let hwnd = self.hwnd.get();
        if hwnd.is_invalid() {
            return None;
        }
        let mut r = RECT::default();
        // SAFETY : sortie valide.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
        Some(PhysicalPosition::new(r.left, r.top))
    }

    fn set_position(&self, position: WindowPosition) {
        let hwnd = self.hwnd.get();
        if hwnd.is_invalid() {
            return;
        }
        let p = position.to_physical(self.scale.get());
        // SAFETY : fenêtre de ce fil.
        let _ = unsafe { SetWindowPos(hwnd, None, p.x, p.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    fn set_size(&self, size: WindowSize) {
        let p = size.to_physical(self.scale.get());
        if self.hwnd.get().is_invalid() {
            self.size.set(p);
            self.window.dispatch_event(WindowEvent::Resized { size: size.to_logical(self.scale.get()) });
            return;
        }
        if p != self.size.get() {
            self.resize_client(p.width, p.height);
        }
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn request_redraw(&self) {
        self.dirty.set(true);
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn update_window_properties(&self, properties: WindowProperties<'_>) {
        self.min_size.set(properties.layout_constraints().min);
        let title = properties.title();
        if self.title.borrow().as_str() != title.as_str() {
            *self.title.borrow_mut() = title.to_string();
            let hwnd = self.hwnd.get();
            if !hwnd.is_invalid() {
                // SAFETY : fenêtre de ce fil.
                let _ = unsafe { SetWindowTextW(hwnd, &HSTRING::from(title.as_str())) };
            }
        }
    }

    fn internal(&self, _: i_slint_core::InternalToken) -> Option<&dyn WindowAdapterInternal> {
        Some(self)
    }
}

impl WindowAdapterInternal for NativeWindow {
    fn set_mouse_cursor(&self, cursor: MouseCursorInner) {
        let c = cursor_for(&cursor);
        if self.cursor.replace(c) != c && self.tracking.get() {
            // SAFETY : curseur système (ou aucun).
            unsafe { SetCursor(Some(c)) };
        }
    }

    fn bring_to_front(&self) -> Result<(), PlatformError> {
        // SAFETY : fenêtre de ce fil.
        let _ = unsafe { SetForegroundWindow(self.hwnd.get()) };
        Ok(())
    }

    /// Champ de texte actif : la fenêtre de composition de l'IME (et sa liste de candidats) se
    /// place au curseur de texte. Le texte composé arrive ensuite par `WM_CHAR`.
    fn input_method_request(&self, request: i_slint_core::window::InputMethodRequest) {
        use i_slint_core::window::InputMethodRequest;
        let (InputMethodRequest::Enable(p) | InputMethodRequest::Update(p)) = request else { return };
        let hwnd = self.hwnd.get();
        if hwnd.is_invalid() {
            return;
        }
        let scale = self.window.scale_factor();
        let x = (p.cursor_rect_origin.x * scale).round() as i32;
        let bottom = ((p.cursor_rect_origin.y + p.cursor_rect_size.height) * scale).round() as i32;
        super::ime::place(hwnd, x, (p.cursor_rect_origin.y * scale).round() as i32, bottom);
    }
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        let hwnd = self.hwnd.get();
        if !hwnd.is_invalid() {
            // SAFETY : fenêtre de ce fil ; plus aucune référence vers elle dans la plateforme.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // La fenêtre est copiée hors du registre : un événement peut en créer ou en fermer d'autres.
    if let Some(w) = find(hwnd)
        && let Some(r) = w.handle(msg, wparam, lparam)
    {
        return r;
    }
    // SAFETY : traitement par défaut.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
