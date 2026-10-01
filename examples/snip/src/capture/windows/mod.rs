//! Capture on Windows.
//!
//! DXGI Desktop Duplication copies each output's image straight from the
//! compositor, which is fast and sees hardware-accelerated content. Outputs
//! it can't duplicate (remote sessions, some hybrid-GPU laptops, protected
//! content changes) fall back to a GDI copy of that monitor.

mod controls;
mod dxgi;
mod gdi;
mod hdr;
mod window_list;

use anyhow::Result;
use windows::Win32::{
    Foundation::{BOOL, LPARAM, POINT, RECT, TRUE},
    Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
    UI::{
        HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
        WindowsAndMessaging::GetCursorPos,
    },
};

use super::{Capturer, Frame};
use crate::geometry::{DisplayArea, PhysPoint, PhysRect, WindowSnapshot};

#[derive(Default)]
pub struct WindowsCapturer;

impl Capturer for WindowsCapturer {
    fn capture_displays(&self) -> Result<Vec<Frame>> {
        let monitors = monitors();
        let mut frames = dxgi::capture().unwrap_or_else(|error| {
            tracing::warn!("desktop duplication failed, using GDI: {error:#}");
            Vec::new()
        });
        frames.retain(|frame| {
            monitors
                .iter()
                .any(|monitor| monitor.native_id == frame.native_id())
        });
        for monitor in &monitors {
            if frames
                .iter()
                .any(|frame| frame.native_id() == monitor.native_id)
            {
                continue;
            }
            match gdi::capture(monitor.area, monitor.native_id) {
                Ok(frame) => frames.push(frame),
                Err(error) => tracing::error!("cannot capture a display: {error:#}"),
            }
        }
        // The duplication reports each output's own scale-less bounds; the
        // monitor list knows the DPI, so the areas come from it.
        for frame in &mut frames {
            if let Some(monitor) = monitors
                .iter()
                .find(|monitor| monitor.native_id == frame.native_id())
            {
                frame.area = monitor.area;
            }
        }
        Ok(frames)
    }

    fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        window_list::snapshots()
    }

    fn pointer(&self) -> Option<PhysPoint> {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.ok()?;
        Some(PhysPoint::new(point.x, point.y))
    }

    fn window_controls(&self, window: &WindowSnapshot) -> Vec<PhysRect> {
        if window.native_id() == 0 {
            return Vec::new();
        }
        controls::controls(window.native_id(), window.frame())
    }
}

/// A monitor's place on the desktop and its DPI scale.
#[derive(Clone, Copy, Debug)]
struct Monitor {
    native_id: u64,
    area: DisplayArea,
}

/// Every attached monitor. A monitor's identifier is its `HMONITOR`, which
/// is also what GPUI uses as the display id on Windows.
fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        found: LPARAM,
    ) -> BOOL {
        unsafe { (*(found.0 as *mut Vec<HMONITOR>)).push(monitor) };
        TRUE
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut handles as *mut Vec<HMONITOR> as isize),
        );
    }
    handles
        .into_iter()
        .filter_map(|handle| {
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            unsafe { GetMonitorInfoW(handle, &mut info) }
                .as_bool()
                .then_some(())?;
            let rect = info.rcMonitor;
            let (mut dpi_x, mut dpi_y) = (96, 96);
            unsafe { GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }.ok();
            Some(Monitor {
                native_id: handle.0 as u64,
                area: DisplayArea::new(
                    PhysRect::from_edges(rect.left, rect.top, rect.right, rect.bottom),
                    dpi_x as f32 / 96.,
                ),
            })
        })
        .collect()
}

fn rect(rect: RECT) -> PhysRect {
    PhysRect::from_edges(rect.left, rect.top, rect.right, rect.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captures the real displays and writes them to `SNIP_SMOKE_DIR` (or the
    /// temporary directory). Run by hand with `--ignored`; CI machines have
    /// no desktop to duplicate.
    #[test]
    #[ignore]
    fn smoke_capture_displays() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init()
            .ok();
        let capturer = WindowsCapturer;
        let started = std::time::Instant::now();
        let frames = capturer.capture_displays().unwrap();
        let elapsed = started.elapsed();
        let windows = capturer.window_snapshots();
        let directory = std::env::var_os("SNIP_SMOKE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        for (ix, frame) in frames.iter().enumerate() {
            let image = frame.crop(frame.bounds()).unwrap();
            let path = directory.join(format!("snip-display-{ix}.png"));
            image.save(&path).unwrap();
            println!(
                "{:?} scale {} → {}",
                frame.bounds(),
                frame.area().scale(),
                path.display()
            );
        }
        println!(
            "{} frames in {elapsed:?}, {} windows",
            frames.len(),
            windows.len()
        );
        assert!(!frames.is_empty());
    }
}
