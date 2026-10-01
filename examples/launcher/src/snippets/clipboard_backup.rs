//! Saving the whole clipboard while a snippet borrows it, and putting it
//! back: every format the copying application offered (rich text, HTML,
//! images, files), not only plain text, and an empty clipboard stays empty.
//!
//! Windows only; elsewhere the launcher keeps what GPUI can read.

/// What was on the clipboard.
pub struct ClipboardBackup {
    #[cfg(target_os = "windows")]
    formats: Vec<(u32, Vec<u8>)>,
}

impl ClipboardBackup {
    /// Copies every format on the clipboard; `None` when it cannot be read.
    pub fn take() -> Option<Self> {
        #[cfg(target_os = "windows")]
        {
            windows::take().map(|formats| Self { formats })
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    /// Puts the saved formats back, replacing what is there now.
    pub fn restore(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            windows::restore(&self.formats)
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::time::Duration;

    use ::windows::Win32::{
        Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
                OpenClipboard, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        },
    };

    /// Formats whose data is not a block of memory (bitmaps, metafiles and
    /// palettes are GDI objects; owner-display and private formats are the
    /// owner's own handles). Their content also comes as a memory format,
    /// such as `CF_DIB` for a bitmap, which is kept.
    fn is_memory_format(format: u32) -> bool {
        !matches!(format, 2 | 3 | 9 | 14 | 0x80..=0x8E | 0x200..=0x3FF)
    }

    /// Opens the clipboard, retrying while another application holds it.
    fn open() -> bool {
        for _ in 0..10 {
            if unsafe { OpenClipboard(HWND::default()) }.is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub fn take() -> Option<Vec<(u32, Vec<u8>)>> {
        if !open() {
            return None;
        }
        let mut formats = Vec::new();
        let mut format = 0;
        loop {
            format = unsafe { EnumClipboardFormats(format) };
            if format == 0 {
                break;
            }
            if !is_memory_format(format) {
                continue;
            }
            let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
                continue;
            };
            let memory = HGLOBAL(handle.0);
            unsafe {
                let size = GlobalSize(memory);
                let data = GlobalLock(memory);
                if data.is_null() {
                    continue;
                }
                let bytes = std::slice::from_raw_parts(data as *const u8, size).to_vec();
                let _ = GlobalUnlock(memory);
                formats.push((format, bytes));
            }
        }
        unsafe {
            let _ = CloseClipboard();
        }
        Some(formats)
    }

    pub fn restore(formats: &[(u32, Vec<u8>)]) -> bool {
        if !open() {
            return false;
        }
        unsafe {
            let _ = EmptyClipboard();
            for (format, bytes) in formats {
                let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
                    continue;
                };
                let data = GlobalLock(memory);
                if data.is_null() {
                    let _ = GlobalFree(memory);
                    continue;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len());
                let _ = GlobalUnlock(memory);
                // On success the clipboard owns the memory; otherwise it is
                // still ours to free.
                if SetClipboardData(*format, HANDLE(memory.0)).is_err() {
                    let _ = GlobalFree(memory);
                }
            }
            let _ = CloseClipboard();
        }
        true
    }
}
