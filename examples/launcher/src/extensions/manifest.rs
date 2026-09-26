//! `launcher.json`: the commands an extension contributes.
//!
//! Permissions stay in `gpui-shell.json`, which rejects unknown fields and
//! belongs to the runtime rather than to this host. Keeping contributions in a
//! file of their own leaves that format untouched.

use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

pub const FILE: &str = "launcher.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LauncherManifest {
    /// A Lucide icon name, or a path inside the extension, shown beside the
    /// extension's commands.
    #[serde(default)]
    pub icon: Option<String>,
    pub commands: Vec<CommandManifest>,
    /// Settings shared by every command of the extension.
    #[serde(default)]
    pub preferences: Vec<PreferenceManifest>,
}

/// How a command runs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CommandMode {
    /// Pushes the page its View renders.
    #[default]
    View,
    /// Runs without a page: the View's `init` does the work and reports back
    /// with a HUD or toast; its `render` is never shown.
    NoView,
}

/// Something the user types before a command runs, such as a search query.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgumentManifest {
    pub name: String,
    pub placeholder: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, rename = "type")]
    pub input: ArgumentInput,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArgumentInput {
    #[default]
    Text,
    Password,
}

/// A setting the user fills in once, in the launcher's settings, and the
/// extension reads through `launch().preferences`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceManifest {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub input: PreferenceInput,
    #[serde(default)]
    pub required: bool,
    /// A string, or a boolean for a checkbox.
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    /// The choices of a dropdown.
    #[serde(default)]
    pub choices: Vec<PreferenceChoice>,
    /// The label beside a checkbox.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PreferenceInput {
    Text,
    /// Stored in the system keychain, never in the settings file.
    Password,
    Checkbox,
    Dropdown,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreferenceChoice {
    pub value: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandManifest {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// The module whose default export is the command's View, relative to the
    /// extension directory.
    pub module: String,
    #[serde(default)]
    pub mode: CommandMode,
    #[serde(default)]
    pub arguments: Vec<ArgumentManifest>,
    /// Offered at the bottom of the root search, with the query as its first
    /// argument, when nothing else matches.
    #[serde(default)]
    pub fallback: bool,
    /// Settings of this command only.
    #[serde(default)]
    pub preferences: Vec<PreferenceManifest>,
}

impl LauncherManifest {
    /// Reads and validates `launcher.json` in `directory`.
    ///
    /// Every error names the offending field, because nothing has run yet and
    /// this message is all an author gets.
    pub fn read(directory: &Path) -> Result<Self> {
        let path = directory.join(FILE);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let manifest =
            Self::parse(&source).with_context(|| format!("invalid {}", path.display()))?;
        for (ix, command) in manifest.commands.iter().enumerate() {
            let module = directory.join(&command.module);
            if !module.is_file() {
                bail!(
                    "commands[{ix}].module `{}` does not exist in {}",
                    command.module,
                    directory.display()
                );
            }
        }
        Ok(manifest)
    }

    pub fn parse(source: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(source)?;
        if manifest.commands.is_empty() {
            bail!("`commands` must list at least one command");
        }
        for (ix, command) in manifest.commands.iter().enumerate() {
            if !is_valid_name(&command.name) {
                bail!(
                    "commands[{ix}].name `{}` must be lowercase letters, digits and `-`, \
                     not starting or ending with `-`",
                    command.name
                );
            }
            if manifest.commands[..ix]
                .iter()
                .any(|other| other.name == command.name)
            {
                bail!("commands[{ix}].name `{}` is declared twice", command.name);
            }
            if command.title.trim().is_empty() {
                bail!("commands[{ix}].title must not be empty");
            }
            if !is_contained_path(&command.module) {
                bail!(
                    "commands[{ix}].module `{}` must be a relative path inside the extension",
                    command.module
                );
            }
        }
        Ok(manifest)
    }
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The same rule GPUI Shell applies to `entry` and to every `import`: the
/// module cannot leave the extension directory.
fn is_contained_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(source: &str) -> String {
        format!("{:#}", LauncherManifest::parse(source).unwrap_err())
    }

    #[test]
    fn test_parses_a_minimal_manifest() {
        let manifest = LauncherManifest::parse(
            r#"{ "commands": [{ "name": "docs", "title": "Docs", "module": "commands/docs.js" }] }"#,
        )
        .unwrap();
        assert_eq!(manifest.commands[0].name, "docs");
        assert!(manifest.commands[0].keywords.is_empty());
    }

    #[test]
    fn test_reports_the_offending_field() {
        assert!(error(r#"{ "commandz": [] }"#).contains("commandz"));
        assert!(error(r#"{ "commands": [] }"#).contains("at least one"));
        assert!(
            error(r#"{ "commands": [{ "name": "Docs", "title": "D", "module": "a.js" }] }"#)
                .contains("commands[0].name")
        );
        assert!(
            error(r#"{ "commands": [{ "name": "d", "title": "D", "module": "../a.js" }] }"#)
                .contains("commands[0].module")
        );
        assert!(
            error(r#"{ "commands": [{ "name": "d", "title": "D", "module": "/a.js" }] }"#)
                .contains("commands[0].module")
        );
        assert!(
            error(
                r#"{ "commands": [
                    { "name": "d", "title": "D", "module": "a.js" },
                    { "name": "d", "title": "E", "module": "b.js" }
                ] }"#
            )
            .contains("declared twice")
        );
    }
}
