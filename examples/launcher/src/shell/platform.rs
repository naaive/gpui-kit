//! Platform services with a defined fallback where a platform lacks them.

mod files;
mod hud;

pub use files::{open_with, trash};

use std::{process::Command, time::Duration};

use gpui_kit::{App, SharedString};

/// How long the previous application needs to become frontmost again after
/// the launcher hides, before a keystroke sent to it lands there.
const REFOCUS_DELAY: Duration = Duration::from_millis(200);

/// Pastes the clipboard into the application that was frontmost before the
/// launcher. The text is already on the clipboard; where synthesizing the
/// paste keystroke is not possible, it stays there for the user to paste.
///
/// The keystroke comes from a helper program run off the main thread:
/// `osascript` on macOS (it needs the Accessibility permission), `xdotool` on
/// X11, `wtype` on Wayland and PowerShell's `SendKeys` on Windows.
pub fn paste_into_previous_application(cx: &mut App) {
    super::launcher::hide(cx);
    let Some(program) = paste_program(
        std::env::consts::OS,
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    ) else {
        return;
    };
    let executor = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            executor.timer(REFOCUS_DELAY).await;
            let [name, arguments @ ..] = program else {
                return;
            };
            let mut command = Command::new(name);
            command.args(arguments);
            #[cfg(target_os = "windows")]
            {
                use std::os::windows::process::CommandExt as _;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                command.creation_flags(CREATE_NO_WINDOW);
            }
            match command.status() {
                Ok(status) if status.success() => {}
                Ok(status) => tracing::warn!(
                    "`{name}` could not paste ({status}); the text is on the clipboard"
                ),
                Err(error) => tracing::info!(
                    "cannot run `{name}` to paste ({error}); the text is on the clipboard"
                ),
            }
        })
        .detach();
}

/// The command that sends the paste shortcut to the frontmost application.
fn paste_program(
    os: &str,
    session_type: Option<&str>,
    wayland_display: bool,
) -> Option<&'static [&'static str]> {
    let wayland = match session_type {
        Some(session) => session.eq_ignore_ascii_case("wayland"),
        None => wayland_display,
    };
    Some(match os {
        "macos" => &[
            "osascript",
            "-e",
            r#"tell application "System Events" to keystroke "v" using command down"#,
        ],
        "windows" => &[
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(New-Object -ComObject WScript.Shell).SendKeys('^v')",
        ],
        "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" if wayland => {
            &["wtype", "-M", "ctrl", "v", "-m", "ctrl"]
        }
        "linux" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" => {
            &["xdotool", "key", "--clearmodifiers", "ctrl+v"]
        }
        _ => return None,
    })
}

/// Shows a short message after the launcher window hides: a small pop-up
/// near the bottom of the screen that closes itself.
pub fn show_hud(text: SharedString, cx: &mut App) {
    super::launcher::hide(cx);
    cx.defer(move |cx| hud::show(text, cx));
}

/// Keeps the launcher out of the Dock and the application switcher on macOS,
/// as a utility summoned by a shortcut should be. GPUI makes every
/// application a regular one when it finishes launching; this runs after.
pub fn hide_dock_icon() {
    #[cfg(target_os = "macos")]
    {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

        if let Some(main_thread) = MainThreadMarker::new() {
            NSApplication::sharedApplication(main_thread)
                .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paste_program() {
        assert_eq!(
            paste_program("macos", None, false).map(|program| program[0]),
            Some("osascript")
        );
        assert_eq!(
            paste_program("windows", None, false).map(|program| program[0]),
            Some("powershell")
        );
        assert_eq!(
            paste_program("linux", Some("x11"), true).map(|program| program[0]),
            Some("xdotool"),
            "the session type wins over a stray WAYLAND_DISPLAY"
        );
        assert_eq!(
            paste_program("linux", Some("wayland"), false),
            Some(&["wtype", "-M", "ctrl", "v", "-m", "ctrl"][..])
        );
        assert_eq!(
            paste_program("linux", None, true).map(|program| program[0]),
            Some("wtype")
        );
        assert_eq!(paste_program("ios", None, false), None);
    }
}
