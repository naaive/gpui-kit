//! Window Management on Windows, through the Win32 window and monitor APIs.
//!
//! A window's visible frame is its DWM extended frame bounds: Windows 10 and
//! later draw an invisible resize border outside it, which `SetWindowPos`
//! includes, so frames are converted between the two to leave no gaps.

use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicIsize, Ordering},
    },
};

use gpui_kit::SharedString;
use windows::Win32::{
    Foundation::{BOOL, HWND, LPARAM, RECT, TRUE},
    Graphics::{
        Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{
            EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
            MONITORINFO, MonitorFromWindow,
        },
    },
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindow, IsZoomed,
        SW_MINIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
        ShowWindow,
    },
};

use super::{Frame, Layout};

/// The window in front before the launcher, as a raw handle.
static FRONTMOST: AtomicIsize = AtomicIsize::new(0);

/// Frames windows had before a command moved them, for Restore.
fn previous_frames() -> &'static Mutex<HashMap<isize, Frame>> {
    static FRAMES: OnceLock<Mutex<HashMap<isize, Frame>>> = OnceLock::new();
    FRAMES.get_or_init(Default::default)
}

pub fn remember_frontmost() {
    unsafe {
        let window = GetForegroundWindow();
        let mut process = 0;
        GetWindowThreadProcessId(window, Some(&mut process));
        // Summoned while already in front: keep the window from before.
        if !window.is_invalid() && process != GetCurrentProcessId() {
            FRONTMOST.store(window.0 as isize, Ordering::Relaxed);
        }
    }
}

pub fn apply(layout: Layout) -> Result<(), SharedString> {
    let raw = FRONTMOST.load(Ordering::Relaxed);
    let window = HWND(raw as *mut c_void);
    if raw == 0 || !unsafe { IsWindow(window) }.as_bool() {
        return Err("No window to move".into());
    }
    unsafe {
        if layout == Layout::Minimize {
            let _ = ShowWindow(window, SW_MINIMIZE);
            return Ok(());
        }
        // A maximized or minimized window ignores a new frame until it is
        // an ordinary window again.
        if IsZoomed(window).as_bool() || IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
    }
    let current = visible_frame(window).ok_or("Couldn’t read the window’s frame")?;
    let target = match layout {
        Layout::Restore => previous_frames()
            .lock()
            .ok()
            .and_then(|frames| frames.get(&raw).copied())
            .ok_or("Nothing to restore")?,
        Layout::NextDisplay | Layout::PreviousDisplay => {
            let step = match layout {
                Layout::NextDisplay => 1,
                _ => -1,
            };
            other_display(window, current, step).ok_or("There is no other display")?
        }
        _ => layout
            .frame(
                current,
                work_area(window).ok_or("Couldn’t read the display")?,
            )
            .ok_or("Unsupported layout")?,
    };
    if layout != Layout::Restore
        && let Ok(mut frames) = previous_frames().lock()
    {
        frames.insert(raw, current);
    }
    set_visible_frame(window, target)?;
    unsafe {
        let _ = SetForegroundWindow(window);
    }
    Ok(())
}

fn frame(rect: RECT) -> Frame {
    Frame::new(
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    )
}

fn visible_frame(window: HWND) -> Option<Frame> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .or_else(|_| GetWindowRect(window, &mut rect))
        .ok()?;
    }
    Some(frame(rect))
}

/// Sets the window's visible frame, adding back the invisible border.
fn set_visible_frame(window: HWND, target: Frame) -> Result<(), SharedString> {
    let mut outer = RECT::default();
    unsafe { GetWindowRect(window, &mut outer) }
        .map_err(|_| SharedString::from("Couldn’t read the window’s frame"))?;
    let visible = visible_frame(window).unwrap_or(frame(outer));
    let left = visible.x - outer.left;
    let top = visible.y - outer.top;
    let right = outer.right - (visible.x + visible.width);
    let bottom = outer.bottom - (visible.y + visible.height);
    unsafe {
        SetWindowPos(
            window,
            HWND::default(),
            target.x - left,
            target.y - top,
            target.width + left + right,
            target.height + top + bottom,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
    }
    .map_err(|error| SharedString::from(format!("Couldn’t move the window: {error}")))
}

fn monitor_area(monitor: HMONITOR) -> Option<Frame> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then(|| frame(info.rcWork))
}

/// The usable area of the display the window is mostly on.
fn work_area(window: HWND) -> Option<Frame> {
    monitor_area(unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) })
}

/// Every display, left to right, then top to bottom.
fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        found: LPARAM,
    ) -> BOOL {
        unsafe { (*(found.0 as *mut Vec<HMONITOR>)).push(monitor) };
        TRUE
    }
    let mut found: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(collect),
            LPARAM(&mut found as *mut Vec<HMONITOR> as isize),
        );
    }
    found.sort_by_key(|monitor| monitor_area(*monitor).map_or((0, 0), |area| (area.x, area.y)));
    found
}

/// The window's frame moved to the display `step` places away, at the same
/// relative position and size.
fn other_display(window: HWND, current: Frame, step: isize) -> Option<Frame> {
    let monitors = monitors();
    if monitors.len() < 2 {
        return None;
    }
    let here = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let index = monitors.iter().position(|monitor| *monitor == here)? as isize;
    let next = monitors[(index + step).rem_euclid(monitors.len() as isize) as usize];
    let (from, to) = (monitor_area(here)?, monitor_area(next)?);
    let scale_x = to.width as f32 / from.width as f32;
    let scale_y = to.height as f32 / from.height as f32;
    let width = ((current.width as f32 * scale_x) as i32).min(to.width);
    let height = ((current.height as f32 * scale_y) as i32).min(to.height);
    let x = to.x + ((current.x - from.x) as f32 * scale_x) as i32;
    let y = to.y + ((current.y - from.y) as f32 * scale_y) as i32;
    Some(Frame::new(
        x.clamp(to.x, to.x + to.width - width),
        y.clamp(to.y, to.y + to.height - height),
        width,
        height,
    ))
}
