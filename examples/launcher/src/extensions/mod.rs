//! Extensions: discovering them, and running their commands on GPUI Shell.
//!
//! An extension is a directory with two manifests. `gpui-shell.json` belongs to
//! the runtime and says what the code may do; `launcher.json` belongs to the
//! launcher and says which commands exist. Discovery reads both and runs
//! nothing, so every command is searchable before any VM starts.
//!
//! A command's module default-exports a GPUI Shell `View` whose `render`
//! returns a `List`. The `bridge` registers `List` and its parts as components
//! that draw nothing and carry a [`PageModel`](crate::model::PageModel) instead,
//! so the launcher renders every page itself.

mod bridge;
mod catalog;
mod host;
mod manifest;

pub use bridge::ScriptModel;
pub use catalog::{Catalog, Extension, ExtensionCommand};
pub use host::ExtensionHost;

use std::fmt;

use gpui_kit::SharedString;

/// `<extension id>/<command name>`: a command's identity across the launcher.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CommandId {
    extension: SharedString,
    command: SharedString,
}

impl CommandId {
    pub fn new(extension: impl Into<SharedString>, command: impl Into<SharedString>) -> Self {
        Self {
            extension: extension.into(),
            command: command.into(),
        }
    }

    pub fn extension(&self) -> &SharedString {
        &self.extension
    }

    pub fn command(&self) -> &SharedString {
        &self.command
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.extension, self.command)
    }
}
