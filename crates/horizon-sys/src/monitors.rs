//! Liste des écrans : rectangle, zone de travail, échelle et identifiant matériel stable.

use crate::win32::{from_wide, to_rect};
use horizon_core::geometry::MonitorInfo;
use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

const MONITORINFOF_PRIMARY: u32 = 1;

/// Correspondance nom GDI (`\\.\DISPLAY1`) → chemin matériel de l'écran, qui ne change pas
/// quand Windows renumérote les écrans.
fn device_paths() -> Vec<(String, String)> {
    let mut out = Vec::new();
    // SAFETY : tampons dimensionnés par GetDisplayConfigBufferSizes, en-têtes de la bonne taille.
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        )
        .is_err()
        {
            return out;
        }
        for p in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                },
                ..Default::default()
            };
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) == 0
                && DisplayConfigGetDeviceInfo(&mut target.header) == 0
            {
                let path = from_wide(&target.monitorDevicePath);
                if !path.is_empty() {
                    out.push((from_wide(&src.viewGdiDeviceName), path));
                }
            }
        }
    }
    out
}

/// Tous les écrans actifs, dans l'ordre de Windows.
pub fn list() -> Vec<MonitorInfo> {
    unsafe extern "system" fn callback(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY : `data` pointe vers le Vec de `list`, vivant pendant l'énumération synchrone.
        let handles = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        handles.push(m);
        true.into()
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY : énumération synchrone, le pointeur reste valide pendant l'appel.
    let ok = unsafe {
        EnumDisplayMonitors(None, None, Some(callback), LPARAM(&mut handles as *mut _ as isize))
    };
    if !ok.as_bool() {
        log::warn!("énumération des écrans échouée");
    }
    let paths = device_paths();
    handles
        .into_iter()
        .filter_map(|h| {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            // SAFETY : structure étendue de la taille annoncée dans cbSize.
            if !unsafe { GetMonitorInfoW(h, &mut info.monitorInfo) }.as_bool() {
                return None;
            }
            let (mut dx, mut dy) = (96u32, 96u32);
            // SAFETY : handle d'écran valide, sorties en u32.
            let _ = unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            let device_name = from_wide(&info.szDevice);
            let id = paths
                .iter()
                .find(|(gdi, _)| gdi.eq_ignore_ascii_case(&device_name))
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| device_name.clone());
            let rect = to_rect(info.monitorInfo.rcMonitor);
            Some(MonitorInfo {
                id,
                device_name,
                rect,
                work: to_rect(info.monitorInfo.rcWork),
                scale: dx as f32 / 96.0,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                portrait: rect.h > rect.w,
            })
        })
        .collect()
}
