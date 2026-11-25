//! Menus contextuels Windows des barres (`edit::popup` et les menus de `app/src-tauri/src/edit.rs`) :
//! entrées, cases à cocher, sous-menus, icônes (menu d'ajout), séparateurs. La barre, qui ne
//! prend jamais le premier plan, devient activable le temps du menu (sans premier plan, le menu
//! ne se fermerait pas d'un clic ailleurs), et le reste en mode édition (`keep_active`).
//!
//! Menus sombres quand Windows est en thème sombre, comme ceux de la version Tauri (muda/tao :
//! `SetPreferredAppMode(AllowDark)` et `AllowDarkModeForWindow` d'uxtheme) ; icônes claires sur
//! menu sombre, foncées sinon (`lib/menuIcon.ts`).
//!
//! Le menu Windows a sa propre boucle de messages : il est ouvert hors du traitement de
//! l'événement Slint en cours (`Timer` à zéro), puis le choix est passé à `chosen`.

use std::cell::Cell;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MENU_ITEM_FLAGS, MENUITEMINFOW, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MIIM_BITMAP, PostMessageW, SetMenuItemInfoW, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};
use windows::core::{HSTRING, PCSTR, PCWSTR, w};

/// Entrée d'un menu simple (compatibilité) : identifiant (non nul) et texte ; `None` = séparateur.
pub type Item = Option<(usize, String)>;

/// Entrée d'un menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// Entrée qui agit : identifiant rendu à `chosen`, texte, case cochée (`Some`) ou non
    /// (`None` : entrée simple), grisée, icône Phosphor (`ui::icons`).
    Item { id: String, text: String, checked: Option<bool>, enabled: bool, icon: Option<&'static str> },
    Separator,
    Sub { text: String, enabled: bool, items: Vec<Entry> },
}

impl Entry {
    pub fn item(id: impl Into<String>, text: impl Into<String>) -> Entry {
        Entry::Item { id: id.into(), text: text.into(), checked: None, enabled: true, icon: None }
    }

    pub fn check(id: impl Into<String>, text: impl Into<String>, checked: bool) -> Entry {
        Entry::Item { id: id.into(), text: text.into(), checked: Some(checked), enabled: true, icon: None }
    }

    pub fn sub(text: impl Into<String>, items: Vec<Entry>) -> Entry {
        Entry::Sub { text: text.into(), enabled: true, items }
    }

    /// Avec une icône (entrée) ; sans effet sur un séparateur ou un sous-menu.
    pub fn with_icon(mut self, name: &'static str) -> Entry {
        if let Entry::Item { icon, .. } = &mut self {
            *icon = Some(name);
        }
        self
    }

    /// Grisée (ou non).
    pub fn enabled(mut self, on: bool) -> Entry {
        match &mut self {
            Entry::Item { enabled, .. } | Entry::Sub { enabled, .. } => *enabled = on,
            Entry::Separator => {}
        }
        self
    }

    /// Texte de l'entrée (ou du sous-menu).
    pub fn text(&self) -> &str {
        match self {
            Entry::Item { text, .. } | Entry::Sub { text, .. } => text,
            Entry::Separator => "",
        }
    }

    /// La même entrée sous un autre identifiant (sans effet sur un séparateur ou un sous-menu).
    pub fn with_id(mut self, new: String) -> Entry {
        if let Entry::Item { id, .. } = &mut self {
            *id = new;
        }
        self
    }

    /// Identifiant d'une entrée (vide pour un séparateur ou un sous-menu).
    pub fn id(&self) -> &str {
        match self {
            Entry::Item { id, .. } => id,
            _ => "",
        }
    }
}

thread_local! {
    /// Un menu a été demandé pendant le traitement de l'événement en cours (clic droit d'un
    /// widget qui a son propre menu : le menu de la barre n'est pas ouvert en plus).
    static REQUESTED: Cell<bool> = const { Cell::new(false) };
    /// Un menu est ouvert, ou s'est fermé à cet instant.
    static OPEN: Cell<Option<Instant>> = const { Cell::new(None) };
    static DARK_READY: Cell<bool> = const { Cell::new(false) };
}

/// Un menu ouvert prend le focus : la fenêtre en mode édition ne doit pas en conclure qu'on a
/// cliqué ailleurs. Vrai pendant le menu et 300 ms après (l'action arrive juste après sa
/// fermeture).
pub fn is_open() -> bool {
    OPEN.get().is_some_and(|t| t.elapsed() < Duration::from_millis(300))
}

/// Un menu a-t-il été demandé pendant le traitement de l'événement en cours ? (Remis à faux
/// quand il s'ouvre, après ce traitement.)
pub fn requested() -> bool {
    REQUESTED.get()
}

/// Ouvre le menu simple `items` sous la souris, pour la barre `hwnd` ; `chosen` reçoit
/// l'identifiant de l'entrée choisie (rien si le menu est fermé sans choix).
pub fn popup(hwnd: isize, items: Vec<Item>, chosen: impl FnOnce(usize) + 'static) {
    let entries = items
        .into_iter()
        .map(|i| match i {
            Some((id, text)) => Entry::item(id.to_string(), text),
            None => Entry::Separator,
        })
        .collect();
    show(hwnd, entries, false, move |id| {
        if let Ok(n) = id.parse() {
            chosen(n);
        }
    });
}

/// Ouvre le menu `entries` sous la souris, pour la fenêtre `hwnd` ; `chosen` reçoit
/// l'identifiant de l'entrée choisie. `keep_active` : la fenêtre reste activable ensuite (mode
/// édition), sinon elle redevient inactivable.
pub fn show(hwnd: isize, entries: Vec<Entry>, keep_active: bool, chosen: impl FnOnce(String) + 'static) {
    show_at(hwnd, entries, keep_active, None, chosen);
}

/// Comme [`show`], le coin haut gauche du menu au point d'écran `at` (pixels physiques) au lieu
/// de la souris : liste déroulante d'une fenêtre de réglages, sous son champ.
pub fn show_at(hwnd: isize, entries: Vec<Entry>, keep_active: bool, at: Option<(i32, i32)>, chosen: impl FnOnce(String) + 'static) {
    open(hwnd, entries, keep_active, at, move |picked| {
        if let Some(id) = picked {
            chosen(id);
        }
    });
}

/// Comme [`show`], mais `closed` est appelé dans tous les cas à la fermeture du menu : avec
/// l'identifiant de l'entrée choisie, ou `None` si le menu est fermé sans choix (panneau qui
/// reste affiché le temps du menu).
pub fn show_then(hwnd: isize, entries: Vec<Entry>, keep_active: bool, closed: impl FnOnce(Option<String>) + 'static) {
    open(hwnd, entries, keep_active, None, closed);
}

fn open(hwnd: isize, entries: Vec<Entry>, keep_active: bool, at: Option<(i32, i32)>, closed: impl FnOnce(Option<String>) + 'static) {
    crate::ui::tooltip::hide();
    REQUESTED.set(true);
    OPEN.set(Some(Instant::now() + Duration::from_secs(3600)));
    slint::Timer::single_shot(Duration::ZERO, move || {
        REQUESTED.set(false);
        let picked = track(hwnd, &entries, keep_active, at);
        OPEN.set(Some(Instant::now()));
        closed(picked);
    });
}

/// Identifiants des entrées, dans l'ordre (le n-ième reçoit le numéro n + 1).
fn flatten<'a>(entries: &'a [Entry], out: &mut Vec<&'a str>) {
    for e in entries {
        match e {
            Entry::Item { id, .. } => out.push(id),
            Entry::Sub { items, .. } => flatten(items, out),
            Entry::Separator => {}
        }
    }
}

/// Construit le menu Windows (sous-menus compris) ; `next` : numéro de la prochaine entrée.
/// Les images des icônes sont rendues dans `bitmaps` (à détruire après le menu).
///
/// # Safety
/// `m` est un menu valide créé par ce fil.
unsafe fn build(m: HMENU, entries: &[Entry], next: &mut usize, icon: &dyn Fn(&str) -> Option<HBITMAP>, bitmaps: &mut Vec<HBITMAP>) {
    for e in entries {
        // SAFETY : menus créés ici, chaînes copiées par Windows.
        unsafe {
            match e {
                Entry::Item { text, checked, enabled, icon: name, .. } => {
                    let mut flags = MF_STRING;
                    if *checked == Some(true) {
                        flags |= MF_CHECKED;
                    }
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    let id = *next;
                    *next += 1;
                    let _ = AppendMenuW(m, flags, id, &HSTRING::from(text.as_str()));
                    if let Some(bmp) = name.and_then(icon) {
                        bitmaps.push(bmp);
                        let info = MENUITEMINFOW {
                            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_BITMAP,
                            hbmpItem: bmp,
                            ..Default::default()
                        };
                        let _ = SetMenuItemInfoW(m, id as u32, false, &info);
                    }
                }
                Entry::Separator => {
                    let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
                }
                Entry::Sub { text, enabled, items } => {
                    let Ok(sub) = CreatePopupMenu() else { continue };
                    build(sub, items, next, icon, bitmaps);
                    let mut flags: MENU_ITEM_FLAGS = MF_POPUP | MF_STRING;
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(m, flags, sub.0 as usize, &HSTRING::from(text.as_str()));
                }
            }
        }
    }
}

fn track(hwnd: isize, entries: &[Entry], keep_active: bool, at: Option<(i32, i32)>) -> Option<String> {
    let dark = dark_menus(hwnd);
    horizon_sys::appbar::set_activatable(hwnd, true);
    horizon_sys::window_focus::bring_to_front(hwnd);
    let h = HWND(hwnd as *mut _);
    // SAFETY : DPI de notre fenêtre.
    let size = ((16 * unsafe { GetDpiForWindow(h) }.max(96)) as f32 / 96.0).round() as u32;
    let color = if dark { [0xe6, 0xe9, 0xf5] } else { [0x2b, 0x2b, 0x3a] };
    let icon = |name: &str| icon_bitmap(name, size, color);
    let mut ids = Vec::new();
    flatten(entries, &mut ids);
    let mut bitmaps = Vec::new();
    // SAFETY : menu créé, montré puis détruit ici (sous-menus avec lui) ; fenêtre de ce fil.
    let cmd = unsafe {
        let m = CreatePopupMenu().ok()?;
        build(m, entries, &mut 1, &icon, &mut bitmaps);
        let mut p = POINT::default();
        match at {
            Some((x, y)) => p = POINT { x, y },
            None => {
                let _ = GetCursorPos(&mut p);
            }
        }
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, p.x, p.y, None, h, None);
        let _ = PostMessageW(Some(h), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(m);
        for b in bitmaps {
            let _ = DeleteObject(HGDIOBJ(b.0));
        }
        cmd.0 as usize
    };
    if !keep_active {
        horizon_sys::appbar::set_activatable(hwnd, false);
    }
    (cmd != 0).then(|| ids.get(cmd - 1).map(|s| (*s).to_owned())).flatten()
}

/// Fonction d'uxtheme.dll par son numéro d'ordre (non documentée, comme muda et tao).
fn uxtheme(ordinal: u16) -> Option<unsafe extern "system" fn() -> isize> {
    // SAFETY : bibliothèque système ; adresse lue par son numéro d'ordre.
    unsafe {
        let lib = LoadLibraryW(w!("uxtheme.dll")).ok()?;
        GetProcAddress(lib, PCSTR(ordinal as usize as *const u8))
    }
}

/// Menus sombres si Windows est en thème sombre (pour l'appli, puis pour la fenêtre `hwnd`).
/// Renvoie vrai si les menus sont sombres.
fn dark_menus(hwnd: isize) -> bool {
    // SAFETY : signatures d'uxtheme (ShouldAppsUseDarkMode 132, AllowDarkModeForWindow 133,
    // SetPreferredAppMode 135, FlushMenuThemes 136) depuis Windows 10 1903.
    unsafe {
        let dark = uxtheme(132).is_some_and(|f| {
            let f: unsafe extern "system" fn() -> bool = std::mem::transmute(f);
            f()
        });
        if !DARK_READY.replace(true)
            && let Some(f) = uxtheme(135)
        {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            // 1 = AllowDark : les menus suivent le thème de Windows.
            set(1);
            if let Some(f) = uxtheme(136) {
                let flush: unsafe extern "system" fn() = std::mem::transmute(f);
                flush();
            }
        }
        if let Some(f) = uxtheme(133) {
            let allow: unsafe extern "system" fn(HWND, bool) -> bool = std::mem::transmute(f);
            allow(HWND(hwnd as *mut _), true);
        }
        dark
    }
}

/// Pixels BGRA prémultipliés de l'icône `name` (carré de `size`), teinte `color` : l'icône sert
/// de masque, comme `menuIcon` de la version Tauri.
pub fn icon_pixels(name: &str, size: u32, color: [u8; 3]) -> Option<Vec<u8>> {
    use i_slint_core::graphics::{ImageInner, SharedImageBuffer, euclid};
    let img = crate::ui::icons::image(name);
    let inner: &ImageInner = (&img).into();
    let alpha: Vec<u8> = match inner.render_to_buffer(Some(euclid::size2(size, size)))? {
        SharedImageBuffer::RGBA8(b) | SharedImageBuffer::RGBA8Premultiplied(b) => b.as_slice().iter().map(|p| p.a).collect(),
        SharedImageBuffer::RGB8(_) => return None,
    };
    if alpha.len() != (size * size) as usize {
        return None;
    }
    let mut out = Vec::with_capacity(alpha.len() * 4);
    for a in alpha {
        let pre = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
        out.extend_from_slice(&[pre(color[2]), pre(color[1]), pre(color[0]), a]);
    }
    Some(out)
}

/// Image 32 bits (alpha prémultiplié) d'une icône, pour une entrée de menu.
fn icon_bitmap(name: &str, size: u32, color: [u8; 3]) -> Option<HBITMAP> {
    let pixels = icon_pixels(name, size, color)?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            // Négatif : lignes de haut en bas.
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY : section DIB de la taille des pixels, copiés juste après.
    unsafe {
        let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            return None;
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        Some(bmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_entries_in_order() {
        let menu = vec![
            Entry::item("a", "A"),
            Entry::Separator,
            Entry::sub("S", vec![Entry::check("b", "B", true), Entry::item("c", "C").enabled(false)]),
            Entry::item("d", "D").with_icon("clock"),
        ];
        let mut ids = Vec::new();
        flatten(&menu, &mut ids);
        assert_eq!(ids, ["a", "b", "c", "d"]);
        assert_eq!(menu[2].text(), "S");
        assert_eq!(menu[3].id(), "d");
    }

    #[test]
    fn icon_is_a_tinted_mask() {
        // Assez grand pour que le trait ait des pixels pleins.
        let px = icon_pixels("plus", 48, [0xe6, 0xe9, 0xf5]).expect("icône rendue");
        assert_eq!(px.len(), 48 * 48 * 4);
        // Des pixels pleins (le trait) et des pixels vides (le fond).
        assert!(px.chunks(4).any(|p| p[3] == 255 && p[0] == 0xf5 && p[2] == 0xe6));
        assert!(px.chunks(4).any(|p| p == [0, 0, 0, 0]));
        assert!(icon_pixels("absente-zz", 16, [0, 0, 0]).is_none());
    }
}
