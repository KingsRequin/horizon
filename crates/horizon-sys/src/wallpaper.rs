//! Image du fond d'écran tel qu'il est affiché, même sous les fenêtres : la fenêtre du bureau
//! (`Progman`) est rendue par `PrintWindow` avec `PW_RENDERFULLCONTENT`, ce qui comprend le
//! rendu DirectX de Wallpaper Engine (scène, vidéo, appli) et ses listes de lecture. Seule la
//! zone demandée est copiée (~25 ms pour un écran 1080p).

use horizon_core::geometry::Rect;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HGDIOBJ, SelectObject, SetViewportOrgEx,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect};
use windows::core::w;

/// Rend aussi le contenu DirectX (absent des en-têtes de la caisse `windows`).
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);
/// Côté de la grille de points relevés dans la zone (l'analyse de couleur n'en demande pas plus).
const SAMPLES: usize = 96;

/// Image réduite (`SAMPLES` points de large au plus, RGBA) de la zone `region` du bureau
/// (coordonnées physiques de l'écran virtuel).
pub fn capture(region: Rect) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    anyhow::ensure!(region.w > 0 && region.h > 0, "zone vide");
    // SAFETY : chaînes constantes ; une fenêtre absente donne une erreur.
    let desktop = unsafe { FindWindowW(w!("Progman"), None) }?;
    let mut win = RECT::default();
    // SAFETY : fenêtre valide, sortie RECT.
    unsafe { GetWindowRect(desktop, &mut win) }?;
    let bgra = render(desktop, region.x - win.left, region.y - win.top, region.w, region.h)?;
    Ok(downsample(&bgra, region.w as usize, region.h as usize))
}

/// Rend la fenêtre dans une image `w` × `h` dont le coin haut gauche est le point (`ox`, `oy`)
/// de la fenêtre ; pixels BGRA de haut en bas.
fn render(hwnd: HWND, ox: i32, oy: i32, w: i32, h: i32) -> anyhow::Result<Vec<u8>> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            // Hauteur négative : lignes de haut en bas.
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY : objets GDI créés ici et rendus à la fin, dans tous les cas ; le tampon de la
    // section DIB (`bits`, w × h × 4 octets) vit tant que la section n'est pas supprimée.
    unsafe {
        let dc = CreateCompatibleDC(None);
        anyhow::ensure!(!dc.is_invalid(), "contexte de dessin indisponible");
        let mut bits = std::ptr::null_mut();
        let bmp = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(e) => {
                let _ = DeleteDC(dc);
                return Err(e.into());
            }
        };
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let _ = SetViewportOrgEx(dc, -ox, -oy, None);
        let ok = PrintWindow(hwnd, dc, PW_RENDERFULLCONTENT).as_bool();
        let out = ok.then(|| std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec());
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        out.ok_or_else(|| anyhow::anyhow!("rendu du bureau refusé"))
    }
}

/// Grille de points (RGBA) relevés dans une image BGRA.
fn downsample(bgra: &[u8], w: usize, h: usize) -> (u32, u32, Vec<u8>) {
    let step = w.div_ceil(SAMPLES).max(1);
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = Vec::with_capacity(ow * oh * 4);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let i = (y * w + x) * 4;
            out.extend_from_slice(&[bgra[i + 2], bgra[i + 1], bgra[i], 255]);
        }
    }
    (ow as u32, oh as u32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_swaps_channels_and_shrinks() {
        let (w, h) = (300, 100);
        let mut img = Vec::new();
        for _ in 0..w * h {
            img.extend_from_slice(&[10, 20, 30, 0]);
        }
        let (ow, oh, px) = downsample(&img, w, h);
        assert_eq!((ow, oh), (75, 25), "un point sur 4 (300 / 96 arrondi au-dessus)");
        assert_eq!(px.len(), (ow * oh * 4) as usize);
        assert_eq!(&px[..4], &[30, 20, 10, 255]);
    }
}
