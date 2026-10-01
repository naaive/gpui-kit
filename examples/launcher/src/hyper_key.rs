//! Hyper Key: Caps Lock held down acts as Ctrl+Shift+Alt+Win at once, a
//! modifier no application uses, for shortcuts of one's own (`hyper-k`).
//! A quick press alone can send Esc instead, or toggle Caps Lock as usual.
//!
//! Windows only, through a low-level keyboard hook that swallows Caps Lock
//! and presses the four modifiers in its place. Releasing Win alone opens
//! the Start menu, and the four modifiers alone open the Office app, so a
//! key no keyboard has is tapped before they are released.

use serde::{Deserialize, Serialize};

/// What Caps Lock does.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HyperKey {
    /// Caps Lock is Caps Lock.
    #[default]
    Off,
    /// Held: Hyper. Pressed alone: Caps Lock.
    CapsLock,
    /// Held: Hyper. Pressed alone: Esc.
    CapsLockEscape,
    /// Held: Hyper. Pressed alone: nothing.
    CapsLockOnly,
}

impl HyperKey {
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::CapsLock,
        Self::CapsLockEscape,
        Self::CapsLockOnly,
    ];

    pub fn value(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::CapsLock => "caps_lock",
            Self::CapsLockEscape => "caps_lock_escape",
            Self::CapsLockOnly => "caps_lock_only",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::CapsLock => "Caps Lock (a quick press toggles Caps Lock)",
            Self::CapsLockEscape => "Caps Lock (a quick press is Esc)",
            Self::CapsLockOnly => "Caps Lock (a quick press does nothing)",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|key| key.value() == value)
    }
}

/// Starts or stops the hook to match `mode`.
pub fn set(mode: HyperKey) {
    #[cfg(target_os = "windows")]
    windows::set(mode);
    #[cfg(not(target_os = "windows"))]
    let _ = mode;
}

pub fn is_supported() -> bool {
    cfg!(target_os = "windows")
}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicU8, AtomicU32, Ordering},
        },
        time::{Duration, Instant},
    };

    use ::windows::Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::KeyboardAndMouse::{
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
                SendInput, VIRTUAL_KEY, VK_CAPITAL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT,
                VK_LWIN,
            },
            WindowsAndMessaging::{
                CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT,
                LLKHF_INJECTED, MSG, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
                UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN,
                WM_SYSKEYUP,
            },
        },
    };

    use super::HyperKey;

    /// A press shorter than this, with no other key, is a tap.
    const TAP: Duration = Duration::from_millis(250);
    /// No keyboard has this key; tapping it keeps Win from opening Start.
    const MASK_KEY: VIRTUAL_KEY = VIRTUAL_KEY(0xE8);
    const MODIFIERS: [VIRTUAL_KEY; 4] = [VK_LCONTROL, VK_LSHIFT, VK_LMENU, VK_LWIN];

    /// The hook thread, so it can be stopped.
    static THREAD: AtomicU32 = AtomicU32::new(0);
    static MODE: AtomicU8 = AtomicU8::new(0);
    /// When Caps Lock went down, and whether another key was used since.
    static HELD: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

    pub fn set(mode: HyperKey) {
        MODE.store(
            HyperKey::ALL
                .iter()
                .position(|known| *known == mode)
                .unwrap_or(0) as u8,
            Ordering::SeqCst,
        );
        let running = THREAD.load(Ordering::SeqCst);
        match (mode, running) {
            (HyperKey::Off, 0) => {}
            (HyperKey::Off, thread) => unsafe {
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
                THREAD.store(0, Ordering::SeqCst);
            },
            (_, 0) => start(),
            // Running: the mode is read on each key.
            _ => {}
        }
    }

    fn mode() -> HyperKey {
        HyperKey::ALL[MODE.load(Ordering::SeqCst) as usize % HyperKey::ALL.len()]
    }

    fn start() {
        let (ready, started) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("hyper-key".into())
            .spawn(move || unsafe {
                let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(on_key), HINSTANCE::default(), 0);
                // 0 says the hook failed, so a later `set` tries again.
                ready
                    .send(match hook {
                        Ok(_) => GetCurrentThreadId(),
                        Err(_) => 0,
                    })
                    .ok();
                let Ok(hook) = hook else {
                    tracing::warn!("cannot watch the keyboard for the Hyper Key");
                    return;
                };
                let mut message = MSG::default();
                while GetMessageW(&mut message, HWND::default(), 0, 0).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let _ = UnhookWindowsHookEx(hook);
                release_if_held();
            });
        if spawned.is_ok()
            && let Ok(thread) = started.recv()
        {
            THREAD.store(thread, Ordering::SeqCst);
        }
    }

    fn key(key: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    dwFlags: match up {
                        true => KEYEVENTF_KEYUP,
                        false => KEYBD_EVENT_FLAGS(0),
                    },
                    ..Default::default()
                },
            },
        }
    }

    fn send(inputs: &[INPUT]) {
        unsafe {
            SendInput(inputs, size_of::<INPUT>() as i32);
        }
    }

    fn press_modifiers() {
        let inputs: Vec<INPUT> = MODIFIERS
            .iter()
            .map(|modifier| key(*modifier, false))
            .collect();
        send(&inputs);
    }

    fn release_modifiers() {
        let mut inputs = vec![key(MASK_KEY, false), key(MASK_KEY, true)];
        inputs.extend(MODIFIERS.iter().rev().map(|modifier| key(*modifier, true)));
        send(&inputs);
    }

    fn release_if_held() {
        if let Ok(mut held) = HELD.lock()
            && held.take().is_some()
        {
            release_modifiers();
        }
    }

    unsafe extern "system" fn on_key(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            if code < 0 {
                return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
            }
            let event = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let injected = event.flags.0 & LLKHF_INJECTED.0 != 0;
            let message = wparam.0 as u32;
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            let up = message == WM_KEYUP || message == WM_SYSKEYUP;
            let mode = mode();
            if mode == HyperKey::Off || injected {
                return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
            }
            if event.vkCode == u32::from(VK_CAPITAL.0) {
                let Ok(mut held) = HELD.lock() else {
                    return CallNextHookEx(HHOOK::default(), code, wparam, lparam);
                };
                if down {
                    // Auto-repeat while held changes nothing.
                    if held.is_none() {
                        *held = Some((Instant::now(), false));
                        press_modifiers();
                    }
                } else if up && let Some((since, used)) = held.take() {
                    release_modifiers();
                    if !used && since.elapsed() < TAP {
                        match mode {
                            HyperKey::CapsLockEscape => {
                                send(&[key(VK_ESCAPE, false), key(VK_ESCAPE, true)])
                            }
                            HyperKey::CapsLock => {
                                send(&[key(VK_CAPITAL, false), key(VK_CAPITAL, true)])
                            }
                            HyperKey::CapsLockOnly | HyperKey::Off => {}
                        }
                    }
                }
                // Caps Lock itself never reaches the application.
                return LRESULT(1);
            }
            if down
                && let Ok(mut held) = HELD.lock()
                && let Some((_, used)) = held.as_mut()
            {
                *used = true;
            }
            CallNextHookEx(HHOOK::default(), code, wparam, lparam)
        }
    }
}
