//! Snippet expansion: typing a snippet's keyword in any application replaces
//! it with the snippet, as Raycast does. Opt-in, in Settings.
//!
//! On Windows a low-level keyboard hook sees each key press. Its callback
//! only forwards the key to a worker thread, which keeps the last characters
//! typed; when they end with a keyword, the launcher erases the keyword with
//! backspaces and pastes the snippet, then puts the clipboard back. Keys the
//! launcher sends itself are marked as injected and ignored, and so is typing
//! in the launcher's own window. A click, a shortcut or another window starts
//! over, since the caret may no longer follow what was typed.
//!
//! Other platforms have no expansion yet; the setting says so.

use std::sync::{Arc, Mutex};

/// The keywords and the snippets they expand to, shared with the worker.
pub type Keywords = Arc<Mutex<Vec<(String, String)>>>;

/// What the user typed most recently, as far as expansion cares.
#[derive(Debug, Default)]
pub struct TypedBuffer {
    text: String,
}

/// More than any keyword is long.
const BUFFER_CHARS: usize = 64;

impl TypedBuffer {
    pub fn push(&mut self, c: char) {
        self.text.push(c);
        let excess = self.text.chars().count().saturating_sub(BUFFER_CHARS);
        if excess > 0 {
            let cut = self.text.char_indices().nth(excess).map_or(0, |(ix, _)| ix);
            self.text.drain(..cut);
        }
    }

    pub fn backspace(&mut self) {
        self.text.pop();
    }

    /// Navigation, a click elsewhere, another window: what came before is
    /// no longer next to the caret.
    pub fn clear(&mut self) {
        self.text.clear();
    }

    /// The snippet whose keyword the text now ends with; the longest wins,
    /// so `;;email` is not taken for `;email`.
    pub fn completed<'a>(&self, keywords: &'a [(String, String)]) -> Option<&'a (String, String)> {
        keywords
            .iter()
            .filter(|(keyword, _)| !keyword.is_empty() && self.text.ends_with(keyword.as_str()))
            .max_by_key(|(keyword, _)| keyword.len())
    }
}

#[cfg(target_os = "windows")]
pub use windows::Expander;

#[cfg(not(target_os = "windows"))]
pub struct Expander;

#[cfg(not(target_os = "windows"))]
impl Expander {
    pub fn start(_: Keywords, _: smol::channel::Sender<(String, String)>) -> Option<Self> {
        None
    }
}

/// Erases `count` characters before the caret and pastes the clipboard,
/// in the application in front.
#[cfg(target_os = "windows")]
pub fn erase_and_paste(count: usize) {
    windows::erase_and_paste(count)
}

#[cfg(not(target_os = "windows"))]
pub fn erase_and_paste(_: usize) {}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        sync::{
            OnceLock,
            atomic::{AtomicU32, Ordering},
            mpsc,
        },
        thread::JoinHandle,
    };

    use ::windows::Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::Threading::{GetCurrentProcessId, GetCurrentThreadId},
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetKeyState, GetKeyboardLayout, INPUT, INPUT_0, INPUT_KEYBOARD,
                KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, ToUnicodeEx,
                VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
                VK_V,
            },
            WindowsAndMessaging::{
                CallNextHookEx, DispatchMessageW, GetForegroundWindow, GetMessageW,
                GetWindowThreadProcessId, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
                PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
                WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT,
                WM_RBUTTONDOWN, WM_SYSKEYDOWN,
            },
        },
    };

    use super::{Keywords, TypedBuffer};

    /// A key press, as the hook saw it.
    struct Press {
        vk: u32,
        scan: u32,
        shift: bool,
        control: bool,
        alt: bool,
        windows_key: bool,
        /// A mouse button, not a key: the caret may have moved.
        click: bool,
        window: isize,
    }

    /// The hook's channel to the worker; the hook callback is a plain
    /// function, so it reaches the channel through a static.
    static PRESSES: OnceLock<std::sync::Mutex<Option<mpsc::Sender<Press>>>> = OnceLock::new();

    fn presses() -> &'static std::sync::Mutex<Option<mpsc::Sender<Press>>> {
        PRESSES.get_or_init(Default::default)
    }

    /// Runs the hook while alive; dropping it removes the hook.
    pub struct Expander {
        hook_thread: AtomicU32,
        hook: Option<JoinHandle<()>>,
    }

    impl Expander {
        /// Starts watching key presses; `found` receives `(keyword, snippet
        /// id)` when a keyword is completed.
        pub fn start(
            keywords: Keywords,
            found: smol::channel::Sender<(String, String)>,
        ) -> Option<Self> {
            let (sender, receiver) = mpsc::channel::<Press>();
            *presses().lock().ok()? = Some(sender);
            std::thread::Builder::new()
                .name("snippet-expansion".into())
                .spawn(move || work(receiver, keywords, found))
                .ok()?;

            let (ready, started) = mpsc::channel();
            let hook = std::thread::Builder::new()
                .name("snippet-keyboard-hook".into())
                .spawn(move || unsafe {
                    let keyboard =
                        SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), HINSTANCE::default(), 0);
                    let mouse =
                        SetWindowsHookExW(WH_MOUSE_LL, Some(on_mouse), HINSTANCE::default(), 0);
                    ready.send(GetCurrentThreadId()).ok();
                    let Ok(keyboard) = keyboard else {
                        tracing::warn!("cannot watch the keyboard for snippet keywords");
                        if let Ok(mouse) = mouse {
                            let _ = UnhookWindowsHookEx(mouse);
                        }
                        return;
                    };
                    let mut message = MSG::default();
                    while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    let _ = UnhookWindowsHookEx(keyboard);
                    if let Ok(mouse) = mouse {
                        let _ = UnhookWindowsHookEx(mouse);
                    }
                })
                .ok()?;
            let thread = started.recv().ok()?;
            Some(Self {
                hook_thread: AtomicU32::new(thread),
                hook: Some(hook),
            })
        }
    }

    impl Drop for Expander {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(
                    self.hook_thread.load(Ordering::Relaxed),
                    WM_QUIT,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
            if let Some(hook) = self.hook.take() {
                hook.join().ok();
            }
            // Closing the channel ends the worker.
            if let Ok(mut presses) = presses().lock() {
                *presses = None;
            }
        }
    }

    fn held(key: VIRTUAL_KEY) -> bool {
        unsafe { GetAsyncKeyState(key.0 as i32) as u16 & 0x8000 != 0 }
    }

    /// The hook callback: forwards the key and returns at once, so typing
    /// is never slowed.
    unsafe extern "system" fn on_key(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
        if code >= 0 && (message.0 as u32 == WM_KEYDOWN || message.0 as u32 == WM_SYSKEYDOWN) {
            let key = unsafe { &*(data.0 as *const KBDLLHOOKSTRUCT) };
            if key.flags.0 & LLKHF_INJECTED.0 == 0
                && let Ok(presses) = presses().lock()
                && let Some(presses) = presses.as_ref()
            {
                presses
                    .send(Press {
                        vk: key.vkCode,
                        scan: key.scanCode,
                        shift: held(VK_SHIFT),
                        control: held(VK_CONTROL),
                        alt: held(VK_MENU),
                        windows_key: held(VK_LWIN) || held(VK_RWIN),
                        click: false,
                        window: unsafe { GetForegroundWindow() }.0 as isize,
                    })
                    .ok();
            }
        }
        unsafe { CallNextHookEx(HHOOK::default(), code, message, data) }
    }

    /// The mouse hook: a click may move the caret, so what was typed before
    /// it is no longer next to the caret.
    unsafe extern "system" fn on_mouse(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
        if code >= 0
            && matches!(
                message.0 as u32,
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
            )
            && let Ok(presses) = presses().lock()
            && let Some(presses) = presses.as_ref()
        {
            presses
                .send(Press {
                    vk: 0,
                    scan: 0,
                    shift: false,
                    control: false,
                    alt: false,
                    windows_key: false,
                    click: true,
                    window: 0,
                })
                .ok();
        }
        unsafe { CallNextHookEx(HHOOK::default(), code, message, data) }
    }

    /// Whether `window` is one of the launcher's own: typing a keyword in
    /// its search field finds the snippet rather than expanding it.
    fn is_own(window: isize) -> bool {
        let mut process = 0;
        unsafe {
            GetWindowThreadProcessId(HWND(window as *mut _), Some(&mut process));
            process == GetCurrentProcessId()
        }
    }

    /// The character a key types in the window's keyboard layout, if any.
    fn character(press: &Press) -> Option<char> {
        let mut state = [0u8; 256];
        if press.shift {
            state[VK_SHIFT.0 as usize] = 0x80;
        }
        // AltGr arrives as Ctrl and Alt together.
        if press.control && press.alt {
            state[VK_CONTROL.0 as usize] = 0x80;
            state[VK_MENU.0 as usize] = 0x80;
        }
        if unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0 {
            state[VK_CAPITAL.0 as usize] = 0x01;
        }
        let layout = unsafe {
            let thread = GetWindowThreadProcessId(HWND(press.window as *mut _), None);
            GetKeyboardLayout(thread)
        };
        let mut buffer = [0u16; 8];
        // Flag 4: leave the keyboard's dead-key state alone, so accents
        // typed in the application still compose.
        let written = unsafe { ToUnicodeEx(press.vk, press.scan, &state, &mut buffer, 4, layout) };
        (written == 1)
            .then(|| char::from_u32(u32::from(buffer[0])))
            .flatten()
            .filter(|c| !c.is_control())
    }

    fn work(
        presses: mpsc::Receiver<Press>,
        keywords: Keywords,
        found: smol::channel::Sender<(String, String)>,
    ) {
        let mut buffer = TypedBuffer::default();
        let mut window = 0isize;
        while let Ok(press) = presses.recv() {
            if press.click {
                buffer.clear();
                continue;
            }
            if press.window != window {
                window = press.window;
                buffer.clear();
            }
            if is_own(press.window) {
                continue;
            }
            // A shortcut (Ctrl+Backspace deletes a word, Ctrl+V pastes) does
            // something the buffer cannot follow; AltGr types a character.
            let altgr = press.control && press.alt && !press.windows_key;
            if (press.control || press.alt || press.windows_key) && !altgr {
                buffer.clear();
                continue;
            }
            if press.vk == u32::from(VK_BACK.0) {
                buffer.backspace();
                continue;
            }
            match character(&press) {
                Some(c) => buffer.push(c),
                // Modifiers alone change nothing; any other key without a
                // character (arrows, Enter, Escape) moves on.
                None if matches!(press.vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x14) => {}
                None => buffer.clear(),
            }
            let Ok(keywords) = keywords.lock() else {
                continue;
            };
            if let Some(completed) = buffer.completed(&keywords) {
                found.send_blocking(completed.clone()).ok();
                buffer.clear();
            }
        }
    }

    fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: match up {
                        true => KEYEVENTF_KEYUP,
                        false => KEYBD_EVENT_FLAGS(0),
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    pub fn erase_and_paste(count: usize) {
        let mut inputs = Vec::with_capacity(count * 2 + 4);
        for _ in 0..count {
            inputs.push(key(VK_BACK, false));
            inputs.push(key(VK_BACK, true));
        }
        inputs.extend([
            key(VK_CONTROL, false),
            key(VK_V, false),
            key(VK_V, true),
            key(VK_CONTROL, true),
        ]);
        unsafe {
            SendInput(&inputs, size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords() -> Vec<(String, String)> {
        vec![
            (";email".into(), "a".into()),
            (";;email".into(), "b".into()),
            ("!sig".into(), "c".into()),
        ]
    }

    #[test]
    fn test_buffer_completes_the_longest_keyword() {
        let mut buffer = TypedBuffer::default();
        for c in "hello ;;email".chars() {
            buffer.push(c);
        }
        assert_eq!(
            buffer.completed(&keywords()).map(|(_, id)| id.as_str()),
            Some("b")
        );

        buffer.clear();
        for c in "!sik".chars() {
            buffer.push(c);
        }
        assert_eq!(buffer.completed(&keywords()), None);
        buffer.backspace();
        buffer.push('g');
        assert_eq!(
            buffer.completed(&keywords()).map(|(_, id)| id.as_str()),
            Some("c")
        );
    }

    #[test]
    fn test_buffer_keeps_only_the_recent_characters() {
        let mut buffer = TypedBuffer::default();
        for _ in 0..200 {
            buffer.push('字');
        }
        assert_eq!(buffer.text.chars().count(), BUFFER_CHARS);
    }
}
