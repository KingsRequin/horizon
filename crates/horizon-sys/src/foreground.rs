//! Fenêtre au premier plan (appli + écran), relance et mise au premier plan des applis.

use crate::win32::{class_name, from_wide};
use horizon_core::apps::{app_key, is_shell_window, launcher_app_key};
use std::cell::RefCell;
use std::path::Path;
use crate::window_focus::{bring_to_front, main_windows, pid_of};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromWindow};
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, GetForegroundWindow,
    GetMessageW, MSG, TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
};
use windows::core::{HSTRING, Interface, PWSTR};

/// Fenêtre au premier plan : appli (AUMID d'une appli empaquetée, sinon chemin de l'exe) et
/// écran (nom GDI `\\.\DISPLAYn`).
#[derive(Debug, Clone, PartialEq)]
pub struct Foreground {
    pub hwnd: isize,
    pub app: String,
    pub monitor_device: String,
}

pub(crate) fn exe_of_pid(pid: u32) -> Option<String> {
    // SAFETY : handle fermé avant de sortir ; tampon de la taille annoncée.
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(p);
        r.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// AUMID déclaré par la fenêtre (propriété du shell), s'il y en a un.
fn window_aumid(h: HWND) -> Option<String> {
    // SAFETY : COM initialisé sur le fil appelant ; chaîne libérée par CoTaskMemFree.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
        let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let s = PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|s| !s.is_empty())
    }
}

/// AUMID d'un processus empaqueté (MSIX : Bloc-notes, Terminal…), `None` pour une appli classique.
pub(crate) fn process_aumid(pid: u32) -> Option<String> {
    use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
    // SAFETY : handle fermé avant de sortir ; tampon de la taille annoncée.
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let r = GetApplicationUserModelId(p, &mut len, Some(PWSTR(buf.as_mut_ptr())));
        let _ = CloseHandle(p);
        if r.is_err() || len == 0 {
            return None;
        }
        Some(from_wide(&buf[..(len as usize).min(buf.len())])).filter(|s| s.contains('!'))
    }
}

/// Identité d'appli d'une fenêtre. L'AUMID ne sert que pour les applis empaquetées (celui du
/// processus, ou celui de la fenêtre hôte des applis UWP) : les applis classiques comme Vivaldi
/// ou Discord déclarent un AUMID maison qui ne permet pas de les relancer.
pub(crate) fn app_of(h: HWND, pid: u32, exe: &str) -> String {
    if let Some(id) = process_aumid(pid) {
        return id;
    }
    let host = exe.to_ascii_lowercase().ends_with(r"\applicationframehost.exe");
    match window_aumid(h) {
        Some(id) if id.contains('!') || host => id,
        _ => exe.to_owned(),
    }
}

fn monitor_device(h: HWND) -> Option<String> {
    // SAFETY : structure étendue de la taille annoncée dans cbSize.
    unsafe {
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(m, &mut info.monitorInfo).as_bool() {
            return None;
        }
        Some(from_wide(&info.szDevice))
    }
}

/// Fenêtre au premier plan, ou `None` si c'est une surface du shell ou une de nos fenêtres.
fn describe(h: HWND, own_pid: u32) -> Option<Foreground> {
    if h.0.is_null() {
        return None;
    }
    let pid = pid_of(h);
    if pid == own_pid {
        return None;
    }
    let exe = exe_of_pid(pid)?;
    let exe_name = Path::new(&exe).file_name()?.to_string_lossy().to_string();
    if is_shell_window(&class_name(h), &exe_name) {
        return None;
    }
    Some(Foreground { hwnd: h.0 as isize, app: app_of(h, pid, &exe), monitor_device: monitor_device(h)? })
}

struct Hook {
    tx: crossbeam_channel::Sender<Option<Foreground>>,
    own_pid: u32,
}

thread_local! {
    static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_event(
    _: HWINEVENTHOOK,
    event: u32,
    h: HWND,
    id_object: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    // OBJID_WINDOW = 0 : seuls les événements de la fenêtre elle-même.
    if id_object != 0 {
        return;
    }
    // SAFETY : appel sans argument.
    if event == EVENT_SYSTEM_MOVESIZEEND && unsafe { GetForegroundWindow() } != h {
        return;
    }
    HOOK.with_borrow(|hook| {
        if let Some(hook) = hook {
            let _ = hook.tx.send(describe(h, hook.own_pid));
        }
    });
}

/// Surveille le premier plan sur un fil dédié (événements Windows, pas de scrutation). Envoie
/// l'état courant au démarrage, puis à chaque changement de fenêtre ou fin de déplacement.
pub fn watch(tx: crossbeam_channel::Sender<Option<Foreground>>, own_pid: u32) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("foreground".into())
        .spawn(move || {
            // COM en STA pour ce fil (propriétés de fenêtre), pour toute sa vie.
            let _com = crate::com::ComGuard::sta().inspect_err(|e| log::warn!("premier plan : COM : {e}"));
            // SAFETY : appel sans argument.
            let _ = tx.send(describe(unsafe { GetForegroundWindow() }, own_pid));
            HOOK.with_borrow_mut(|h| *h = Some(Hook { tx, own_pid }));
            let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
            // SAFETY : rappel `extern "system"` valide pour toute la vie du fil ; les crochets
            // hors contexte sont servis par la boucle de messages de ce fil.
            unsafe {
                let a = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_event), 0, 0, flags);
                let b = SetWinEventHook(EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND, None, Some(on_event), 0, 0, flags);
                if a.is_invalid() || b.is_invalid() {
                    log::error!("crochet du premier plan impossible");
                    return;
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("fil du premier plan : {e}"))
}

/// Raccourci `.lnk` chargé. À appeler sur le fil shell (`crate::shell`).
fn load_lnk(lnk: &str) -> Option<IShellLinkW> {
    // SAFETY : objets COM du shell.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk), STGM_READ).ok()?;
        Some(link)
    }
}

/// AUMID déclaré par un raccourci `.lnk` (celui que Windows associe à l'appli lancée).
pub(crate) fn lnk_aumid(lnk: &str) -> Option<String> {
    let link = load_lnk(lnk)?;
    // SAFETY : magasin de propriétés du raccourci chargé.
    let v = unsafe { link.cast::<IPropertyStore>().ok()?.GetValue(&PKEY_AppUserModel_ID).ok()? };
    Some(v.to_string()).filter(|s| !s.is_empty())
}

/// Cible et arguments d'un raccourci `.lnk`. À appeler sur le fil shell (`crate::shell`).
pub(crate) fn lnk_info(lnk: &str) -> Option<(String, String)> {
    let link = load_lnk(lnk)?;
    // SAFETY : raccourci chargé ; tampons bornés.
    unsafe {
        let mut path = [0u16; 1024];
        link.GetPath(&mut path, std::ptr::null_mut(), 0).ok()?;
        let mut args = [0u16; 1024];
        let _ = link.GetArguments(&mut args);
        let s = from_wide;
        let target = s(&path);
        (!target.is_empty()).then(|| (target, s(&args)))
    }
}

/// Clé d'appli d'une cible de raccourci fixe (`.lnk`, `shell:AppsFolder\…`, exe), ou `None`
/// si ce n'est pas une appli (URL, dossier…). Voir `horizon_core::shortcuts::target_app_key`.
pub fn target_app_key(target: &str) -> Option<String> {
    let target = target.to_owned();
    crate::shell::run(move || horizon_core::shortcuts::target_app_key(&target, lnk_info)).flatten()
}

/// Comment relancer une appli connue par son AUMID ou son exe (conception §5.4) : AUMID →
/// `shell:AppsFolder\…` ; raccourci du menu Démarrer qui lance la même appli ; sinon l'exe s'il
/// existe encore. `None` = impossible de la relancer proprement.
pub fn resolve_launch(app: &str) -> Option<String> {
    if app.contains('!') {
        return Some(format!(r"shell:AppsFolder\{app}"));
    }
    let key = app_key(app);
    let hit = crate::shell::run(move || {
        crate::startmenu::shortcuts().into_iter().find(|l| {
            lnk_info(&l.to_string_lossy()).is_some_and(|(t, a)| launcher_app_key(&t, &a) == key)
        })
    })
    .flatten();
    if let Some(l) = hit {
        return Some(l.display().to_string());
    }
    Path::new(app).exists().then(|| app.to_owned())
}

/// Clé d'appli (`app_key`) d'une fenêtre principale, `None` pour une surface du shell
/// (bureau, barre des tâches…) ou un processus illisible. Lue sur le fil shell.
pub fn window_app_key(hwnd: isize, pid: u32) -> Option<String> {
    crate::shell::run(move || window_app_key_here(hwnd, pid)).flatten()
}

fn window_app_key_here(hwnd: isize, pid: u32) -> Option<String> {
    let h = HWND(hwnd as *mut _);
    let exe = exe_of_pid(pid)?;
    let exe_name = Path::new(&exe).file_name()?.to_string_lossy().to_string();
    if is_shell_window(&class_name(h), &exe_name) {
        return None;
    }
    Some(app_key(&app_of(h, pid, &exe)))
}

/// Restaure et met au premier plan la fenêtre principale la plus haute de l'appli de clé
/// `key` (sur le fil shell). `false` si l'appli n'a pas de fenêtre ouverte.
pub fn focus_app(key: &str) -> bool {
    let key = key.to_owned();
    crate::shell::run(move || {
        let found = main_windows().into_iter().find(|&(h, pid)| window_app_key_here(h, pid).as_deref() == Some(&key));
        found.is_some_and(|(h, _)| bring_to_front(h))
    })
    .unwrap_or(false)
}

/// Fenêtre principale visible d'une appli qui tourne (AUMID ou chemin d'exe) → restaurée et
/// mise au premier plan. `false` si l'appli n'a pas de fenêtre ouverte.
pub fn focus_running(app: &str) -> bool {
    focus_app(&app_key(app))
}
