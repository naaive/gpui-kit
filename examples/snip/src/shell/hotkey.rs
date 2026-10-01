//! System-wide shortcuts: capture and pin work while another application is
//! in front.
//!
//! GPUI key bindings only fire while the application has focus, so the
//! shortcuts are registered with the operating system through
//! `global-hotkey` (macOS, Windows, X11). Wayland has no such facility for
//! ordinary clients; there the user binds `snip capture` in the desktop's
//! keyboard settings.

use std::fmt;

use anyhow::{Result, anyhow, bail};
use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
    hotkey::{Code, HotKey, Modifiers},
};
use gpui_kit::{Keystroke, SharedString};

/// Converts a GPUI keystroke, as settings store it (`f1`, `ctrl-shift-a`),
/// into a hotkey the operating system can register.
///
/// A shortcut needs a modifier unless its key is a function key: a bare
/// letter would stop that letter from reaching every other application.
pub fn parse_shortcut(shortcut: &str) -> Result<HotKey> {
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        bail!("Enter a shortcut, such as f1 or ctrl-shift-a.");
    }
    if shortcut.contains(' ') {
        bail!("Use one key combination, not a sequence.");
    }
    let keystroke = Keystroke::parse(shortcut)
        .map_err(|_| anyhow!("“{shortcut}” isn't a shortcut. Use a form such as ctrl-shift-a."))?;
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
fn key_code(key: &str) -> Option<Code> {
    key.parse::<HotKey>().ok().map(|hotkey| hotkey.key)
}

fn is_function_key(key: &str) -> bool {
    key.strip_prefix('f')
        .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
}

/// Whether this session can register system-wide shortcuts at all.
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

/// What a system-wide shortcut does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotkeyCommand {
    Capture,
    PinClipboard,
}

/// What became of a shortcut; shown in settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HotkeyStatus {
    Registered,
    /// The session offers no global shortcuts (Wayland).
    Unavailable,
    /// Registration was refused, usually because another application
    /// holds the same shortcut.
    Failed(SharedString),
}

impl fmt::Display for HotkeyStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registered => formatter.write_str("Works in every application."),
            Self::Unavailable => formatter.write_str(
                "Wayland doesn't let applications register system-wide shortcuts. \
                 Bind `snip capture` in your desktop's keyboard settings instead.",
            ),
            Self::Failed(reason) => write!(formatter, "Couldn’t register the shortcut: {reason}"),
        }
    }
}

/// Owns the registration of the capture and pin shortcuts.
///
/// The manager must live on the main thread: macOS delivers hotkeys through
/// the main run loop, and Windows through the message loop of the thread
/// that created it, which in GPUI is the main thread.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<(HotkeyCommand, HotKey)>,
    statuses: Vec<(HotkeyCommand, HotkeyStatus)>,
}

impl Hotkeys {
    /// Starts listening; `on_pressed` runs on a platform thread with the id
    /// of the hotkey that was pressed.
    pub fn new(on_pressed: impl Fn(u32) + Send + Sync + 'static) -> Self {
        if !session_supports_hotkeys() {
            tracing::info!("global shortcuts are unavailable on Wayland");
            return Self {
                manager: None,
                registered: Vec::new(),
                statuses: Vec::new(),
            };
        }
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state() == HotKeyState::Pressed {
                on_pressed(event.id());
            }
        }));
        let manager = GlobalHotKeyManager::new()
            .inspect_err(|error| tracing::error!("cannot listen for global shortcuts: {error}"))
            .ok();
        Self {
            manager,
            registered: Vec::new(),
            statuses: Vec::new(),
        }
    }

    /// Replaces the shortcut of `command`. A failure leaves none registered
    /// for it and is kept as its status: the command stays reachable from
    /// the tray and `snip capture`.
    pub fn register(&mut self, command: HotkeyCommand, shortcut: &str) {
        let status = match &self.manager {
            None => HotkeyStatus::Unavailable,
            Some(manager) => {
                if let Some(ix) = self
                    .registered
                    .iter()
                    .position(|(known, _)| *known == command)
                {
                    let (_, previous) = self.registered.remove(ix);
                    manager.unregister(previous).ok();
                }
                match parse_shortcut(shortcut).and_then(|hotkey| {
                    if self.registered.iter().any(|(_, known)| *known == hotkey) {
                        bail!("Another Snip command uses this shortcut.");
                    }
                    manager.register(hotkey)?;
                    Ok(hotkey)
                }) {
                    Ok(hotkey) => {
                        self.registered.push((command, hotkey));
                        HotkeyStatus::Registered
                    }
                    Err(error) => {
                        tracing::warn!("cannot register `{shortcut}`: {error:#}");
                        HotkeyStatus::Failed(error.to_string().into())
                    }
                }
            }
        };
        self.statuses.retain(|(known, _)| *known != command);
        self.statuses.push((command, status));
    }

    /// The command whose hotkey has `id`.
    pub fn command(&self, id: u32) -> Option<HotkeyCommand> {
        self.registered
            .iter()
            .find(|(_, hotkey)| hotkey.id() == id)
            .map(|(command, _)| *command)
    }

    pub fn status(&self, command: HotkeyCommand) -> HotkeyStatus {
        self.statuses
            .iter()
            .find(|(known, _)| *known == command)
            .map(|(_, status)| status.clone())
            .unwrap_or(HotkeyStatus::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortcut() {
        assert_eq!(parse_shortcut("f1").unwrap(), HotKey::new(None, Code::F1));
        assert_eq!(
            parse_shortcut("ctrl-shift-a").unwrap(),
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyA)
        );
        assert_eq!(
            parse_shortcut("cmd-alt-1").unwrap(),
            HotKey::new(Some(Modifiers::SUPER | Modifiers::ALT), Code::Digit1)
        );
    }

    #[test]
    fn test_parse_shortcut_rejects() {
        for shortcut in ["", "   ", "a", "alt-", "ctrl-a ctrl-b", "fn-f1"] {
            assert!(parse_shortcut(shortcut).is_err(), "{shortcut:?}");
        }
    }

    #[test]
    fn test_is_supported_by_session() {
        let linux = cfg!(target_os = "linux");
        assert!(is_supported_by_session(Some("x11"), false));
        assert_eq!(is_supported_by_session(Some("wayland"), true), !linux);
        assert!(is_supported_by_session(None, false));
    }
}
