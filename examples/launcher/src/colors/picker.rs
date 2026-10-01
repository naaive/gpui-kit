//! The on-screen color picker on Windows: a magnifier window that follows
//! the pointer, and low-level hooks that take the click meant for the
//! application below.
//!
//! Everything runs on one thread with its own message loop: the hooks'
//! callbacks, the magnifier's window procedure and its timer, so their state
//! is a thread-local. The click that picks, and its release, are swallowed;
//! so are Esc, Enter and the arrow keys, which cancel, pick and nudge the
//! pointer by a pixel (ten with Shift).

use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, BitBlt, COLORONCOLOR, CreateCompatibleBitmap, CreateCompatibleDC,
            CreatePen, CreateSolidBrush, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC,
            DeleteObject, DrawTextW, EndPaint, FillRect, GetDC, GetMonitorInfoW, GetPixel,
            GetStockObject, HGDIOBJ, InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO,
            MonitorFromPoint, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, Rectangle, ReleaseDC, SRCCOPY,
            SelectObject, SetBkMode, SetStretchBltMode, SetTextColor, StretchBlt, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{
                GetKeyState, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE,
                VK_UP,
            },
            WindowsAndMessaging::{
                CS_HREDRAW, CS_VREDRAW, CallNextHookEx, CreateWindowExW, DefWindowProcW,
                DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, HHOOK, HWND_TOPMOST,
                KBDLLHOOKSTRUCT, LWA_ALPHA, MSG, MSLLHOOKSTRUCT, PostQuitMessage, RegisterClassW,
                SWP_NOACTIVATE, SWP_SHOWWINDOW, SetCursorPos, SetLayeredWindowAttributes, SetTimer,
                SetWindowPos, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
                WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_PAINT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSW,
                WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
    core::{PCWSTR, w},
};

use super::Color;

/// Pixels on each side of the one under the pointer.
const RADIUS: i32 = 6;
/// How large each magnified pixel is drawn.
const ZOOM: i32 = 10;
const LENS: i32 = (2 * RADIUS + 1) * ZOOM;
const LABEL: i32 = 26;
/// The magnifier's distance from the pointer.
const OFFSET: i32 = 24;
/// Picking gives up on its own after this long.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Only one picker at a time.
static PICKING: AtomicBool = AtomicBool::new(false);

struct State {
    started: Instant,
    /// The answer, once the user picked or cancelled; `Some(None)` cancels.
    result: Option<Option<Color>>,
    /// The button whose release is still to be swallowed.
    pending_release: Option<u32>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|state| state.borrow_mut().as_mut().map(f))
}

/// Starts picking; the receiver gets the color, or `None` when cancelled.
pub fn pick() -> smol::channel::Receiver<Option<Color>> {
    let (sender, receiver) = smol::channel::bounded(1);
    if PICKING.swap(true, Ordering::SeqCst) {
        return receiver;
    }
    let started = std::thread::Builder::new()
        .name("color-picker".into())
        .spawn(move || {
            // Let the launcher's window go first, so it is not what is picked.
            std::thread::sleep(Duration::from_millis(150));
            let color = unsafe { run() };
            PICKING.store(false, Ordering::SeqCst);
            sender.try_send(color).ok();
        });
    if started.is_err() {
        PICKING.store(false, Ordering::SeqCst);
    }
    receiver
}

/// The color of the screen pixel at `point`.
fn color_at(point: POINT) -> Option<Color> {
    unsafe {
        let screen = GetDC(HWND::default());
        let pixel = GetPixel(screen, point.x, point.y);
        ReleaseDC(HWND::default(), screen);
        // CLR_INVALID: outside every display.
        (pixel.0 != 0xFFFF_FFFF).then(|| {
            Color::rgb(
                (pixel.0 & 0xFF) as u8,
                ((pixel.0 >> 8) & 0xFF) as u8,
                ((pixel.0 >> 16) & 0xFF) as u8,
            )
        })
    }
}

fn cursor() -> POINT {
    let mut point = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut point);
    }
    point
}

fn finish(result: Option<Color>) {
    with_state(|state| {
        if state.result.is_none() {
            state.result = Some(result);
        }
    });
}

unsafe fn run() -> Option<Color> {
    unsafe {
        let instance = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class = w!("LauncherColorPicker");
        let definition = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_procedure),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // Fails harmlessly when a previous pick registered it.
        RegisterClassW(&definition);
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class,
            w!("Color Picker"),
            WS_POPUP,
            0,
            0,
            LENS,
            LENS + LABEL,
            HWND::default(),
            None,
            instance,
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), 255, LWA_ALPHA);
        STATE.with(|state| {
            *state.borrow_mut() = Some(State {
                started: Instant::now(),
                result: None,
                pending_release: None,
            })
        });
        place(window);
        SetTimer(window, 1, 16, None);

        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), HINSTANCE::default(), 0).ok();
        let keyboard =
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), HINSTANCE::default(), 0).ok();
        let mut message = MSG::default();
        while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        for hook in [mouse, keyboard].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(hook);
        }
        let _ = DestroyWindow(window);
        STATE
            .with(|state| state.borrow_mut().take())
            .and_then(|state| state.result.flatten())
    }
}

/// Moves the magnifier beside the pointer, on the side with room.
fn place(window: HWND) {
    let point = cursor();
    let (width, height) = (LENS, LENS + LABEL);
    let mut x = point.x + OFFSET;
    let mut y = point.y + OFFSET;
    unsafe {
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let area = info.rcMonitor;
            if x + width > area.right {
                x = point.x - OFFSET - width;
            }
            if y + height > area.bottom {
                y = point.y - OFFSET - height;
            }
        }
        let _ = SetWindowPos(
            window,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match message {
            WM_TIMER => {
                let done = with_state(|state| {
                    if state.started.elapsed() > TIMEOUT {
                        state.result.get_or_insert(None);
                    }
                    state.result.is_some() && state.pending_release.is_none()
                })
                .unwrap_or(true);
                if done {
                    PostQuitMessage(0);
                } else {
                    place(window);
                    let _ = InvalidateRect(window, None, false);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                paint(window);
                LRESULT(0)
            }
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// Draws the magnified pixels around the pointer, the one under it framed,
/// and its value below.
unsafe fn paint(window: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let target = BeginPaint(window, &mut paint);
        let point = cursor();
        let (width, height) = (LENS, LENS + LABEL);
        let memory = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, width, height);
        let previous = SelectObject(memory, bitmap);

        let background = CreateSolidBrush(COLORREF(0x0020_2020));
        FillRect(
            memory,
            &RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            },
            background,
        );
        let _ = DeleteObject(background);

        let screen = GetDC(HWND::default());
        SetStretchBltMode(memory, COLORONCOLOR);
        let _ = StretchBlt(
            memory,
            0,
            0,
            LENS,
            LENS,
            screen,
            point.x - RADIUS,
            point.y - RADIUS,
            2 * RADIUS + 1,
            2 * RADIUS + 1,
            SRCCOPY,
        );
        ReleaseDC(HWND::default(), screen);

        // The pixel under the pointer, framed in white with a black edge so
        // the frame shows on any color.
        let center = RADIUS * ZOOM;
        let hollow = SelectObject(memory, GetStockObject(NULL_BRUSH));
        for (inset, color) in [(0, 0x0000_0000), (1, 0x00FF_FFFF)] {
            let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
            let old = SelectObject(memory, HGDIOBJ(pen.0));
            let _ = Rectangle(
                memory,
                center - 1 + inset,
                center - 1 + inset,
                center + ZOOM + 1 - inset,
                center + ZOOM + 1 - inset,
            );
            SelectObject(memory, old);
            let _ = DeleteObject(pen);
        }
        SelectObject(memory, hollow);

        if let Some(color) = color_at(point) {
            SetBkMode(memory, TRANSPARENT);
            SetTextColor(memory, COLORREF(0x00FF_FFFF));
            let mut text: Vec<u16> = color.format(super::Format::Hex).encode_utf16().collect();
            let mut label = RECT {
                left: 0,
                top: LENS,
                right: width,
                bottom: height,
            };
            DrawTextW(
                memory,
                &mut text,
                &mut label,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }

        let _ = BitBlt(target, 0, 0, width, height, memory, 0, 0, SRCCOPY);
        SelectObject(memory, previous);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(memory);
        let _ = EndPaint(window, &paint);
    }
}

unsafe extern "system" fn on_mouse(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let event = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let message = wparam.0 as u32;
            let swallow = match message {
                WM_LBUTTONDOWN => {
                    finish(color_at(event.pt));
                    with_state(|state| state.pending_release = Some(WM_LBUTTONUP));
                    true
                }
                WM_RBUTTONDOWN => {
                    finish(None);
                    with_state(|state| state.pending_release = Some(WM_RBUTTONUP));
                    true
                }
                WM_LBUTTONUP | WM_RBUTTONUP => with_state(|state| {
                    let expected = state.pending_release == Some(message);
                    if expected {
                        state.pending_release = None;
                    }
                    expected
                })
                .unwrap_or(false),
                _ => false,
            };
            if swallow {
                return LRESULT(1);
            }
        }
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
    }
}

unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let key = event.vkCode as u16;
            let handled = [
                VK_ESCAPE, VK_RETURN, VK_SPACE, VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN,
            ]
            .iter()
            .any(|known| known.0 == key);
            let message = wparam.0 as u32;
            if handled && (message == WM_KEYDOWN || message == WM_SYSKEYDOWN) {
                let point = cursor();
                let step = match GetKeyState(VK_SHIFT.0 as i32) < 0 {
                    true => 10,
                    false => 1,
                };
                match key {
                    k if k == VK_ESCAPE.0 => finish(None),
                    k if k == VK_RETURN.0 || k == VK_SPACE.0 => finish(color_at(point)),
                    k if k == VK_LEFT.0 => {
                        let _ = SetCursorPos(point.x - step, point.y);
                    }
                    k if k == VK_RIGHT.0 => {
                        let _ = SetCursorPos(point.x + step, point.y);
                    }
                    k if k == VK_UP.0 => {
                        let _ = SetCursorPos(point.x, point.y - step);
                    }
                    _ => {
                        let _ = SetCursorPos(point.x, point.y + step);
                    }
                }
                return LRESULT(1);
            }
            if handled && message == WM_KEYUP {
                return LRESULT(1);
            }
        }
        CallNextHookEx(HHOOK::default(), code, wparam, lparam)
    }
}
