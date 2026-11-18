//! Fenêtres d'application ouvertes : relevé (widget « Fenêtres », palette), surveillance par
//! événements (compteur, sans scrutation) et déplacement d'une fenêtre sur un autre écran.
//!
//! Les décisions (quelles fenêtres, quel écran, quelle place) sont dans
//! `horizon_core::app_windows` ; ici, seulement ce que Windows en dit et ce qu'on lui demande.
//! Tout est en pixels physiques : le fil qui déplace se déclare conscient de l'échelle de
//! chaque écran (Horizon l'est déjà par son manifeste ; un exemple ou un test ne l'est pas).

use crate::win32::{class_name, frame_bounds, from_wide, is_cloaked, window_rect};
use crate::window_focus::{bring_to_front, pid_of};
use horizon_core::app_windows::{Insets, WindowFacts, WindowView, is_app_window, monitor_of, target_frame};
use horizon_core::apps::{display_name, is_shell_window};
use horizon_core::geometry::{MonitorInfo, Rect};
use std::cell::RefCell;
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_CLOAKED, EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE, EVENT_OBJECT_UNCLOAKED, EVENT_SYSTEM_MINIMIZEEND, EVENT_SYSTEM_MINIMIZESTART, EnumWindows, GA_PARENT,
    GW_OWNER, GWL_EXSTYLE, GetAncestor, GetDesktopWindow, GetMessageW, GetWindow, GetWindowLongPtrW,
    GetWindowPlacement, GetWindowTextLengthW, GetWindowTextW, IsHungAppWindow, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, MSG, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER,
    SetWindowPos, ShowWindow, TranslateMessage, WINDOWPLACEMENT, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

fn hwnd(h: isize) -> HWND {
    HWND(h as *mut _)
}

/// Toutes les fenêtres de premier niveau, la plus haute d'abord.
fn top_level() -> Vec<HWND> {
    unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
        // SAFETY : `data` pointe vers le Vec de l'appelant, vivant pendant l'énumération.
        unsafe { &mut *(data.0 as *mut Vec<HWND>) }.push(h);
        true.into()
    }
    let mut out: Vec<HWND> = Vec::new();
    // SAFETY : énumération synchrone, le pointeur reste valide pendant l'appel.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize)) };
    out
}

fn title_of(h: HWND) -> String {
    // SAFETY : tampon de la taille annoncée. Le titre d'une fenêtre d'un autre processus est lu
    // sans lui envoyer de message : une appli bloquée ne bloque pas le relevé.
    unsafe {
        let n = GetWindowTextLengthW(h);
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, &mut buf);
        from_wide(&buf[..got.max(0) as usize])
    }
}

/// Une fenêtre d'application et son processus, ou `None`.
struct Candidate {
    h: HWND,
    pid: u32,
    title: String,
    exe: String,
}

/// Fenêtre d'application ? Les tests bon marché d'abord ; le processus n'est ouvert que pour
/// celles qui restent.
fn candidate(h: HWND, own_pid: u32) -> Option<Candidate> {
    // SAFETY : lectures d'attributs d'une fenêtre (une fenêtre disparue renvoie des zéros).
    let (visible, ex, owned) = unsafe {
        (
            IsWindowVisible(h).as_bool(),
            GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
            GetWindow(h, GW_OWNER).is_ok_and(|o| !o.0.is_null()),
        )
    };
    let pid = pid_of(h);
    let mut f = WindowFacts {
        visible,
        tool: ex & WS_EX_TOOLWINDOW.0 != 0,
        app_window: ex & WS_EX_APPWINDOW.0 != 0,
        no_activate: ex & WS_EX_NOACTIVATE.0 != 0,
        owned,
        own_process: pid == own_pid,
        // Titre provisoire : les tests suivants (DWM, titre, processus) coûtent plus.
        title: "?".into(),
        ..Default::default()
    };
    if !is_app_window(&f) {
        return None;
    }
    f.cloaked = is_cloaked(h);
    f.title = title_of(h);
    if !is_app_window(&f) {
        return None;
    }
    let exe = crate::foreground::exe_of_pid(pid)?;
    let exe_name = exe.rsplit(['\\', '/']).next().unwrap_or(&exe);
    f.shell = is_shell_window(&class_name(h), exe_name);
    is_app_window(&f).then_some(Candidate { h, pid, title: f.title, exe })
}

/// Poignées des fenêtres d'application ouvertes (compteur ; aucun appel COM).
pub fn handles(own_pid: u32) -> Vec<isize> {
    top_level().into_iter().filter_map(|h| candidate(h, own_pid)).map(|c| c.h.0 as isize).collect()
}

/// Où se trouve une fenêtre : son cadre visible, ou pour une fenêtre réduite sa place une fois
/// restaurée (en coordonnées de l'espace de travail : décalées de la zone de travail de
/// l'écran principal).
fn place_of(h: HWND, monitors: &[MonitorInfo]) -> Option<Rect> {
    // SAFETY : lecture d'état.
    if !unsafe { IsIconic(h) }.as_bool() {
        return frame_bounds(h);
    }
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    // SAFETY : structure de la taille annoncée.
    unsafe { GetWindowPlacement(h, &mut wp) }.ok()?;
    let (dx, dy) = monitors
        .iter()
        .find(|m| m.primary)
        .map_or((0, 0), |p| (p.work.x - p.rect.x, p.work.y - p.rect.y));
    let r = crate::win32::to_rect(wp.rcNormalPosition);
    Some(Rect { x: r.x + dx, y: r.y + dy, ..r })
}

/// Fenêtres d'application ouvertes, la plus haute d'abord, avec leur appli et leur écran.
/// Lu sur le fil shell (identité des applis empaquetées par COM).
pub fn list(own_pid: u32, monitors: &[MonitorInfo]) -> Vec<WindowView> {
    let monitors = monitors.to_vec();
    crate::shell::run(move || {
        let _dpi = DpiAware::enter();
        top_level()
            .into_iter()
            .filter_map(|h| candidate(h, own_pid))
            .map(|c| {
                let app = crate::foreground::app_of(c.h, c.pid, &c.exe);
                let icon = if app.contains('!') { format!(r"shell:AppsFolder\{app}") } else { c.exe.clone() };
                let monitor = place_of(c.h, &monitors)
                    .and_then(|r| monitor_of(r, &monitors))
                    .map(|m| m.id.clone())
                    .unwrap_or_default();
                WindowView {
                    hwnd: c.h.0 as i64,
                    title: c.title,
                    app: display_name(&app, None),
                    icon,
                    // SAFETY : lecture d'état.
                    minimized: unsafe { IsIconic(c.h) }.as_bool(),
                    monitor,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Cadre visible d'une fenêtre en pixels physiques, agrandie et réduite (diagnostic, exemple).
pub fn state(h: isize) -> Option<(Rect, bool, bool)> {
    let _dpi = DpiAware::enter();
    let w = hwnd(h);
    // SAFETY : lectures d'état.
    let (max, min) = unsafe { (IsZoomed(w).as_bool(), IsIconic(w).as_bool()) };
    Some((frame_bounds(w)?, max, min))
}


/// Le fil courant compte en pixels physiques pendant la vie de la garde.
struct DpiAware(DPI_AWARENESS_CONTEXT);

impl DpiAware {
    fn enter() -> DpiAware {
        // SAFETY : réglage propre au fil, rendu par Drop.
        DpiAware(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for DpiAware {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY : contexte rendu par SetThreadDpiAwarenessContext.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}

fn set_rect(h: HWND, r: Rect) -> Result<(), String> {
    // SAFETY : fenêtre vérifiée par l'appelant ; sans effet si elle disparaît entre-temps.
    unsafe { SetWindowPos(h, None, r.x, r.y, r.w, r.h, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER) }
        .map_err(|e| format!("déplacement refusé ({e})"))
}

/// Ramène la fenêtre `h` sur l'écran `to` puis la met au premier plan : réduite, elle est
/// d'abord restaurée ; agrandie, elle est restaurée, déplacée puis agrandie de nouveau ; sa
/// taille et sa place dans la zone de travail sont gardées en pixels logiques (réduite si
/// l'écran d'arrivée est trop petit). Déjà sur cet écran : seulement le premier plan.
///
/// Bloquant (messages envoyés à la fenêtre) : à appeler sur un fil à part.
pub fn bring_here(h: isize, to: &MonitorInfo, monitors: &[MonitorInfo]) -> Result<(), String> {
    let w = hwnd(h);
    // SAFETY : lectures d'état.
    unsafe {
        if !IsWindow(Some(w)).as_bool() {
            return Err("la fenêtre est fermée".into());
        }
        if IsHungAppWindow(w).as_bool() {
            return Err("la fenêtre ne répond pas".into());
        }
    }
    let _dpi = DpiAware::enter();
    // SAFETY : fenêtre vérifiée ci-dessus.
    unsafe {
        if IsIconic(w).as_bool() {
            let _ = ShowWindow(w, SW_RESTORE);
        }
    }
    let here = |w: HWND| frame_bounds(w).and_then(|f| monitor_of(f, monitors)).map(|m| m.id.clone());
    if here(w).as_deref() != Some(to.id.as_str()) {
        move_to(w, to, monitors)?;
    }
    bring_to_front(h);
    Ok(())
}

fn move_to(w: HWND, to: &MonitorInfo, monitors: &[MonitorInfo]) -> Result<(), String> {
    // SAFETY : lecture d'état puis restauration d'une fenêtre existante.
    let maximized = unsafe { IsZoomed(w) }.as_bool();
    if maximized {
        // SAFETY : idem.
        let _ = unsafe { ShowWindow(w, SW_RESTORE) };
    }
    let (frame, rect) = frame_bounds(w).zip(window_rect(w)).ok_or("fenêtre illisible")?;
    let from = monitor_of(frame, monitors).ok_or("écran de la fenêtre inconnu")?;
    let goal = target_frame(frame, from, to);
    let ratio = f64::from(to.scale) / f64::from(from.scale.max(0.1));
    set_rect(w, Insets::between(rect, frame).scaled(ratio).around(goal))?;
    // Changement d'échelle : l'appli a pu se redimensionner elle-même (WM_DPICHANGED) et ses
    // bords invisibles ont changé d'épaisseur. Bords relus, cadre reposé exactement.
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(40));
        let Some((frame, rect)) = frame_bounds(w).zip(window_rect(w)) else { break };
        if frame == goal {
            break;
        }
        set_rect(w, Insets::between(rect, frame).around(goal))?;
    }
    if maximized {
        // SAFETY : fenêtre existante.
        let _ = unsafe { ShowWindow(w, SW_MAXIMIZE) };
    }
    Ok(())
}

/// Changement d'une fenêtre de premier niveau, vu par les crochets d'événements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Affichée, cachée, masquée ou démasquée par DWM, réduite ou restaurée.
    Shown(isize),
    /// Détruite : seule une fenêtre déjà comptée intéresse (l'appelant sait lesquelles).
    Destroyed(isize),
}

thread_local! {
    static WATCH: RefCell<Option<crossbeam_channel::Sender<Change>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, event: u32, h: HWND, id_object: i32, id_child: i32, _: u32, _: u32) {
    // OBJID_WINDOW = 0, CHILDID_SELF = 0 : la fenêtre elle-même, pas un de ses éléments.
    if id_object != 0 || id_child != 0 || h.0.is_null() {
        return;
    }
    let change = if event == EVENT_OBJECT_DESTROY {
        Change::Destroyed(h.0 as isize)
    } else {
        // SAFETY : simples lectures ; une fenêtre fille ne compte pas.
        if unsafe { GetAncestor(h, GA_PARENT) } != unsafe { GetDesktopWindow() } {
            return;
        }
        Change::Shown(h.0 as isize)
    };
    WATCH.with_borrow(|tx| {
        if let Some(tx) = tx {
            let _ = tx.send(change);
        }
    });
}

/// Surveille l'apparition et la disparition des fenêtres sur un fil dédié (crochets
/// d'événements de Windows, aucune scrutation). Les événements arrivent bruts : à l'appelant de
/// les regrouper.
pub fn watch(tx: crossbeam_channel::Sender<Change>) -> anyhow::Result<()> {
    std::thread::Builder::new()
        .name("fenetres-crochets".into())
        .spawn(move || {
            WATCH.with_borrow_mut(|w| *w = Some(tx));
            let ranges = [
                (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
                (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
                (EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND),
            ];
            // SAFETY : rappel `extern "system"` valide pour toute la vie du fil ; les crochets
            // hors contexte sont servis par la boucle de messages de ce fil.
            unsafe {
                for (from, to) in ranges {
                    // Nos propres fenêtres (panneau, palette) ne comptent jamais.
                    let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
                    if SetWinEventHook(from, to, None, Some(on_event), 0, 0, flags).is_invalid() {
                        log::error!("crochet des fenêtres impossible ({from:#x}..{to:#x})");
                    }
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("fil des fenêtres : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_never_includes_our_own_windows() {
        // Le processus de test n'a pas de fenêtre ; tout ce qui est listé est ailleurs.
        let own = std::process::id();
        let monitors = crate::monitors::list();
        for w in list(own, &monitors) {
            assert!(!w.title.trim().is_empty());
            assert!(!w.monitor.is_empty() || monitors.is_empty());
        }
        assert!(handles(own).iter().all(|&h| pid_of(hwnd(h)) != own));
    }
}
