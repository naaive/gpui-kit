//! Which display the launcher opens on.

use gpui_kit::{App, DisplayId};

use crate::shell::settings::ShowOn;

/// The display `show_on` names, or `None` for the primary display, which is
/// also where the launcher opens when the platform cannot tell (macOS and
/// Linux for now).
///
/// Call it before the launcher takes the front: the active window is the one
/// [`crate::window_layout::remember_frontmost`] recorded.
pub fn launcher_display(show_on: ShowOn, cx: &App) -> Option<DisplayId> {
    #[cfg(target_os = "windows")]
    {
        let monitor = match show_on {
            ShowOn::MouseScreen => win32::pointer_monitor(),
            ShowOn::ActiveWindowScreen => win32::window_monitor(crate::window_layout::frontmost()?),
            ShowOn::PrimaryScreen => return None,
        }?;
        // GPUI names a Windows display by its monitor handle.
        cx.displays()
            .into_iter()
            .map(|display| display.id())
            .find(|id| u64::from(*id) == monitor)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (show_on, cx);
        None
    }
}

#[cfg(target_os = "windows")]
mod win32 {
    use std::ffi::c_void;

    use windows::Win32::{
        Foundation::{HWND, POINT},
        Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint, MonitorFromWindow},
        UI::WindowsAndMessaging::GetCursorPos,
    };

    /// The monitor under the pointer, as a raw handle.
    pub fn pointer_monitor() -> Option<u64> {
        let mut point = POINT::default();
        // SAFETY: `point` is a valid place for the position.
        unsafe { GetCursorPos(&mut point) }.ok()?;
        // SAFETY: a plain query on a point.
        let monitor = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONULL) };
        (!monitor.is_invalid()).then_some(monitor.0 as u64)
    }

    /// The monitor most of the window `raw` is on, as a raw handle.
    pub fn window_monitor(raw: isize) -> Option<u64> {
        // SAFETY: a window that has closed since gives no monitor.
        let monitor = unsafe { MonitorFromWindow(HWND(raw as *mut c_void), MONITOR_DEFAULTTONULL) };
        (!monitor.is_invalid()).then_some(monitor.0 as u64)
    }
}
