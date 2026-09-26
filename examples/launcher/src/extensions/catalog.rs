use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui_kit::SharedString;
use gpui_shell::plugin::PluginManifest;

use super::{
    CommandId,
    manifest::{ArgumentManifest, CommandMode, LauncherManifest, PreferenceManifest},
};

/// Every installed extension, read from disk without running any of them.
#[derive(Default)]
pub struct Catalog {
    extensions: Vec<Extension>,
}

impl Catalog {
    /// Reads every extension directory under `roots`, in order. An earlier root
    /// wins a duplicate id, so a user's copy can shadow a bundled one.
    ///
    /// A broken extension is logged and skipped; it never hides the others.
    pub fn discover(roots: &[PathBuf]) -> Self {
        let mut extensions: Vec<Extension> = Vec::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(root) else {
                continue;
            };
            let mut directories: Vec<PathBuf> = entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.is_dir())
                .collect();
            directories.sort();
            for directory in directories {
                match Extension::read(&directory) {
                    Ok(extension) if extensions.iter().any(|e| e.id == extension.id) => {
                        tracing::info!(
                            "extension `{}` in {} is shadowed by an earlier one",
                            extension.id,
                            directory.display()
                        );
                    }
                    Ok(extension) => extensions.push(extension),
                    Err(error) => tracing::warn!("skipping extension: {error:#}"),
                }
            }
        }
        Self { extensions }
    }

    pub fn commands(&self) -> impl Iterator<Item = (&Extension, &ExtensionCommand)> {
        self.extensions.iter().flat_map(|extension| {
            extension
                .commands
                .iter()
                .map(move |command| (extension, command))
        })
    }

    pub fn command(&self, id: &CommandId) -> Option<(&Extension, &ExtensionCommand)> {
        self.commands().find(|(_, command)| command.id == *id)
    }
}

pub struct Extension {
    id: SharedString,
    preferences: Vec<PreferenceManifest>,
    name: SharedString,
    root: PathBuf,
    commands: Vec<ExtensionCommand>,
}

impl Extension {
    fn read(directory: &Path) -> Result<Self> {
        let shell = PluginManifest::read(directory)
            .with_context(|| format!("invalid extension in {}", directory.display()))?;
        let launcher = LauncherManifest::read(directory)?;
        let id: SharedString = shell.id().to_owned().into();
        let commands = launcher
            .commands
            .into_iter()
            .map(|command| ExtensionCommand {
                id: CommandId::new(id.clone(), command.name),
                title: command.title.into(),
                subtitle: command.subtitle.map(Into::into),
                icon: command
                    .icon
                    .or_else(|| launcher.icon.clone())
                    .map(Into::into),
                keywords: command.keywords.into_iter().map(Into::into).collect(),
                module: command.module,
                mode: command.mode,
                arguments: command.arguments,
                fallback: command.fallback,
                preferences: command.preferences,
            })
            .collect();
        Ok(Self {
            id,
            preferences: launcher.preferences,
            name: shell.name().to_owned().into(),
            root: directory.to_path_buf(),
            commands,
        })
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Settings shared by every command of the extension.
    pub fn preferences(&self) -> &[PreferenceManifest] {
        &self.preferences
    }
}

pub struct ExtensionCommand {
    id: CommandId,
    mode: CommandMode,
    arguments: Vec<ArgumentManifest>,
    fallback: bool,
    preferences: Vec<PreferenceManifest>,
    title: SharedString,
    subtitle: Option<SharedString>,
    icon: Option<SharedString>,
    keywords: Vec<SharedString>,
    module: String,
}

impl ExtensionCommand {
    pub fn id(&self) -> &CommandId {
        &self.id
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn subtitle(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    pub fn icon(&self) -> Option<&SharedString> {
        self.icon.as_ref()
    }

    pub fn keywords(&self) -> &[SharedString] {
        &self.keywords
    }

    /// The command's module, relative to its extension's root.
    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn mode(&self) -> CommandMode {
        self.mode
    }

    pub fn arguments(&self) -> &[ArgumentManifest] {
        &self.arguments
    }

    pub fn is_fallback(&self) -> bool {
        self.fallback
    }

    /// Settings of this command only; see also [`Extension::preferences`].
    pub fn preferences(&self) -> &[PreferenceManifest] {
        &self.preferences
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovers_the_bundled_extensions() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("extensions");
        let catalog = Catalog::discover(&[root]);
        let (extension, command) = catalog
            .command(&CommandId::new("com.gpui-kit.links", "links"))
            .expect("the bundled links command is discovered");
        assert_eq!(extension.name().as_ref(), "GPUI Kit");
        assert_eq!(command.module(), "commands/links.js");
    }
}
