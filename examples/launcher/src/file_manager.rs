//! The files selected in the file manager window that was in front before
//! the launcher: File Explorer on Windows, the Finder on macOS.
//!
//! Only read: nothing here changes the selection or activates a window.

use std::path::PathBuf;

/// The selected files of the file manager window `window` (the frontmost one
/// on macOS, where it is not needed), or an empty list when that window is
/// not a file manager or nothing is selected. Blocking.
pub fn selected_files(window: Option<isize>) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        window
            .map(|window| explorer::selected(window).unwrap_or_default())
            .unwrap_or_default()
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        finder::selected().unwrap_or_default()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let _ = window;
        Vec::new()
    }
}

#[cfg(target_os = "windows")]
mod explorer {
    use std::path::PathBuf;

    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize, IServiceProvider,
            },
            UI::Shell::{
                IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp,
                SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_SELECTION, ShellWindows,
            },
        },
        core::{Interface as _, VARIANT},
    };

    /// Asks every Explorer window for its handle; the one that is `window`
    /// answers with its view's selection.
    pub fn selected(window: isize) -> windows::core::Result<Vec<PathBuf>> {
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let result = (|| {
                let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL)?;
                for index in 0..windows.Count()? {
                    let Ok(item) = windows.Item(&VARIANT::from(index)) else {
                        continue;
                    };
                    let Ok(browser) = item.cast::<IWebBrowserApp>() else {
                        continue;
                    };
                    if browser.HWND().map(|handle| handle.0) != Ok(window) {
                        continue;
                    }
                    let provider: IServiceProvider = browser.cast()?;
                    let shell: IShellBrowser = provider.QueryService(&SID_STopLevelBrowser)?;
                    let view: IFolderView = shell.QueryActiveShellView()?.cast()?;
                    let Ok(items) = view.Items::<IShellItemArray>(SVGIO_SELECTION) else {
                        return Ok(Vec::new());
                    };
                    let mut paths = Vec::new();
                    for ix in 0..items.GetCount()? {
                        let path = items.GetItemAt(ix)?.GetDisplayName(SIGDN_FILESYSPATH);
                        if let Ok(path) = path {
                            paths.push(PathBuf::from(path.to_string().unwrap_or_default()));
                            windows::Win32::System::Com::CoTaskMemFree(Some(path.0 as _));
                        }
                    }
                    return Ok(paths);
                }
                Ok(Vec::new())
            })();
            if initialized {
                CoUninitialize();
            }
            result
        }
    }
}

#[cfg(target_os = "macos")]
mod finder {
    use std::{path::PathBuf, process::Command};

    pub fn selected() -> Option<Vec<PathBuf>> {
        let script = r#"
            tell application "System Events" to set frontApp to name of first process whose frontmost is true
            if frontApp is not "Finder" then return ""
            tell application "Finder" to set chosen to selection as alias list
            set text item delimiters to linefeed
            set paths to {}
            repeat with each in chosen
                set end of paths to POSIX path of each
            end repeat
            return paths as text
        "#;
        let output = Command::new("osascript")
            .args(["-e", script])
            .output()
            .ok()?;
        Some(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.is_empty())
                .map(PathBuf::from)
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_window_that_is_not_a_file_manager_has_no_selection() {
        assert!(selected_files(None).is_empty() || cfg!(target_os = "macos"));
        assert!(selected_files(Some(0)).is_empty() || cfg!(target_os = "macos"));
    }
}
