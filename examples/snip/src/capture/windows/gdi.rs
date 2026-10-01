//! GDI capture of one monitor, for outputs Desktop Duplication can't reach.

use anyhow::{Result, bail};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};

use super::super::{Frame, bgra_to_rgba};
use crate::geometry::DisplayArea;

pub fn capture(area: DisplayArea, native_id: u64) -> Result<Frame> {
    let bounds = area.bounds();
    let (width, height) = (bounds.width, bounds.height);
    let mut bgra = vec![0u8; width as usize * height as usize * 4];
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let previous = SelectObject(memory, bitmap);
        // CAPTUREBLT includes layered windows, such as menus and tooltips.
        let copied = BitBlt(
            memory,
            0,
            0,
            width,
            height,
            screen,
            bounds.x,
            bounds.y,
            SRCCOPY | CAPTUREBLT,
        );
        // GetDIBits needs the bitmap out of every device context.
        SelectObject(memory, previous);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: rows top to bottom.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = GetDIBits(
            memory,
            bitmap,
            0,
            height as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        copied?;
        if rows != height {
            bail!("GDI copied {rows} of {height} rows");
        }
    }
    let pixels = bgra_to_rgba(&bgra, width as usize, height as usize, width as usize * 4);
    Frame::new(area, native_id, pixels)
}
