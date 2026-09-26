//! The summon shortcut: a system-wide key combination that shows the launcher
//! while another application is in front.
//!
//! GPUI key bindings only fire while the application has focus, so the
//! shortcut is registered with the operating system through `global-hotkey`
//! (macOS, Windows, X11). Wayland has no such facility for ordinary clients;
//! there the user binds `launcher toggle` in the desktop's keyboard settings.

use std::fmt;

use anyhow::{Result, anyhow, bail};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui_kit::{Keystroke, SharedString};

pub const DEFAULT_SHORTCUT: &str = "alt-space";

/// Converts a GPUI keystroke, as settings store it (`alt-space`,
/// `secondary-shift-k`), into a hotkey the operating system can register.
///
/// A shortcut needs a modifier unless its key is a function key: a bare
/// letter would stop that letter from reaching every other application.
pub fn parse_shortcut(shortcut: &str) -> Result<HotKey> {
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        bail!("Enter a shortcut, such as {DEFAULT_SHORTCUT}.");
    }
    if shortcut.contains(' ') {
        bail!("Use one key combination, not a sequence.");
    }
    let keystroke = Keystroke::parse(shortcut)
        .map_err(|_| anyhow!("“{shortcut}” isn't a shortcut. Use a form such as alt-space."))?;
    let modifiers = &keystroke.modifiers;
    if modifiers.function {
        bail!("The fn key can't be part of a system-wide shortcut.");
    }
    let mods = [
        (modifiers.control, Modifiers::CONTROL),
        (modifiers.alt, Modifiers::ALT),
        (modifiers.shift, Modifiers::SHIFT),
        (modifiers.platform, Modifiers::SUPER),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(Modifiers::empty(), |mods, (_, modifier)| mods | modifier);

    let key = key_code(&keystroke.key).ok_or_else(|| {
        anyhow!(
            "“{}” can't be used in a system-wide shortcut.",
            keystroke.key
        )
    })?;
    if mods.is_empty() && !is_function_key(&keystroke.key) {
        bail!("Add a modifier, such as alt or ctrl.");
    }
    Ok(HotKey::new(Some(mods), key))
}

/// Maps a GPUI key name onto the physical key `global-hotkey` registers.
/// Both name keys the same way (`space`, `pageup`, `f13`, `a`), ignoring case.
fn key_code(key: &str) -> Option<Code> {
    key.parse::<HotKey>().ok().map(|hotkey| hotkey.key)
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// Whether this session can register system-wide shortcuts at all, judged
/// from `XDG_SESSION_TYPE` and `WAYLAND_DISPLAY`.
pub fn is_supported_by_session(session_type: Option<&str>, wayland_display: bool) -> bool {
    if !cfg!(target_os = "linux") {
        return true;
    }
    match session_type {
        Some(session) => !session.eq_ignore_ascii_case("wayland"),
        None => !wayland_display,
    }
}

fn session_supports_hotkeys() -> bool {
    is_supported_by_session(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    )
}

/// What became of the summon shortcut; shown in settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HotkeyStatus {
    Registered,
    /// The session offers no global shortcuts (Wayland).
    Unavailable,
    /// Registration was attempted and refused, usually because another
    /// application holds the same shortcut.
    Failed(SharedString),
}

impl fmt::Display for HotkeyStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registered => formatter.write_str("Works in every application."),
            Self::Unavailable => formatter.write_str(
                "Wayland doesn't let applications register system-wide shortcuts. \
                 Bind `launcher toggle` in your desktop's keyboard settings instead.",
            ),
            Self::Failed(reason) => write!(formatter, "Couldn't register the shortcut: {reason}"),
        }
    }
}

/// Owns the registration of the summon shortcut.
///
/// The manager must live on the main thread: macOS delivers hotkeys through
/// the main run loop, and Windows through the message loop of the thread that
/// created it, which in GPUI is the main thread.
pub struct SummonHotkey {
    manager: Option<GlobalHotKeyManager>,
    registered: Option<HotKey>,
    status: HotkeyStatus,
}

impl SummonHotkey {
    /// Starts listening for hotkeys; `on_pressed` runs on a platform thread
    /// with the id of the hotkey that was pressed.
    pub fn new(on_pressed: impl Fn(u32) + Send + Sync + 'static) -> Self {
        if !session_supports_hotkeys() {
            tracing::info!(
                "global shortcuts are unavailable on Wayland; bind `launcher toggle` in the desktop's keyboard settings"
            );
            return Self {
                manager: None,
                registered: None,
                status: HotkeyStatus::Unavailable,
            };
        }
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() == HotKeyState::Pressed {
                on_pressed(event.id());
            }
        }));
        match GlobalHotKeyManager::new() {
            Ok(manager) => Self {
                manager: Some(manager),
                registered: None,
                status: HotkeyStatus::Failed("no shortcut is set".into()),
            },
            Err(error) => {
                tracing::error!("cannot listen for global shortcuts: {error}");
                Self {
                    manager: None,
                    registered: None,
                    status: HotkeyStatus::Failed(error.to_string().into()),
                }
            }
        }
    }

    /// Replaces the registered shortcut. A failure leaves no shortcut
    /// registered and is kept as the status rather than returned: the
    /// launcher stays usable through `launcher toggle`.
    pub fn register(&mut self, shortcut: &str) -> &HotkeyStatus {
        let Some(manager) = &self.manager else {
            return &self.status;
        };
        if let Some(previous) = self.registered.take() {
            manager.unregister(previous).ok();
        }
        self.status = match parse_shortcut(shortcut) {
            Ok(hotkey) => match manager.register(hotkey) {
                Ok(()) => {
                    self.registered = Some(hotkey);
                    HotkeyStatus::Registered
                }
                Err(error) => HotkeyStatus::Failed(error.to_string().into()),
            },
            Err(error) => HotkeyStatus::Failed(error.to_string().into()),
        };
        if let HotkeyStatus::Failed(reason) = &self.status {
            tracing::warn!("cannot register the summon shortcut `{shortcut}`: {reason}");
        }
        &self.status
    }

    /// Whether `id`, from a hotkey event, is the summon shortcut.
    pub fn is_summon(&self, id: u32) -> bool {
        self.registered.is_some_and(|hotkey| hotkey.id() == id)
    }

    pub fn status(&self) -> &HotkeyStatus {
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortcut() {
        assert_eq!(
            parse_shortcut("alt-space").unwrap(),
            HotKey::new(Some(Modifiers::ALT), Code::Space)
        );
        assert_eq!(
            parse_shortcut("ctrl-shift-k").unwrap(),
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyK)
        );
        assert_eq!(
            parse_shortcut("cmd-alt-1").unwrap(),
            HotKey::new(Some(Modifiers::SUPER | Modifiers::ALT), Code::Digit1)
        );
        assert_eq!(
            parse_shortcut("ctrl-pageup").unwrap(),
            HotKey::new(Some(Modifiers::CONTROL), Code::PageUp)
        );
        assert_eq!(
            parse_shortcut("f13").unwrap(),
            HotKey::new(None, Code::F13),
            "a function key needs no modifier"
        );
    }

    #[test]
    fn test_parse_shortcut_rejects() {
        for shortcut in ["", "   ", "space", "a", "alt-", "ctrl-a ctrl-b", "fn-f1"] {
            assert!(parse_shortcut(shortcut).is_err(), "{shortcut:?}");
        }
    }

    #[test]
    fn test_is_supported_by_session() {
        let linux = cfg!(target_os = "linux");
        assert!(is_supported_by_session(Some("x11"), false));
        assert_eq!(is_supported_by_session(Some("wayland"), true), !linux);
        assert_eq!(is_supported_by_session(None, true), !linux);
        assert!(is_supported_by_session(None, false));
    }
}
