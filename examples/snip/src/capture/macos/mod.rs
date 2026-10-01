//! Capture on macOS.
//!
//! ScreenCaptureKit takes one screenshot per display, which needs the user's
//! Screen Recording permission. macOS places displays in points, not pixels,
//! and each display can have its own backing scale; the desktop pixels here
//! are points multiplied by the largest of those scales, so every display is
//! captured at that density (a lower-density display is upscaled) and one
//! selection can cross displays without changing resolution midway.

mod sck;
mod window_list;

use anyhow::{Result, bail};
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGError, CGEvent,
    CGGetActiveDisplayList, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
};

use super::{Capturer, Frame};
use crate::geometry::{PhysPoint, PhysRect, WindowSnapshot};

/// Displays listed at most; far more than a Mac can drive.
const MAX_DISPLAYS: usize = 32;

#[derive(Default)]
pub struct MacCapturer;

impl Capturer for MacCapturer {
    fn capture_displays(&self) -> Result<Vec<Frame>> {
        ensure_permission()?;
        let displays = displays();
        sck::capture(&displays, desktop_scale(&displays))
    }

    fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        window_list::snapshots(desktop_scale(&displays()))
    }

    fn pointer(&self) -> Option<PhysPoint> {
        // An event created without a source carries the current pointer
        // location, in global points from the primary display's top left.
        let event = CGEvent::new(None)?;
        let location = CGEvent::location(Some(&event));
        let scale = desktop_scale(&displays());
        Some(PhysPoint::new(
            to_physical(location.x, scale),
            to_physical(location.y, scale),
        ))
    }
}

/// A display's place on the desktop, in points, and its backing scale.
#[derive(Clone, Copy, Debug)]
struct Display {
    /// The `CGDirectDisplayID`, which is also GPUI's display id on macOS.
    native_id: CGDirectDisplayID,
    points: CGRect,
    scale: f64,
}

/// Every active display.
fn displays() -> Vec<Display> {
    let mut ids = [0 as CGDirectDisplayID; MAX_DISPLAYS];
    let mut count = 0u32;
    let error =
        unsafe { CGGetActiveDisplayList(MAX_DISPLAYS as u32, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success {
        tracing::error!("cannot list the displays: {error:?}");
        return Vec::new();
    }
    ids[..(count as usize).min(MAX_DISPLAYS)]
        .iter()
        .map(|&native_id| Display {
            native_id,
            points: CGDisplayBounds(native_id),
            scale: backing_scale(native_id),
        })
        .collect()
}

/// Pixels per point in a display's current mode: 2 on a Retina display at a
/// scaled resolution, 1 on most external monitors.
fn backing_scale(display: CGDirectDisplayID) -> f64 {
    let Some(mode) = CGDisplayCopyDisplayMode(display) else {
        return 1.;
    };
    let points = CGDisplayMode::width(Some(&mode));
    let pixels = CGDisplayMode::pixel_width(Some(&mode));
    if points == 0 {
        1.
    } else {
        pixels as f64 / points as f64
    }
}

/// The desktop's pixels per point: the densest display's, so no display is
/// captured below its own resolution.
fn desktop_scale(displays: &[Display]) -> f64 {
    largest_scale(displays.iter().map(|display| display.scale))
}

fn largest_scale(scales: impl IntoIterator<Item = f64>) -> f64 {
    scales
        .into_iter()
        .filter(|scale| scale.is_finite() && *scale > 0.)
        .fold(None, |largest: Option<f64>, scale| {
            Some(largest.map_or(scale, |largest| largest.max(scale)))
        })
        .unwrap_or(1.)
}

/// A coordinate in global points as a desktop pixel.
fn to_physical(points: f64, scale: f64) -> i32 {
    (points * scale).round() as i32
}

/// A rectangle in global points in desktop pixels. Edges are rounded rather
/// than origin and size, so displays that touch in points still touch.
fn to_physical_rect(rect: CGRect, scale: f64) -> PhysRect {
    PhysRect::from_edges(
        to_physical(rect.origin.x, scale),
        to_physical(rect.origin.y, scale),
        to_physical(rect.origin.x + rect.size.width, scale),
        to_physical(rect.origin.y + rect.size.height, scale),
    )
}

/// Fails unless the user has allowed this app to record the screen. Without
/// the permission ScreenCaptureKit only shows the wallpaper and the menu bar,
/// so the first refusal also asks macOS to prompt for it.
fn ensure_permission() -> Result<()> {
    if CGPreflightScreenCaptureAccess() {
        return Ok(());
    }
    CGRequestScreenCaptureAccess();
    bail!(
        "Allow Snip to record the screen in System Settings › Privacy & Security › Screen & System Audio Recording, then try again."
    )
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::{CGPoint, CGSize};

    use super::*;

    #[test]
    fn test_desktop_scale_is_the_densest_display() {
        assert_eq!(largest_scale([1., 2., 1.5]), 2.);
        assert_eq!(largest_scale([f64::NAN, 0., 1.]), 1.);
        assert_eq!(largest_scale([]), 1., "no display");
    }

    #[test]
    fn test_points_scale_to_desktop_pixels() {
        // A display left of the primary one, which owns the origin.
        let rect = CGRect::new(CGPoint::new(-1280., -200.), CGSize::new(1280., 800.));
        assert_eq!(
            to_physical_rect(rect, 2.),
            PhysRect::new(-2560, -400, 2560, 1600)
        );
        assert_eq!(to_physical(10.25, 2.), 21, "rounded");

        // Edges that meet in points still meet after scaling.
        let left = CGRect::new(CGPoint::new(0., 0.), CGSize::new(100.3, 10.));
        let right = CGRect::new(CGPoint::new(100.3, 0.), CGSize::new(50., 10.));
        let (left, right) = (to_physical_rect(left, 1.5), to_physical_rect(right, 1.5));
        assert_eq!(left.right(), right.x);
    }

    /// Captures the real displays and writes them to `SNIP_SMOKE_DIR` (or the
    /// temporary directory). Run by hand with `--ignored`; it needs the
    /// Screen Recording permission.
    #[test]
    #[ignore]
    fn smoke_capture_displays() {
        let capturer = MacCapturer;
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
            "{} frames in {elapsed:?}, {} windows, pointer at {:?}",
            frames.len(),
            windows.len(),
            capturer.pointer()
        );
        assert!(!frames.is_empty());
    }
}
