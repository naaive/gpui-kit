//! The text selected in the application that was in front when the launcher
//! was summoned, for the `{selection}` placeholder in quicklinks and
//! snippets.
//!
//! It is read through the accessibility API (UI Automation on Windows), so
//! nothing is typed into the application and the clipboard is left alone.
//! The control that has the keyboard focus is noted as the launcher is
//! summoned, before its window takes the focus, and its selection is read
//! on a background thread. Applications that do not expose their selection
//! give none.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

/// The selection read at the last summon, and when.
static LATEST: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// A selection older than this belongs to an earlier summon.
const FRESH: Duration = Duration::from_secs(600);

/// Notes the focused control and reads its selection in the background;
/// [`latest`] has it a moment later. Call before the launcher's window
/// takes the focus.
pub fn capture() {
    if let Ok(mut latest) = LATEST.lock() {
        *latest = None;
    }
    #[cfg(target_os = "windows")]
    {
        let Some(control) = windows::focused_control() else {
            return;
        };
        std::thread::Builder::new()
            .name("selection".into())
            .spawn(move || {
                if let Some(text) = windows::selected_text(control)
                    && let Ok(mut latest) = LATEST.lock()
                {
                    *latest = Some((Instant::now(), text));
                }
            })
            .ok();
    }
}

/// The text selected when the launcher was last summoned.
pub fn latest() -> Option<String> {
    LATEST
        .lock()
        .ok()?
        .as_ref()
        .filter(|(at, _)| at.elapsed() < FRESH)
        .map(|(_, text)| text.clone())
}

#[cfg(target_os = "windows")]
mod windows {
    use windows::{
        Win32::{
            Foundation::HWND,
            System::{
                Com::{
                    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                },
                Threading::GetCurrentProcessId,
            },
            UI::{
                Accessibility::{
                    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern,
                    TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId, UIA_TextPatternId,
                },
                WindowsAndMessaging::{
                    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
                },
            },
        },
        core::VARIANT,
    };

    /// The most text that is read; a whole document selected is not useful
    /// in a placeholder and slow to read.
    const MAX_CHARS: i32 = 10_000;

    /// The window that has the keyboard focus in the application in front,
    /// as a raw handle; `None` when the launcher itself is in front. Cheap:
    /// it asks the window manager, not the application.
    pub fn focused_control() -> Option<isize> {
        unsafe {
            let front = GetForegroundWindow();
            let mut process = 0;
            let thread = GetWindowThreadProcessId(front, Some(&mut process));
            if front.is_invalid() || process == GetCurrentProcessId() {
                return None;
            }
            let mut info = GUITHREADINFO {
                cbSize: size_of::<GUITHREADINFO>() as u32,
                ..Default::default()
            };
            let focus = match GetGUIThreadInfo(thread, &mut info) {
                Ok(()) if !info.hwndFocus.is_invalid() => info.hwndFocus,
                _ => front,
            };
            Some(focus.0 as isize)
        }
    }

    fn selection_of(element: &IUIAutomationElement) -> Option<String> {
        unsafe {
            let pattern: IUIAutomationTextPattern =
                element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            let ranges = pattern.GetSelection().ok()?;
            let mut text = String::new();
            for index in 0..ranges.Length().ok()? {
                let range = ranges.GetElement(index).ok()?;
                text.push_str(&range.GetText(MAX_CHARS).ok()?.to_string());
            }
            (!text.trim().is_empty()).then_some(text)
        }
    }

    /// The text selected in the control `window`, or in the element with
    /// the keyboard focus inside it (a field on a web page).
    pub fn selected_text(window: isize) -> Option<String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let element = automation
                .ElementFromHandle(HWND(window as *mut std::ffi::c_void))
                .ok()?;
            if let Some(text) = selection_of(&element) {
                return Some(text);
            }
            let focused = automation
                .CreatePropertyCondition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
                .ok()?;
            let inner = element.FindFirst(TreeScope_Descendants, &focused).ok()?;
            selection_of(&inner)
        }
    }
}
