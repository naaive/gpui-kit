//! Open windows on Windows: the top-level windows the taskbar and Alt+Tab
//! show, front to back.

use std::{ffi::c_void, path::PathBuf};

use windows::Win32::{
    Foundation::{BOOL, CloseHandle, HWND, LPARAM, TRUE, WPARAM},
    Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
    System::Threading::{
        GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    },
    UI::{
        Input::KeyboardAndMouse::{
            INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VK_MENU,
        },
        WindowsAndMessaging::{
            EnumChildWindows, EnumWindows, GA_ROOTOWNER, GW_OWNER, GWL_EXSTYLE, GetAncestor,
            GetLastActivePopup, GetShellWindow, GetWindow, GetWindowLongW, GetWindowTextLengthW,
            GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible,
            PostMessageW, SW_MINIMIZE, SW_RESTORE, SetForegroundWindow, ShowWindow, WM_CLOSE,
            WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        },
    },
};

use super::OpenWindow;

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

pub fn open_windows() -> Vec<OpenWindow> {
    unsafe extern "system" fn collect(window: HWND, found: LPARAM) -> BOOL {
        unsafe { (*(found.0 as *mut Vec<HWND>)).push(window) };
        TRUE
    }
    let mut found: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut Vec<HWND> as isize));
    }
    let own = unsafe { GetCurrentProcessId() };
    found
        .into_iter()
        .filter(|window| is_switchable(*window))
        .filter_map(|window| {
            let title = title(window)?;
            let pid = process_id(window);
            if pid == own {
                return None;
            }
            let mut exe = executable(pid);
            // A Store app's frame belongs to ApplicationFrameHost; the app is
            // the process of the frame's content window.
            if exe.as_ref().is_some_and(|exe| {
                exe.file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("ApplicationFrameHost.exe"))
            }) && let Some(content) = content_process(window, pid)
            {
                exe = executable(content).or(exe);
            }
            Some(OpenWindow {
                handle: window.0 as isize,
                title,
                exe,
                minimized: unsafe { IsIconic(window) }.as_bool(),
            })
        })
        .collect()
}

/// Whether Alt+Tab would show the window: visible, not cloaked (on another
/// virtual desktop, or a suspended Store app), not a tool window, and the
/// last active popup of its owner chain.
fn is_switchable(window: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(window).as_bool() || window == GetShellWindow() {
            return false;
        }
        let mut cloaked: u32 = 0;
        if DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut c_void,
            size_of::<u32>() as u32,
        )
        .is_ok()
            && cloaked != 0
        {
            return false;
        }
        let style = GetWindowLongW(window, GWL_EXSTYLE) as u32;
        if style & WS_EX_APPWINDOW.0 != 0 {
            return true;
        }
        if style & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
            return false;
        }
        if GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.is_invalid()) {
            return false;
        }
        let root = GetAncestor(window, GA_ROOTOWNER);
        GetLastActivePopup(root) == window || root == window
    }
}

fn title(window: HWND) -> Option<String> {
    unsafe {
        let length = GetWindowTextLengthW(window);
        if length <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = GetWindowTextW(window, &mut buffer);
        let title = String::from_utf16_lossy(&buffer[..copied.max(0) as usize]);
        let title = title.trim().to_owned();
        (!title.is_empty()).then_some(title)
    }
}

fn process_id(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

pub fn foreground() -> isize {
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize
}

pub fn window_executable(handle: isize) -> Option<PathBuf> {
    executable(process_id(hwnd(handle)))
}

/// The full path of the process `pid`'s executable.
pub fn executable(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buffer[..length as usize],
        )))
    }
}

/// The process of a child window that belongs to another process than
/// `frame_pid`: a Store app's content inside its frame.
fn content_process(window: HWND, frame_pid: u32) -> Option<u32> {
    struct Search {
        frame_pid: u32,
        found: Option<u32>,
    }
    unsafe extern "system" fn visit(child: HWND, search: LPARAM) -> BOOL {
        let search = unsafe { &mut *(search.0 as *mut Search) };
        let pid = process_id(child);
        if pid != search.frame_pid {
            search.found = Some(pid);
            return BOOL(0);
        }
        TRUE
    }
    let mut search = Search {
        frame_pid,
        found: None,
    };
    unsafe {
        let _ = EnumChildWindows(
            window,
            Some(visit),
            LPARAM(&mut search as *mut Search as isize),
        );
    }
    search.found
}

pub fn focus(handle: isize) -> bool {
    let window = hwnd(handle);
    unsafe {
        if !IsWindow(window).as_bool() {
            return false;
        }
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        if SetForegroundWindow(window).as_bool() {
            return true;
        }
        // Windows refuses the foreground to a process that did not receive
        // the last input; a tapped Alt counts as input and lifts the lock.
        let tap = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_MENU,
                    dwFlags: flags,
                    ..Default::default()
                },
            },
        };
        SendInput(
            &[tap(Default::default()), tap(KEYEVENTF_KEYUP)],
            size_of::<INPUT>() as i32,
        );
        SetForegroundWindow(window).as_bool()
    }
}

pub fn minimize(handle: isize) -> bool {
    let window = hwnd(handle);
    unsafe {
        // `ShowWindow` returns whether the window was visible, not success.
        let _ = ShowWindow(window, SW_MINIMIZE);
        IsIconic(window).as_bool()
    }
}

/// Asks the window to close, as its close button would; it may ask to save.
pub fn close(handle: isize) -> bool {
    let window = hwnd(handle);
    unsafe {
        IsWindow(window).as_bool() && PostMessageW(window, WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
    }
}
