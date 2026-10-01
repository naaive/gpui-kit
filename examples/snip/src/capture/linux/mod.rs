//! Capture on Linux.
//!
//! On X11 the root window is read directly, and its window stack drives
//! automatic selection. Wayland gives ordinary clients neither the screen
//! nor the windows; there the screenshot portal asks the compositor for an
//! image of the whole desktop, and selection is by hand.

mod portal;
mod x11;

use anyhow::Result;

use super::{Capturer, Frame};
use crate::geometry::{PhysPoint, WindowSnapshot};

#[derive(Default)]
pub struct LinuxCapturer;

impl Capturer for LinuxCapturer {
    fn capture_displays(&self) -> Result<Vec<Frame>> {
        if is_wayland() {
            portal::capture()
        } else {
            x11::capture()
        }
    }

    fn window_snapshots(&self) -> Vec<WindowSnapshot> {
        if is_wayland() {
            return Vec::new();
        }
        x11::window_snapshots().unwrap_or_else(|error| {
            tracing::warn!("cannot list the windows: {error:#}");
            Vec::new()
        })
    }

    fn pointer(&self) -> Option<PhysPoint> {
        if is_wayland() {
            return None;
        }
        x11::pointer().ok()
    }
}

/// Whether this is a Wayland session, as GPUI decides it.
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
}
