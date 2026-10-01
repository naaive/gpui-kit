//! Freezing the screen: one still frame per display, plus the windows on
//! it, taken before any of our own windows appear.
//!
//! Each platform captures natively behind [`Capturer`]: DXGI Desktop
//! Duplication (GDI as fallback) on Windows, ScreenCaptureKit on macOS, X11
//! shared memory or the screenshot portal on Linux. Everything above this
//! module sees only [`Frame`]s in physical desktop pixels.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::{
    geometry::{DisplayArea, PhysPoint, PhysRect, WindowSnapshot},
    scene::Color,
};

/// One display's image when the screen was frozen.
#[derive(Clone)]
pub struct Frame {
    /// Opaque RGBA, row-major, `width * height * 4` bytes. A `Vec` rather
    /// than a slice, so the captured buffer is shared without a copy.
    pixels: Arc<Vec<u8>>,
    area: DisplayArea,
    /// The platform's identifier for the display, used to find the GPUI
    /// display it belongs to.
    native_id: u64,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("area", &self.area)
            .field("native_id", &self.native_id)
            .finish_non_exhaustive()
    }
}

impl Frame {
    /// A frame from RGBA pixels covering `area`'s bounds exactly.
    pub fn new(area: DisplayArea, native_id: u64, pixels: Vec<u8>) -> Result<Self> {
        let bounds = area.bounds();
        let expected = bounds.width as usize * bounds.height as usize * 4;
        if bounds.is_empty() || pixels.len() != expected {
            bail!(
                "a {}×{} frame needs {expected} bytes, got {}",
                bounds.width,
                bounds.height,
                pixels.len()
            );
        }
        Ok(Self {
            pixels: Arc::new(pixels),
            area,
            native_id,
        })
    }

    pub fn area(&self) -> DisplayArea {
        self.area
    }

    /// This frame at another interface scale, for a capture API that
    /// can't tell (the Wayland screenshot portal).
    pub fn with_scale(mut self, scale: f32) -> Self {
        self.area = DisplayArea::new(self.area.bounds(), scale);
        self
    }

    pub fn bounds(&self) -> PhysRect {
        self.area.bounds()
    }

    pub fn native_id(&self) -> u64 {
        self.native_id
    }

    /// Opaque RGBA rows.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The color at a desktop position, if it is on this display.
    pub fn pixel(&self, at: PhysPoint) -> Option<Color> {
        let bounds = self.bounds();
        if !bounds.contains(at) {
            return None;
        }
        let offset =
            ((at.y - bounds.y) as usize * bounds.width as usize + (at.x - bounds.x) as usize) * 4;
        let pixel = &self.pixels[offset..offset + 4];
        Some(Color::rgb(pixel[0], pixel[1], pixel[2]))
    }

    /// The part of the frame inside `rect`, a desktop rectangle.
    pub fn crop(&self, rect: PhysRect) -> Option<image::RgbaImage> {
        let bounds = self.bounds();
        let rect = rect.intersect(&bounds)?;
        let row_bytes = rect.width as usize * 4;
        let mut pixels = Vec::with_capacity(row_bytes * rect.height as usize);
        for y in rect.y..rect.bottom() {
            let start = ((y - bounds.y) as usize * bounds.width as usize
                + (rect.x - bounds.x) as usize)
                * 4;
            pixels.extend_from_slice(&self.pixels[start..start + row_bytes]);
        }
        image::RgbaImage::from_raw(rect.width as u32, rect.height as u32, pixels)
    }
}

/// Everything frozen at one instant.
#[derive(Clone, Debug, Default)]
pub struct CaptureSet {
    frames: Vec<Frame>,
    /// Frontmost first.
    windows: Vec<WindowSnapshot>,
    /// Where the pointer was.
    pointer: Option<PhysPoint>,
}

impl CaptureSet {
    pub fn new(frames: Vec<Frame>, windows: Vec<WindowSnapshot>) -> Self {
        Self {
            frames,
            windows,
            pointer: None,
        }
    }

    pub fn with_pointer(mut self, pointer: Option<PhysPoint>) -> Self {
        self.pointer = pointer;
        self
    }

    pub fn pointer(&self) -> Option<PhysPoint> {
        self.pointer
    }

    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    pub fn with_frames(mut self, frames: Vec<Frame>) -> Self {
        self.frames = frames;
        self
    }

    pub fn windows(&self) -> &[WindowSnapshot] {
        &self.windows
    }

    /// The frame of the display containing `point`.
    pub fn frame_at(&self, point: PhysPoint) -> Option<&Frame> {
        self.frames
            .iter()
            .find(|frame| frame.bounds().contains(point))
    }
}

/// Captures the displays of one platform.
pub trait Capturer: Send + Sync {
    /// One frame per attached display.
    fn capture_displays(&self) -> Result<Vec<Frame>>;

    /// The visible top-level windows, frontmost first; empty where the
    /// platform doesn't tell applications (Wayland).
    fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        Vec::new()
    }

    /// Where the pointer is, in desktop pixels.
    fn pointer(&self) -> Option<PhysPoint> {
        None
    }
}

/// Freezes every display and, with `detect_windows`, records the windows.
/// Runs on a background thread: capture can take tens of milliseconds.
pub fn capture_all(capturer: &dyn Capturer, detect_windows: bool) -> Result<CaptureSet> {
    // Windows first: the frames should show the windows as listed, and
    // enumerating is far quicker than copying pixels.
    let windows = if detect_windows {
        capturer.window_snapshots()
    } else {
        Vec::new()
    };
    let frames = capturer.capture_displays()?;
    if frames.is_empty() {
        bail!("no display could be captured");
    }
    Ok(CaptureSet::new(frames, windows).with_pointer(capturer.pointer()))
}

/// The capturer for the platform this runs on.
pub fn platform_capturer() -> Arc<dyn Capturer> {
    #[cfg(target_os = "windows")]
    return Arc::new(windows::WindowsCapturer);
    #[cfg(target_os = "macos")]
    return Arc::new(macos::MacCapturer);
    #[cfg(target_os = "linux")]
    return Arc::new(linux::LinuxCapturer);
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    return Arc::new(Unsupported);
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
struct Unsupported;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
impl Capturer for Unsupported {
    fn capture_displays(&self) -> Result<Vec<Frame>> {
        bail!("screen capture isn't supported on this platform")
    }
}

/// Converts BGRA rows with any stride into tight RGBA, forcing alpha opaque:
/// screens have no transparency, and some capture APIs leave alpha at zero.
pub(crate) fn bgra_to_rgba(source: &[u8], width: usize, height: usize, stride: usize) -> Vec<u8> {
    convert_rows(source, height, stride, width * 4, |row, out| {
        for (pixel, out) in row[..width * 4]
            .chunks_exact(4)
            .zip(out.chunks_exact_mut(4))
        {
            out.copy_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
        }
    })
}

/// Converts `height` source rows of `stride` bytes into tight rows of
/// `row_bytes`, spreading the rows over the processor's cores: a 4K frame
/// is tens of megabytes, converted while the user waits for the overlay.
pub(crate) fn convert_rows(
    source: &[u8],
    height: usize,
    stride: usize,
    row_bytes: usize,
    convert: impl Fn(&[u8], &mut [u8]) + Sync,
) -> Vec<u8> {
    let mut pixels = vec![0; row_bytes * height];
    if row_bytes == 0 || height == 0 {
        return pixels;
    }
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get().min(8));
    let band_rows = height.div_ceil(threads);
    let convert = &convert;
    std::thread::scope(|scope| {
        for (band_ix, band) in pixels.chunks_mut(band_rows * row_bytes).enumerate() {
            let first_row = band_ix * band_rows;
            scope.spawn(move || {
                for (row_ix, out) in band.chunks_exact_mut(row_bytes).enumerate() {
                    let start = (first_row + row_ix) * stride;
                    convert(&source[start..], out);
                }
            });
        }
    });
    pixels
}

/// A capture that hands out fixed frames, for tests.
#[cfg(test)]
pub struct FakeCapturer {
    frames: Vec<Frame>,
    windows: Vec<WindowSnapshot>,
}

#[cfg(test)]
impl FakeCapturer {
    pub fn new(frames: Vec<Frame>, windows: Vec<WindowSnapshot>) -> Self {
        Self { frames, windows }
    }

    /// A display of one solid color.
    pub fn solid_frame(bounds: PhysRect, scale: f32, color: Color) -> Frame {
        let pixels =
            [color.r, color.g, color.b, 255].repeat((bounds.width * bounds.height) as usize);
        Frame::new(DisplayArea::new(bounds, scale), 0, pixels).unwrap()
    }
}

#[cfg(test)]
impl Capturer for FakeCapturer {
    fn capture_displays(&self) -> Result<Vec<Frame>> {
        Ok(self.frames.clone())
    }

    fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        self.windows.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(bounds: PhysRect) -> Frame {
        let mut pixels = Vec::new();
        for y in 0..bounds.height {
            for x in 0..bounds.width {
                pixels.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        Frame::new(DisplayArea::new(bounds, 1.), 0, pixels).unwrap()
    }

    #[test]
    fn test_frame_rejects_wrong_size() {
        let area = DisplayArea::new(PhysRect::new(0, 0, 2, 2), 1.);
        assert!(Frame::new(area, 0, vec![0; 15]).is_err());
        assert!(Frame::new(area, 0, vec![0; 16]).is_ok());
    }

    #[test]
    fn test_pixel_and_crop_use_desktop_coordinates() {
        let frame = gradient(PhysRect::new(-100, 50, 20, 10));
        assert_eq!(
            frame.pixel(PhysPoint::new(-95, 53)),
            Some(Color::rgb(5, 3, 0))
        );
        assert_eq!(frame.pixel(PhysPoint::new(0, 0)), None);

        let crop = frame.crop(PhysRect::new(-90, 55, 4, 2)).unwrap();
        assert_eq!(crop.dimensions(), (4, 2));
        assert_eq!(crop.get_pixel(0, 0).0, [10, 5, 0, 255]);
        assert_eq!(crop.get_pixel(3, 1).0, [13, 6, 0, 255]);

        let clipped = frame.crop(PhysRect::new(-110, 40, 20, 20)).unwrap();
        assert_eq!(clipped.dimensions(), (10, 10), "cut to the display");
    }

    #[test]
    fn test_bgra_to_rgba_skips_stride_padding() {
        let source = [
            1, 2, 3, 0, 4, 5, 6, 0, 99, 99, 7, 8, 9, 0, 10, 11, 12, 0, 99, 99,
        ];
        assert_eq!(
            bgra_to_rgba(&source, 2, 2, 10),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }
}
