//! Platform conventions for a tray utility.

/// Keeps Snip out of the Dock and the application switcher on macOS, as a
/// utility that lives in the menu bar should be. GPUI makes every
/// application a regular one when it finishes launching; this runs after.
pub fn hide_dock_icon() {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

        if let Some(main_thread) = MainThreadMarker::new() {
            NSApplication::sharedApplication(main_thread)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    }
}

/// Makes sure a pop-up window is on screen, activating it or not.
///
/// On Windows a GPUI pop-up opened while another application is in front
/// can stay hidden; this shows it. The show is posted rather than sent:
/// a synchronous `ShowWindow` re-enters GPUI's window procedure while the
/// application is already borrowed. Focus is left to `activate_window`.
pub fn ensure_shown(window: &gpui_kit::Window, activate: bool) {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{
            IsWindowVisible, SW_SHOW, SW_SHOWNOACTIVATE, ShowWindowAsync,
        };

        let Some(hwnd) = hwnd(window) else {
            return;
        };
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                let command = if activate { SW_SHOW } else { SW_SHOWNOACTIVATE };
                let _ = ShowWindowAsync(hwnd, command);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (window, activate);
}

/// Makes the whole window translucent, from 0 (invisible) to 1 (opaque).
/// Returns false where the platform leaves it to what the window draws.
///
/// Windows draws Snip's windows opaque (see `main`), so a translucent pin
/// is a layered window that the compositor fades as a whole.
pub fn set_window_opacity(window: &gpui_kit::Window, opacity: f32) -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::{
            Foundation::COLORREF,
            UI::WindowsAndMessaging::{
                GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes,
                SetWindowLongPtrW, WS_EX_LAYERED,
            },
        };

        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let alpha = (opacity.clamp(0., 1.) * 255.).round() as u8;
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if style & WS_EX_LAYERED.0 as isize == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_LAYERED.0 as isize);
            }
            SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA).is_ok()
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, opacity);
        false
    }
}

/// Resizes the window from `from` to `to` (logical sizes) keeping the
/// point `anchor` (window-local, logical) where it is on screen, as a pin
/// zooms around the pointer. Returns false where the platform leaves the
/// window to grow from its top-left corner.
pub fn resize_window_around(
    window: &gpui_kit::Window,
    anchor: gpui_kit::Point<gpui_kit::Pixels>,
    from: gpui_kit::Size<gpui_kit::Pixels>,
    to: gpui_kit::Size<gpui_kit::Pixels>,
    cx: &gpui_kit::App,
) -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::{
            Foundation::RECT,
            UI::WindowsAndMessaging::{GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos},
        };

        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        let mut frame = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut frame) }.is_err() {
            return false;
        }
        let scale = window.scale_factor();
        let (from_width, from_height) = (f32::from(from.width), f32::from(from.height));
        if from_width <= 0. || from_height <= 0. {
            return false;
        }
        let (x, y) = (f32::from(anchor.x), f32::from(anchor.y));
        let (width, height) = (f32::from(to.width) * scale, f32::from(to.height) * scale);
        // The anchor's place on screen, and the same share of the new size.
        let left = frame.left as f32 + x * scale - x / from_width * width;
        let top = frame.top as f32 + y * scale - y / from_height * height;
        let (left, top, width, height) = (
            left.round() as i32,
            top.round() as i32,
            width.round() as i32,
            height.round() as i32,
        );
        // Deferred, as GPUI's own resize is: moving the window sends it
        // messages that would re-enter GPUI while it is updating.
        let hwnd = hwnd.0 as isize;
        window
            .spawn(cx, async move |_| unsafe {
                SetWindowPos(
                    windows::Win32::Foundation::HWND(hwnd as *mut std::ffi::c_void),
                    None,
                    left,
                    top,
                    width,
                    height,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .ok();
            })
            .detach();
        true
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, anchor, from, to, cx);
        false
    }
}

#[cfg(target_os = "windows")]
fn hwnd(window: &gpui_kit::Window) -> Option<windows::Win32::Foundation::HWND> {
    use raw_window_handle::RawWindowHandle;

    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return None;
    };
    Some(windows::Win32::Foundation::HWND(
        handle.hwnd.get() as *mut std::ffi::c_void
    ))
}
