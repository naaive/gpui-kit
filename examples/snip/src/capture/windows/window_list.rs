//! The visible windows, frontmost first, for automatic selection.

use std::ffi::c_void;

use windows::Win32::{
    Foundation::{BOOL, HWND, LPARAM, RECT, TRUE},
    Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
    UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GWL_EXSTYLE, GetWindowLongW, GetWindowRect, IsIconic,
        IsWindowVisible, WS_EX_TRANSPARENT,
    },
};

use super::rect;
use crate::geometry::{PhysRect, WindowSnapshot};

/// Child windows recorded per window at most; a window with thousands of
/// children (old-style dialogs, some IDEs) shouldn't stall the capture.
const MAX_PARTS: usize = 256;

pub fn snapshots() -> Vec<WindowSnapshot> {
    unsafe extern "system" fn collect(window: HWND, found: LPARAM) -> BOOL {
        unsafe { (*(found.0 as *mut Vec<HWND>)).push(window) };
        TRUE
    }
    // EnumWindows lists top-level windows in z-order, topmost first.
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(
            Some(collect),
            LPARAM(&mut handles as *mut Vec<HWND> as isize),
        );
    }
    handles
        .into_iter()
        .filter(|window| is_selectable(*window))
        .filter_map(|window| {
            let frame = visible_frame(window)?;
            Some(
                WindowSnapshot::new(frame)
                    .with_parts(parts(window, &frame))
                    .with_native_id(window.0 as u64),
            )
        })
        .collect()
}

/// Whether a window can be seen and picked: shown, not minimized, not
/// cloaked (suspended store apps and windows on other virtual desktops are
/// "visible" but cloaked), and not click-through like screen overlays.
fn is_selectable(window: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
            return false;
        }
        if GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 {
            return false;
        }
        let mut cloaked: u32 = 0;
        let is_cloaked = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0;
        !is_cloaked
    }
}

/// The frame a user sees, without the invisible resize border Windows 10
/// and later draw around windows.
fn visible_frame(window: HWND) -> Option<PhysRect> {
    let mut bounds = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .or_else(|_| GetWindowRect(window, &mut bounds))
        .ok()?;
    }
    let frame = rect(bounds);
    (!frame.is_empty()).then_some(frame)
}

fn parts(window: HWND, frame: &PhysRect) -> Vec<PhysRect> {
    unsafe extern "system" fn collect(child: HWND, found: LPARAM) -> BOOL {
        let found = unsafe { &mut *(found.0 as *mut Vec<HWND>) };
        if found.len() >= MAX_PARTS {
            return BOOL(0);
        }
        found.push(child);
        TRUE
    }
    let mut children: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumChildWindows(
            window,
            Some(collect),
            LPARAM(&mut children as *mut Vec<HWND> as isize),
        );
    }
    children
        .into_iter()
        .filter(|child| unsafe { IsWindowVisible(*child) }.as_bool())
        .filter_map(|child| {
            let mut bounds = RECT::default();
            unsafe { GetWindowRect(child, &mut bounds) }.ok()?;
            rect(bounds).intersect(frame)
        })
        .filter(|part| part.width >= 8 && part.height >= 8 && part != frame)
        .collect()
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW};

    use super::*;

    /// Lists what automatic selection would offer, frontmost first. Run by
    /// hand with `--ignored --nocapture` to see why a window is or isn't
    /// picked.
    #[test]
    #[ignore]
    fn smoke_list_windows() {
        unsafe extern "system" fn collect(window: HWND, found: LPARAM) -> BOOL {
            unsafe { (*(found.0 as *mut Vec<HWND>)).push(window) };
            TRUE
        }
        let mut handles: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(
                Some(collect),
                LPARAM(&mut handles as *mut Vec<HWND> as isize),
            );
        }
        for window in handles.into_iter().filter(|window| is_selectable(*window)) {
            let mut class = [0u16; 128];
            let mut title = [0u16; 128];
            let class_length = unsafe { GetClassNameW(window, &mut class) } as usize;
            let title_length = unsafe { GetWindowTextW(window, &mut title) } as usize;
            let exstyle = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32;
            println!(
                "{:?} ex={exstyle:#x} class={} title={}",
                visible_frame(window),
                String::from_utf16_lossy(&class[..class_length]),
                String::from_utf16_lossy(&title[..title_length]),
            );
        }
    }
}
