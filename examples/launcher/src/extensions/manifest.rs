//! `launcher.json`: the commands an extension contributes.
//!
//! Permissions stay in `gpui-shell.json`, which rejects unknown fields and
//! belongs to the runtime rather than to this host. Keeping contributions in a
//! file of their own leaves that format untouched.

use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub const FILE: &str = "launcher.json";

/// Raycast's limit, and for the same reason: the arguments sit inline in the
/// search field, where a fourth one no longer fits beside the query.
pub const MAX_ARGUMENTS: usize = 3;

#[derive(Debug, Deserialize, JsonSchema)]
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
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq)]
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
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArgumentManifest {
    /// The key in `launch().arguments`: a letter, then letters, digits or `_`.
    pub name: String,
    pub placeholder: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, rename = "type")]
    pub input: ArgumentInput,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ArgumentInput {
    #[default]
    Text,
    Password,
}

/// A setting the user fills in once, in the launcher's settings, and the
/// extension reads through `launch().preferences`.
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreferenceManifest {
    /// The key in `launch().preferences`: a letter, then letters, digits or `_`.
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
    pub default: Option<Value>,
    /// The choices of a dropdown.
    #[serde(default)]
    pub choices: Vec<PreferenceChoice>,
    /// The label beside a checkbox.
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PreferenceInput {
    Text,
    /// Stored in the system keychain, never in the settings file.
    Password,
    Checkbox,
    Dropdown,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreferenceChoice {
    pub value: String,
    pub title: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandManifest {
    /// Lowercase letters, digits and `-`; unique within the extension.
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
    /// At most three.
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

/// The JSON Schema of `launcher.json`, for editor validation.
///
/// Generated from the types the parser reads, so the two cannot disagree.
/// The rules a schema cannot state (unique names, a dropdown default among its
/// choices) are checked by [`LauncherManifest::parse`].
pub fn schema() -> Value {
    schemars::schema_for!(LauncherManifest).to_value()
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
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        if self.commands.is_empty() {
            bail!("`commands` must list at least one command");
        }
        if let Some(icon) = &self.icon {
            non_empty("icon", icon)?;
        }
        validate_preferences("preferences", &self.preferences, &[])?;
        for (ix, command) in self.commands.iter().enumerate() {
            let at = format!("commands[{ix}]");
            if !is_valid_command_name(&command.name) {
                bail!(
                    "{at}.name `{}` must be lowercase letters, digits and `-`, not starting or \
                     ending with `-`",
                    command.name
                );
            }
            if self.commands[..ix]
                .iter()
                .any(|other| other.name == command.name)
            {
                bail!("{at}.name `{}` is declared twice", command.name);
            }
            non_empty(&format!("{at}.title"), &command.title)?;
            if let Some(subtitle) = &command.subtitle {
                non_empty(&format!("{at}.subtitle"), subtitle)?;
            }
            if let Some(icon) = &command.icon {
                non_empty(&format!("{at}.icon"), icon)?;
            }
            for (kx, keyword) in command.keywords.iter().enumerate() {
                non_empty(&format!("{at}.keywords[{kx}]"), keyword)?;
            }
            if !is_contained_path(&command.module) {
                bail!(
                    "{at}.module `{}` must be a relative path inside the extension",
                    command.module
                );
            }
            validate_arguments(&at, command)?;
            validate_preferences(
                &format!("{at}.preferences"),
                &command.preferences,
                &self.preferences,
            )?;
        }
        Ok(())
    }
}

fn validate_arguments(at: &str, command: &CommandManifest) -> Result<()> {
    if command.arguments.len() > MAX_ARGUMENTS {
        bail!(
            "{at}.arguments lists {} arguments; a command takes at most {MAX_ARGUMENTS}",
            command.arguments.len()
        );
    }
    for (ix, argument) in command.arguments.iter().enumerate() {
        let at = format!("{at}.arguments[{ix}]");
        if !is_identifier(&argument.name) {
            bail!(
                "{at}.name `{}` must start with a letter and contain only letters, digits and `_`",
                argument.name
            );
        }
        if command.arguments[..ix]
            .iter()
            .any(|other| other.name == argument.name)
        {
            bail!("{at}.name `{}` is declared twice", argument.name);
        }
        non_empty(&format!("{at}.placeholder"), &argument.placeholder)?;
    }
    if command.fallback {
        match command.arguments.first() {
            Some(first) if first.input == ArgumentInput::Text => {}
            Some(_) => bail!(
                "{at}.fallback receives the search text as its first argument, which must \
                 therefore be a `text` argument"
            ),
            None => bail!(
                "{at}.fallback receives the search text as its first argument; declare one in \
                 {at}.arguments"
            ),
        }
    }
    Ok(())
}

/// `shared` are the extension's own preferences when `preferences` are a
/// command's: both reach the command through one `launch().preferences`, so a
/// name may appear in only one of them.
fn validate_preferences(
    at: &str,
    preferences: &[PreferenceManifest],
    shared: &[PreferenceManifest],
) -> Result<()> {
    for (ix, preference) in preferences.iter().enumerate() {
        let at = format!("{at}[{ix}]");
        if !is_identifier(&preference.name) {
            bail!(
                "{at}.name `{}` must start with a letter and contain only letters, digits and `_`",
                preference.name
            );
        }
        if preferences[..ix]
            .iter()
            .any(|other| other.name == preference.name)
        {
            bail!("{at}.name `{}` is declared twice", preference.name);
        }
        if shared.iter().any(|other| other.name == preference.name) {
            bail!(
                "{at}.name `{}` is already an extension preference; a command's preferences \
                 share one namespace with the extension's",
                preference.name
            );
        }
        non_empty(&format!("{at}.title"), &preference.title)?;
        validate_preference_input(&at, preference)?;
    }
    Ok(())
}

fn validate_preference_input(at: &str, preference: &PreferenceManifest) -> Result<()> {
    let input = preference.input;
    if input != PreferenceInput::Dropdown && !preference.choices.is_empty() {
        bail!("{at}.choices is only for a `dropdown` preference");
    }
    if input != PreferenceInput::Checkbox && preference.label.is_some() {
        bail!("{at}.label is only for a `checkbox` preference");
    }
    match input {
        PreferenceInput::Text | PreferenceInput::Password => {
            if let Some(default) = &preference.default
                && !default.is_string()
            {
                bail!("{at}.default must be a string, not {default}");
            }
        }
        PreferenceInput::Checkbox => {
            match &preference.label {
                Some(label) => non_empty(&format!("{at}.label"), label)?,
                None => bail!("{at}.label is required: it is the text beside the checkbox"),
            }
            if let Some(default) = &preference.default
                && !default.is_boolean()
            {
                bail!("{at}.default must be true or false, not {default}");
            }
        }
        PreferenceInput::Dropdown => {
            if preference.choices.is_empty() {
                bail!("{at}.choices must list at least one choice");
            }
            for (cx, choice) in preference.choices.iter().enumerate() {
                non_empty(&format!("{at}.choices[{cx}].value"), &choice.value)?;
                non_empty(&format!("{at}.choices[{cx}].title"), &choice.title)?;
                if preference.choices[..cx]
                    .iter()
                    .any(|other| other.value == choice.value)
                {
                    bail!(
                        "{at}.choices[{cx}].value `{}` is declared twice",
                        choice.value
                    );
                }
            }
            match &preference.default {
                None => {}
                Some(Value::String(value))
                    if preference
                        .choices
                        .iter()
                        .any(|choice| &choice.value == value) => {}
                Some(Value::String(value)) => {
                    bail!("{at}.default `{value}` is not the value of any of its choices")
                }
                Some(other) => bail!("{at}.default must be one of its choices' values, not {other}"),
            }
        }
    }
    Ok(())
}

fn non_empty(at: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{at} must not be empty");
    }
    Ok(())
}

fn is_valid_command_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A name a script reads as a property, `launch().arguments.query`, so it is
/// kept to what needs no quoting.
fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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

    /// A manifest with one command whose own fields are `command`, and
    /// extension preferences `preferences`.
    fn manifest(command: &str, preferences: &str) -> String {
        format!(
            r#"{{
                "commands": [{{ "name": "c", "title": "C", "module": "c.js" {command} }}],
                "preferences": [{preferences}]
            }}"#
        )
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
    fn test_parses_a_complete_manifest() {
        let manifest = LauncherManifest::parse(&manifest(
            r#", "mode": "no-view", "fallback": true,
               "arguments": [{ "name": "query", "placeholder": "Query", "required": true }],
               "preferences": [{ "name": "limit", "title": "Limit", "type": "text", "default": "20" }]"#,
            r#"{ "name": "token", "title": "Token", "type": "password", "required": true },
               { "name": "unread", "title": "Unread", "type": "checkbox", "label": "Only unread",
                 "default": false },
               { "name": "sort", "title": "Sort", "type": "dropdown", "default": "new",
                 "choices": [{ "value": "new", "title": "Newest" }, { "value": "old", "title": "Oldest" }] }"#,
        ))
        .unwrap();
        let command = &manifest.commands[0];
        assert_eq!(command.mode, CommandMode::NoView);
        assert!(command.fallback);
        assert_eq!(command.arguments[0].name, "query");
        assert_eq!(manifest.preferences.len(), 3);
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
        assert!(
            error(r#"{ "commands": [{ "name": "d", "title": " ", "module": "a.js" }] }"#)
                .contains("commands[0].title must not be empty")
        );
    }

    #[test]
    fn test_validates_arguments() {
        let argument = |name: &str| format!(r#"{{ "name": "{name}", "placeholder": "P" }}"#);
        let with = |arguments: &[String]| {
            error(&manifest(
                &format!(r#", "arguments": [{}]"#, arguments.join(",")),
                "",
            ))
        };
        assert!(
            with(&[argument("a"), argument("b"), argument("c"), argument("d")])
                .contains("commands[0].arguments lists 4 arguments; a command takes at most 3")
        );
        assert!(with(&[argument("a"), argument("a")]).contains("commands[0].arguments[1].name"));
        assert!(with(&[argument("2x")]).contains("commands[0].arguments[0].name `2x`"));
        assert!(
            error(&manifest(
                r#", "arguments": [{ "name": "q", "placeholder": "" }]"#,
                ""
            ))
            .contains("commands[0].arguments[0].placeholder must not be empty")
        );
        assert!(
            error(&manifest(r#", "fallback": true"#, ""))
                .contains("commands[0].fallback receives the search text")
        );
        assert!(
            error(&manifest(
                r#", "fallback": true, "arguments": [{ "name": "q", "placeholder": "Q", "type": "password" }]"#,
                ""
            ))
            .contains("must therefore be a `text` argument")
        );
    }

    #[test]
    fn test_validates_preferences() {
        let preference = |fields: &str| error(&manifest("", &format!(r#"{{ "name": "p", "title": "P", {fields} }}"#)));
        assert!(
            preference(r#""type": "dropdown""#)
                .contains("preferences[0].choices must list at least one choice")
        );
        assert!(
            preference(
                r#""type": "dropdown", "default": "x", "choices": [{ "value": "a", "title": "A" }]"#
            )
            .contains("preferences[0].default `x` is not the value of any of its choices")
        );
        assert!(
            preference(
                r#""type": "dropdown", "choices": [{ "value": "a", "title": "A" }, { "value": "a", "title": "B" }]"#
            )
            .contains("preferences[0].choices[1].value `a` is declared twice")
        );
        assert!(
            preference(r#""type": "checkbox""#).contains("preferences[0].label is required")
        );
        assert!(
            preference(r#""type": "checkbox", "label": "L", "default": "yes""#)
                .contains("preferences[0].default must be true or false")
        );
        assert!(
            preference(r#""type": "text", "default": 3"#)
                .contains("preferences[0].default must be a string")
        );
        assert!(
            preference(r#""type": "text", "label": "L""#)
                .contains("preferences[0].label is only for a `checkbox`")
        );
        assert!(
            preference(r#""type": "text", "choices": [{ "value": "a", "title": "A" }]"#)
                .contains("preferences[0].choices is only for a `dropdown`")
        );

        // Extension and command preferences share one namespace.
        assert!(
            error(&manifest(
                r#", "preferences": [{ "name": "token", "title": "T", "type": "text" }]"#,
                r#"{ "name": "token", "title": "Token", "type": "password" }"#,
            ))
            .contains("commands[0].preferences[0].name `token` is already an extension preference")
        );
        assert!(
            error(&manifest(
                "",
                r#"{ "name": "a", "title": "A", "type": "text" }, { "name": "a", "title": "B", "type": "text" }"#,
            ))
            .contains("preferences[1].name `a` is declared twice")
        );
    }

    #[test]
    fn test_schema_describes_the_manifest() {
        let schema = schema();
        let properties = schema["properties"].as_object().expect("an object schema");
        assert!(properties.contains_key("commands"));
        assert!(properties.contains_key("preferences"));
    }
}
