//! Moving the pointer and turning the wheel, for scrolling capture.
//!
//! Only Windows for now: macOS posts scroll events in points rather than
//! the physical pixels capture works in, and Linux has no common way to
//! inject input under Wayland.

use anyhow::Result;

use crate::geometry::PhysPoint;

#[cfg(target_os = "windows")]
mod platform {
    use anyhow::{Context as _, Result};
    use windows::Win32::{
        Foundation::POINT,
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
            },
            WindowsAndMessaging::{GetCursorPos, SetCursorPos, WHEEL_DELTA},
        },
    };

    use crate::geometry::PhysPoint;

    pub fn move_pointer(to: PhysPoint) -> Result<()> {
        unsafe { SetCursorPos(to.x, to.y) }.context("cannot move the pointer")
    }

    pub fn pointer() -> Option<PhysPoint> {
        let mut point = POINT::default();
        unsafe { GetCursorPos(&mut point) }.ok()?;
        Some(PhysPoint::new(point.x, point.y))
    }

    /// Turns the wheel `notches` toward the user, at the pointer.
    pub fn scroll_down(notches: i32) -> Result<()> {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    mouseData: (-(WHEEL_DELTA as i32) * notches) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    ..Default::default()
                },
            },
        };
        let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
        anyhow::ensure!(sent == 1, "the wheel event was blocked");
        Ok(())
    }
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

pub fn move_pointer(to: PhysPoint) -> Result<()> {
    #[cfg(target_os = "windows")]
    return platform::move_pointer(to);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = to;
        anyhow::bail!("moving the pointer isn't supported here")
    }
}

pub fn pointer() -> Option<PhysPoint> {
    #[cfg(target_os = "windows")]
    return platform::pointer();
    #[cfg(not(target_os = "windows"))]
    None
}

/// Turns the wheel `notches` toward the user, where the pointer is.
pub fn scroll_down(notches: i32) -> Result<()> {
    #[cfg(target_os = "windows")]
    return platform::scroll_down(notches);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = notches;
        anyhow::bail!("scrolling isn't supported here")
    }
}
