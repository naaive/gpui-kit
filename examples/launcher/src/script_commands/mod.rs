//! Script Commands: scripts in a folder that become commands, described by
//! comments in Raycast's format, so scripts written for Raycast run here:
//!
//! ```text
//! #!/bin/bash
//! # @raycast.schemaVersion 1
//! # @raycast.title Say Hello
//! # @raycast.mode fullOutput
//! # @raycast.icon 👋
//! # @raycast.argument1 { "type": "text", "placeholder": "Name" }
//! echo "Hello, $1"
//! ```
//!
//! `@launcher.` works in place of `@raycast.`. The mode decides how output
//! shows: `fullOutput` on a page, `compact` as a toast, `silent` as a HUD
//! after the launcher hides (`inline` is treated as `compact`).

mod page;

pub use page::{create_script_command, script_items};

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// How a script's output is shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mode {
    #[default]
    FullOutput,
    Compact,
    Silent,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScriptArgument {
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub optional: bool,
    /// `text` or `password`; `dropdown` is read as text.
    #[serde(default, rename = "type")]
    pub kind: String,
}

/// A script and what its comments say about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptCommand {
    pub path: PathBuf,
    pub title: String,
    pub mode: Mode,
    pub icon: Option<String>,
    pub package: Option<String>,
    pub description: Option<String>,
    pub arguments: Vec<ScriptArgument>,
    pub current_directory: Option<PathBuf>,
}

/// The folder script commands are read from.
pub fn directory() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("script-commands"))
}

/// Every script command in `directory` (not its subfolders, like Raycast),
/// sorted by title. Files without a title comment are not commands.
pub fn discover(directory: &Path) -> Vec<ScriptCommand> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut commands: Vec<ScriptCommand> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let source = std::fs::read_to_string(&path).ok()?;
            parse(&path, &source)
        })
        .collect();
    commands.sort_by_cached_key(|command| command.title.to_lowercase());
    commands
}

/// Reads the metadata comments of a script; `None` without a title.
pub fn parse(path: &Path, source: &str) -> Option<ScriptCommand> {
    let mut command = ScriptCommand {
        path: path.to_path_buf(),
        title: String::new(),
        mode: Mode::default(),
        icon: None,
        package: None,
        description: None,
        arguments: Vec::new(),
        current_directory: None,
    };
    // Metadata sits in the leading comments; stop well before the body.
    for line in source.lines().take(60) {
        let line = line.trim();
        let Some(rest) = ["@raycast.", "@launcher."]
            .iter()
            .find_map(|marker| line.split_once(marker).map(|(_, rest)| rest))
        else {
            continue;
        };
        let (key, value) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let value = value.trim().to_owned();
        match key {
            "title" => command.title = value,
            "mode" => {
                command.mode = match value.as_str() {
                    "silent" => Mode::Silent,
                    "compact" | "inline" => Mode::Compact,
                    _ => Mode::FullOutput,
                }
            }
            "icon" if !value.is_empty() => command.icon = Some(value),
            "packageName" if !value.is_empty() => command.package = Some(value),
            "description" if !value.is_empty() => command.description = Some(value),
            "currentDirectoryPath" if !value.is_empty() => {
                command.current_directory = Some(expand_home(&value))
            }
            key if key.starts_with("argument") => {
                if let Ok(argument) = serde_json::from_str::<ScriptArgument>(&value) {
                    command.arguments.push(argument);
                }
            }
            _ => {}
        }
    }
    (!command.title.is_empty()).then_some(command)
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest.trim_start_matches(['/', '\\'])))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

/// The program that runs `script`, and its arguments before the script's
/// path: from the extension, else from a `#!` line.
pub fn interpreter(script: &Path, source_first_line: &str) -> Vec<String> {
    let extension = script
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let by_extension: &[&str] = match extension.as_str() {
        "sh" | "bash" => &["bash"],
        "zsh" => &["zsh"],
        "py" => &[if cfg!(windows) { "python" } else { "python3" }],
        "js" | "mjs" => &["node"],
        "ts" => &["npx", "tsx"],
        "rb" => &["ruby"],
        "pl" => &["perl"],
        "php" => &["php"],
        "swift" => &["swift"],
        "ps1" => &[
            "powershell",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ],
        "bat" | "cmd" => &["cmd", "/C"],
        _ => &[],
    };
    if !by_extension.is_empty() {
        return by_extension
            .iter()
            .map(|part| match *part {
                "bash" => bash(),
                part => part.to_owned(),
            })
            .collect();
    }
    match source_first_line.strip_prefix("#!") {
        // `#!/usr/bin/env python3` → `python3`; `#!/bin/bash` → `bash`.
        Some(shebang) => {
            let mut parts = shebang.split_whitespace();
            let program = parts.next().unwrap_or_default();
            let program = match program.rsplit('/').next() {
                Some("env") => parts.next().unwrap_or_default().to_owned(),
                Some(name) => name.to_owned(),
                None => program.to_owned(),
            };
            std::iter::once(program)
                .chain(parts.map(str::to_owned))
                .collect()
        }
        None => Vec::new(),
    }
}

/// The shell for `.sh` scripts. On Windows that is Git Bash: a plain `bash`
/// finds WSL's in System32 first, which cannot read Windows paths.
fn bash() -> String {
    #[cfg(target_os = "windows")]
    {
        let candidates = [
            std::env::var_os("ProgramFiles")
                .map(|dir| PathBuf::from(dir).join(r"Git\bin\bash.exe")),
            std::env::var_os("ProgramFiles(x86)")
                .map(|dir| PathBuf::from(dir).join(r"Git\bin\bash.exe")),
            std::env::var_os("LOCALAPPDATA")
                .map(|dir| PathBuf::from(dir).join(r"Programs\Git\bin\bash.exe")),
        ];
        if let Some(found) = candidates.into_iter().flatten().find(|path| path.is_file()) {
            return found.display().to_string();
        }
    }
    "bash".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_raycast_metadata() {
        let source = r#"#!/bin/bash

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Say Hello
# @raycast.mode compact

# Optional parameters:
# @raycast.icon 👋
# @raycast.packageName Greetings
# @raycast.argument1 { "type": "text", "placeholder": "Name", "optional": true }
# @raycast.argument2 { "type": "password", "placeholder": "Secret" }

echo "Hello, $1"
"#;
        let command = parse(Path::new("/s/hello.sh"), source).unwrap();
        assert_eq!(command.title, "Say Hello");
        assert_eq!(command.mode, Mode::Compact);
        assert_eq!(command.icon.as_deref(), Some("👋"));
        assert_eq!(command.package.as_deref(), Some("Greetings"));
        assert_eq!(command.arguments.len(), 2);
        assert!(command.arguments[0].optional);
        assert_eq!(command.arguments[1].kind, "password");
        assert_eq!(parse(Path::new("x.sh"), "echo no metadata"), None);
    }

    #[test]
    fn test_interpreter_from_extension_or_shebang() {
        assert!(interpreter(Path::new("a.sh"), "")[0].contains("bash"));
        assert_eq!(
            interpreter(Path::new("a"), "#!/usr/bin/env python3 -u"),
            ["python3", "-u"]
        );
        assert_eq!(interpreter(Path::new("a"), "#!/bin/zsh"), ["zsh"]);
        assert!(interpreter(Path::new("a"), "echo").is_empty());
    }
}
