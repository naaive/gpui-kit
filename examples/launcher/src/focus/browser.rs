//! The address a browser window shows, read through UI Automation: the
//! first edit field of a Chromium or Firefox window is its address bar.

use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        },
        UI::Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
            UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
        },
    },
    core::VARIANT,
};

/// Browsers by executable name.
pub const BROWSERS: [&str; 8] = [
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "arc",
    "360chrome",
];

pub fn is_browser(application: &str) -> bool {
    BROWSERS
        .iter()
        .any(|browser| browser.eq_ignore_ascii_case(application))
}

/// The text in the window's address bar; `None` when it has none or the
/// browser does not expose it.
pub fn address(window: isize) -> Option<String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let root = automation
            .ElementFromHandle(HWND(window as *mut std::ffi::c_void))
            .ok()?;
        let condition = automation
            .CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_EditControlTypeId.0),
            )
            .ok()?;
        let edit = root.FindFirst(TreeScope_Descendants, &condition).ok()?;
        let value: IUIAutomationValuePattern = edit.GetCurrentPatternAs(UIA_ValuePatternId).ok()?;
        let text = value.CurrentValue().ok()?.to_string();
        (!text.trim().is_empty()).then_some(text)
    }
}
