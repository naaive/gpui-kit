//! What the platform says about the clipboard without reading it.

/// A number that changes whenever the clipboard does; `None` where the
/// platform has none, and every poll then reads the clipboard.
#[cfg(target_os = "windows")]
pub fn change_count() -> Option<u32> {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    Some(unsafe { GetClipboardSequenceNumber() })
}

#[cfg(not(target_os = "windows"))]
pub fn change_count() -> Option<u32> {
    None
}

/// Whether the application that copied asked clipboard managers to leave the
/// content alone, as password managers do.
#[cfg(target_os = "windows")]
pub fn is_private() -> bool {
    use windows::{
        Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW},
        core::w,
    };
    unsafe {
        [
            w!("ExcludeClipboardContentFromMonitorProcessing"),
            w!("Clipboard Viewer Ignore"),
        ]
        .into_iter()
        .any(|name| {
            let format = RegisterClipboardFormatW(name);
            format != 0 && IsClipboardFormatAvailable(format).is_ok()
        })
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_private() -> bool {
    false
}
