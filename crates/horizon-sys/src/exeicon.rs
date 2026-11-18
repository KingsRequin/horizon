//! Icône d'un fichier, raccourci ou appli (48 px, PNG), **sans** la flèche des raccourcis :
//! on demande l'icône à la liste d'images du système sans masque de superposition.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_PIDL, SHGFI_SYSICONINDEX, SHGetFileInfoW, SHGetImageList,
    SHIL_EXTRALARGE, SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
use windows::core::{HSTRING, PCWSTR};

/// Pixels RGBA d'une icône (largeur, hauteur, octets).
fn icon_rgba(icon: HICON) -> Option<(u32, u32, Vec<u8>)> {
    // SAFETY : GetIconInfo crée deux bitmaps que l'on libère ; tampons dimensionnés d'après BITMAP.
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(icon, &mut info).ok()?;
        let free = |b: HBITMAP| {
            if !b.is_invalid() {
                let _ = DeleteObject(b.into());
            }
        };
        let mut bm = BITMAP::default();
        let ok = GetObjectW(
            info.hbmColor.into(),
            size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut _),
        ) != 0;
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        let mut out = None;
        if ok && w > 0 && h > 0 {
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // de haut en bas
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let dc = GetDC(None);
            let lines = GetDIBits(
                dc,
                info.hbmColor,
                0,
                h as u32,
                Some(buf.as_mut_ptr() as *mut _),
                &mut bi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, dc);
            if lines == h {
                // BGRA → RGBA ; une icône sans canal alpha est rendue opaque.
                let has_alpha = buf.chunks_exact(4).any(|p| p[3] != 0);
                for p in buf.chunks_exact_mut(4) {
                    p.swap(0, 2);
                    if !has_alpha {
                        p[3] = 255;
                    }
                }
                out = Some((w as u32, h as u32, buf));
            }
        }
        free(info.hbmColor);
        free(info.hbmMask);
        out
    }
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    let mut enc = png::Encoder::new(&mut png, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(rgba).ok()?;
    Some(png)
}

/// Fichier d'icône déclaré par un raccourci `.lnk` (utile quand il pointe vers une URL, comme
/// les jeux Steam).
fn lnk_icon_location(lnk: &str) -> Option<String> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::Interface;
    // SAFETY : objets COM du shell sur un fil initialisé en STA ; tampon de chemin borné.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(lnk), STGM_READ).ok()?;
        let mut buf = [0u16; 260];
        let mut index = 0;
        link.GetIconLocation(&mut buf, &mut index).ok()?;
        let path = crate::win32::from_wide(&buf);
        let path = std::env::var("SystemRoot")
            .map(|r| path.replace("%SystemRoot%", &r))
            .unwrap_or(path);
        (!path.is_empty() && std::path::Path::new(&path).exists()).then_some(path)
    }
}

/// `IconFile=` d'un raccourci Internet `.url` (raccourcis Steam du menu Démarrer).
fn url_icon_file(url_file: &str) -> Option<String> {
    let text = std::fs::read_to_string(url_file).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix("IconFile="))
        .map(|p| p.trim().to_owned())
        .filter(|p| std::path::Path::new(p).exists())
}

/// Icône PNG d'une cible du shell (exe, .lnk, dossier, `shell:AppsFolder\…`). `None` pour une
/// URL ou une cible introuvable.
pub fn icon_png(target: &str) -> Option<Vec<u8>> {
    let target = target.to_owned();
    crate::shell::run(move || icon_png_here(&target)).flatten()
}

fn icon_png_here(target: &str) -> Option<Vec<u8>> {
    let lower = target.to_ascii_lowercase();
    let declared = if lower.ends_with(".lnk") {
        lnk_icon_location(target)
    } else if lower.ends_with(".url") {
        url_icon_file(target)
    } else {
        None
    };
    if let Some(icon) = declared
        && let Some(png) = shell_icon_png(&icon)
    {
        return Some(png);
    }
    shell_icon_png(target)
}

fn shell_icon_png(target: &str) -> Option<Vec<u8>> {
    // SAFETY : sur le fil shell (COM initialisé) ; PIDL libéré par CoTaskMemFree ; HICON détruit.
    unsafe {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(target), None, &mut pidl, 0, None).ok()?;
        let mut sfi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(
            PCWSTR(pidl as *const u16),
            Default::default(),
            Some(&mut sfi),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_PIDL | SHGFI_SYSICONINDEX,
        );
        CoTaskMemFree(Some(pidl as *const _));
        if r == 0 {
            return None;
        }
        let list: IImageList = SHGetImageList(SHIL_EXTRALARGE as i32).ok()?;
        // ILD_TRANSPARENT sans masque de superposition : pas de flèche de raccourci.
        let icon = list.GetIcon(sfi.iIcon, ILD_TRANSPARENT.0).ok()?;
        let rgba = icon_rgba(icon);
        let _ = DestroyIcon(icon);
        let (w, h, px) = rgba?;
        encode_png(w, h, &px)
    }
}
