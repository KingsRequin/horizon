//! Section DIB 32 bits (BGRA prémultiplié) et son DC mémoire : la cible du rendu logiciel de
//! Slint, présentée par `UpdateLayeredWindowIndirect` (fenêtre layered) ou `BitBlt` (fenêtre
//! opaque).

use slint::platform::software_renderer::{PremultipliedRgbaColor, TargetPixel};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};

/// Pixel BGRA prémultiplié, l'ordre d'une section DIB 32 bits (0xAARRGGBB en mémoire).
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Bgra(u32);

impl Bgra {
    fn to_color(self) -> PremultipliedRgbaColor {
        let v = self.0;
        PremultipliedRgbaColor { red: (v >> 16) as u8, green: (v >> 8) as u8, blue: v as u8, alpha: (v >> 24) as u8 }
    }
    fn from_color(c: PremultipliedRgbaColor) -> Self {
        Self((c.alpha as u32) << 24 | (c.red as u32) << 16 | (c.green as u32) << 8 | c.blue as u32)
    }
}

impl TargetPixel for Bgra {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let mut x = self.to_color();
        x.blend(color);
        *self = Self::from_color(x);
    }
    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0xff00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32)
    }
    /// Fond transparent (et non noir opaque) : seul ce que la fenêtre dessine est visible.
    fn background() -> Self {
        Self(0)
    }
}

pub struct Dib {
    pub dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut Bgra,
    pub width: u32,
    pub height: u32,
}

impl Dib {
    pub fn new(width: u32, height: u32) -> anyhow::Result<Self> {
        let (width, height) = (width.max(1), height.max(1));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                // Hauteur négative : lignes de haut en bas, comme le tampon de Slint.
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY : appels GDI documentés ; `bits` reçoit le tampon de la section.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Ok(Self { dc, bitmap, old, bits: bits.cast(), width, height })
        }
    }

    pub fn pixels(&mut self) -> &mut [Bgra] {
        // SAFETY : la section fait largeur × hauteur pixels de 32 bits et vit autant que `self`.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        // SAFETY : objets créés par `new`, rendus dans l'ordre inverse.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
