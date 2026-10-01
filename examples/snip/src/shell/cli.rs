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
                  --delay SECONDS waits first, to open a menu or hover
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
    /// Capture after this many seconds.
    CaptureAfter(u32),
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
            Self::CaptureAfter(seconds) => Message::CaptureAfter { seconds },
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

/// The longest delay `capture --delay` accepts, in seconds.
const MAX_DELAY: u32 = 60;

/// Parses the arguments after the program name.
pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    let mut arguments = arguments.into_iter().peekable();
    let command = match arguments.next().as_deref() {
        None => Command::Start,
        Some("-h" | "--help" | "help") => Command::Help,
        Some("capture") if arguments.peek().is_some_and(|word| word == "--delay") => {
            arguments.next();
            let seconds = arguments
                .next()
                .and_then(|word| word.parse::<u32>().ok())
                .filter(|seconds| *seconds <= MAX_DELAY)
                .ok_or_else(|| {
                    UsageError(format!("--delay takes whole seconds, up to {MAX_DELAY}"))
                })?;
            match seconds {
                0 => Command::Capture,
                seconds => Command::CaptureAfter(seconds),
            }
        }
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
        assert_eq!(
            parse_words(&["capture", "--delay", "3"]),
            Ok(Command::CaptureAfter(3))
        );
        assert_eq!(
            parse_words(&["capture", "--delay", "0"]),
            Ok(Command::Capture)
        );
        assert!(parse_words(&["capture", "--delay"]).is_err());
        assert!(parse_words(&["capture", "--delay", "soon"]).is_err());
        assert!(parse_words(&["capture", "--delay", "600"]).is_err());
    }

    #[test]
    fn test_message() {
        assert_eq!(Command::Capture.message(), Some(Message::Capture));
        assert_eq!(Command::Help.message(), None);
    }
}
