//! The command line: what the user asked this process to do.
//!
//! Every command is a request to the running instance. The process that
//! receives it forwards it over the local socket (see [`super::ipc`]) and
//! exits, or, when none is running, becomes the instance and does it. That
//! makes `snip capture` the shortcut to bind where the system offers no
//! global hotkeys to applications, as on Wayland.

use std::fmt;

use super::ipc::Message;

pub const USAGE: &str = "\
Usage: snip [COMMAND]

Commands:
  (none)          Start Snip in the background, with its tray icon
  capture         Freeze the screen and select an area
  pin-clipboard   Pin the image or text on the clipboard to the screen
  settings        Open the settings window
  quit            Quit the running instance

Options:
  -h, --help      Print this help";

/// What this process was asked to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Start,
    Capture,
    PinClipboard,
    Settings,
    Quit,
    Help,
}

impl Command {
    /// The message this command sends to a running instance.
    pub fn message(self) -> Option<Message> {
        Some(match self {
            Self::Start => Message::Start,
            Self::Capture => Message::Capture,
            Self::PinClipboard => Message::PinClipboard,
            Self::Settings => Message::Settings,
            Self::Quit => Message::Quit,
            Self::Help => return None,
        })
    }
}

/// Why the arguments could not be understood; printed above [`USAGE`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageError(String);

impl fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

/// Parses the arguments after the program name.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    let mut arguments = arguments.into_iter();
    let command = match arguments.next().as_deref() {
        None => Command::Start,
        Some("-h" | "--help" | "help") => Command::Help,
        Some("capture") => Command::Capture,
        Some("pin-clipboard") => Command::PinClipboard,
        Some("settings") => Command::Settings,
        Some("quit") => Command::Quit,
        Some(other) => return Err(UsageError(format!("unknown command “{other}”"))),
    };
    if let Some(extra) = arguments.next() {
        return Err(UsageError(format!("unexpected argument “{extra}”")));
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_words(words: &[&str]) -> Result<Command, UsageError> {
        parse(words.iter().map(|word| word.to_string()))
    }

    #[test]
    fn test_parse() {
        assert_eq!(parse_words(&[]), Ok(Command::Start));
        assert_eq!(parse_words(&["capture"]), Ok(Command::Capture));
        assert_eq!(parse_words(&["pin-clipboard"]), Ok(Command::PinClipboard));
        assert_eq!(parse_words(&["--help"]), Ok(Command::Help));
        assert!(parse_words(&["shoot"]).is_err());
        assert!(parse_words(&["capture", "now"]).is_err());
    }

    #[test]
    fn test_message() {
        assert_eq!(Command::Capture.message(), Some(Message::Capture));
        assert_eq!(Command::Help.message(), None);
    }
}
