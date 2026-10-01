//! The command line: what the user asked this process to do.
//!
//! Every command except starting is a request to the running instance; the
//! process that receives it forwards it over the local socket (see
//! [`super::ipc`]) and exits. Parsing is by hand because the grammar is five
//! verbs and at most one operand, which a dependency would not make clearer.

use std::{fmt, path::PathBuf};

use super::ipc::Message;

pub const USAGE: &str = "\
Usage: launcher [COMMAND]

Commands:
  (none)             Start the launcher, or show the one already running
  toggle             Show the launcher, or hide it if it is in front
  show               Show the launcher
  hide               Hide the launcher
  open <url>         Open a deep link, such as
                     launcher://extensions/<extension-id>/<command>
  dev <directory>    Load an extension directory ahead of the installed ones,
                     writing its TypeScript declarations first
  types <directory>  Write TypeScript declarations and the launcher.json
                     schema into an extension directory
  new <directory> [--template list|detail|form|no-view|menu-bar]
                     Start an extension from a template
  lint <directory>   Check an extension's manifests, modules and images
                     without running it

Options:
  -h, --help         Print this help";

/// What this process was asked to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Start the launcher, or show it if it is already running.
    Start,
    Toggle,
    Show,
    Hide,
    /// Open a `launcher://` deep link. The URL is checked here so a typo is
    /// reported in the terminal rather than as a toast in the window.
    Open(String),
    /// Load a development extension directory.
    Dev(PathBuf),
    /// Write an extension's TypeScript declarations; needs no instance.
    Types(PathBuf),
    /// Start an extension from a template; needs no instance.
    New {
        directory: PathBuf,
        template: crate::extensions::Template,
    },
    /// Check an extension without running it; needs no instance.
    Lint(PathBuf),
    Help,
}

impl Command {
    /// The message this command sends to a running instance. Asking for help
    /// needs no instance.
    pub fn message(&self) -> Option<Message> {
        Some(match self {
            Self::Start | Self::Show => Message::Show,
            Self::Toggle => Message::Toggle,
            Self::Hide => Message::Hide,
            Self::Open(url) => Message::Open(url.clone()),
            Self::Dev(directory) => Message::Dev(directory.clone()),
            Self::Types(_) | Self::New { .. } | Self::Lint(_) | Self::Help => return None,
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
///
/// A `dev` directory is made absolute against `current_directory`, because
/// the running instance that receives it has a different working directory.
pub fn parse(
    arguments: impl IntoIterator<Item = String>,
    current_directory: &std::path::Path,
) -> Result<Command, UsageError> {
    let mut arguments = arguments.into_iter();
    let Some(verb) = arguments.next() else {
        return Ok(Command::Start);
    };
    let command = match verb.as_str() {
        "-h" | "--help" | "help" => Command::Help,
        "toggle" => Command::Toggle,
        "show" => Command::Show,
        "hide" => Command::Hide,
        "open" => {
            let url = operand(&mut arguments, "open", "a URL")?;
            super::deeplink::parse(&url).map_err(|error| UsageError(format!("{error:#}")))?;
            Command::Open(url)
        }
        "dev" => {
            let directory = PathBuf::from(operand(&mut arguments, "dev", "a directory")?);
            Command::Dev(current_directory.join(directory))
        }
        "types" => {
            let directory = PathBuf::from(operand(&mut arguments, "types", "a directory")?);
            Command::Types(current_directory.join(directory))
        }
        "lint" => {
            let directory = PathBuf::from(operand(&mut arguments, "lint", "a directory")?);
            Command::Lint(current_directory.join(directory))
        }
        "new" => {
            let directory = PathBuf::from(operand(&mut arguments, "new", "a directory")?);
            let template = match arguments.next().as_deref() {
                None => Default::default(),
                Some("--template") => {
                    let name = operand(&mut arguments, "--template", "a template")?;
                    crate::extensions::Template::parse(&name).ok_or_else(|| {
                        UsageError(format!(
                            "unknown template `{name}`; use {}",
                            crate::extensions::Template::NAMES.join(", ")
                        ))
                    })?
                }
                Some(other) => return Err(UsageError(format!("unexpected argument `{other}`"))),
            };
            Command::New {
                directory: current_directory.join(directory),
                template,
            }
        }
        other => return Err(UsageError(format!("unknown command `{other}`"))),
    };
    match arguments.next() {
        Some(extra) => Err(UsageError(format!("unexpected argument `{extra}`"))),
        None => Ok(command),
    }
}

fn operand(
    arguments: &mut impl Iterator<Item = String>,
    verb: &str,
    what: &str,
) -> Result<String, UsageError> {
    arguments
        .next()
        .filter(|operand| !operand.is_empty())
        .ok_or_else(|| UsageError(format!("`{verb}` needs {what}")))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn run(arguments: &[&str]) -> Result<Command, UsageError> {
        parse(
            arguments.iter().map(|argument| argument.to_string()),
            Path::new("/work"),
        )
    }

    #[test]
    fn test_parse_verbs() {
        assert_eq!(run(&[]), Ok(Command::Start));
        assert_eq!(run(&["toggle"]), Ok(Command::Toggle));
        assert_eq!(run(&["show"]), Ok(Command::Show));
        assert_eq!(run(&["hide"]), Ok(Command::Hide));
        assert_eq!(run(&["--help"]), Ok(Command::Help));
        assert_eq!(run(&["-h"]), Ok(Command::Help));
        assert_eq!(
            run(&["open", "launcher://extensions/com.example/hello"]),
            Ok(Command::Open(
                "launcher://extensions/com.example/hello".into()
            ))
        );
    }

    #[test]
    fn test_parse_dev_resolves_against_the_current_directory() {
        assert_eq!(
            run(&["dev", "my-extension"]),
            Ok(Command::Dev(PathBuf::from("/work/my-extension")))
        );
        assert_eq!(
            run(&["dev", "/abs/extension"]),
            Ok(Command::Dev(PathBuf::from("/abs/extension")))
        );
        assert_eq!(
            run(&["types", "ext"]),
            Ok(Command::Types(PathBuf::from("/work/ext")))
        );
        assert_eq!(
            run(&["lint", "ext"]),
            Ok(Command::Lint(PathBuf::from("/work/ext")))
        );
        assert_eq!(
            run(&["new", "ext", "--template", "menu-bar"]),
            Ok(Command::New {
                directory: PathBuf::from("/work/ext"),
                template: crate::extensions::Template::MenuBar,
            })
        );
        assert!(run(&["new", "ext", "--template", "nope"]).is_err());
    }

    #[test]
    fn test_parse_errors() {
        assert!(run(&["frobnicate"]).is_err());
        assert!(run(&["open"]).is_err(), "`open` needs a URL");
        assert!(run(&["dev", ""]).is_err(), "`dev` needs a directory");
        assert!(run(&["toggle", "now"]).is_err(), "no trailing arguments");
        assert!(
            run(&["open", "https://gpui-kit.com"]).is_err(),
            "a deep link is checked before it is forwarded"
        );
    }

    #[test]
    fn test_command_message() {
        assert_eq!(Command::Start.message(), Some(Message::Show));
        assert_eq!(Command::Toggle.message(), Some(Message::Toggle));
        assert_eq!(
            Command::Dev("/x".into()).message(),
            Some(Message::Dev("/x".into()))
        );
        assert_eq!(Command::Help.message(), None);
    }
}
