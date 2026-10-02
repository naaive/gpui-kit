//! Where the root search's commands come from.
//!
//! A source turns something outside the launcher — installed applications,
//! the operating system, extension manifests — into plain [`Item`]s. Sources
//! know nothing about ranking or sections; the root search page combines them.
//! Two sources depend on the query instead of on the machine: the
//! [`calculator`] and the [`fallback`] commands.

pub mod applications;
pub mod calculator;
mod conversion;
pub mod currency;
mod dates;
pub mod fallback;
pub mod process;
pub mod system;

use gpui_kit::SharedString;

use crate::{
    extensions::{Catalog, LaunchRequest},
    model::{Accessory, Action, Effect, Item, ItemId},
};

/// A collection of commands that does not depend on the query.
pub trait CommandSource {
    /// The section the commands are listed under when nothing is typed.
    fn title(&self) -> SharedString;

    /// Every command the source offers now.
    fn commands(&self) -> Vec<Item>;
}

/// The commands declared by installed extensions; read from their manifests,
/// so listing them runs no extension code.
pub struct ExtensionCommands<'a> {
    catalog: &'a Catalog,
}

impl<'a> ExtensionCommands<'a> {
    pub fn new(catalog: &'a Catalog) -> Self {
        Self { catalog }
    }
}

impl CommandSource for ExtensionCommands<'_> {
    fn title(&self) -> SharedString {
        "Extensions".into()
    }

    fn commands(&self) -> Vec<Item> {
        self.catalog
            .commands()
            .map(|(extension, command)| {
                // A command that runs on its own says so, and can be stopped.
                let runs_alone = match command.mode() {
                    crate::extensions::CommandMode::MenuBar => {
                        Some(("Menu Bar", "Remove from Tray"))
                    }
                    _ if command.interval().is_some() => {
                        Some(("Background", "Stop Running in Background"))
                    }
                    _ => None,
                };
                let item = Item::new(
                    ItemId::new(command.id().to_string()),
                    command.title().clone(),
                )
                .with_subtitle(
                    command
                        .subtitle()
                        .cloned()
                        .unwrap_or_else(|| extension.name().clone()),
                )
                .with_accessory(Accessory::text(
                    runs_alone.map_or("Command", |(label, _)| label),
                ))
                .with_action(Action::new(
                    "Open Command",
                    Effect::Launch(LaunchRequest::new(command.id().clone())),
                ));
                // Only an extension with settings has anything to configure.
                let has_preferences =
                    !extension.preferences().is_empty() || !command.preferences().is_empty();
                // Raycast's Configure Extension opens the extension in
                // settings, where its commands' aliases and hotkeys are too.
                let id = command.id().to_string();
                let item = match has_preferences {
                    true => item.with_action(
                        Action::new(
                            "Configure Extension",
                            Effect::Run(crate::model::RunHandler::new(move |(), _, cx| {
                                crate::settings_window::open_extension(id.clone(), cx)
                            })),
                        )
                        .with_shortcut("secondary-shift-,"),
                    ),
                    false => item,
                };
                let item = match runs_alone {
                    Some((_, stop)) => {
                        let id = command.id().clone();
                        item.with_action(Action::new(
                            stop,
                            Effect::Run(crate::model::RunHandler::new(move |(), _, cx| {
                                crate::shell::background::deactivate(&id, cx);
                                crate::shell::launcher::perform(
                                    Effect::ShowToast(crate::model::Toast::new(
                                        crate::model::ToastStyle::Success,
                                        "Stopped",
                                    )),
                                    cx,
                                );
                            })),
                        ))
                    }
                    None => item,
                };
                let item = match command.icon() {
                    Some(icon) => item.with_icon(icon.clone()),
                    None => item,
                };
                command
                    .keywords()
                    .iter()
                    .fold(item, |item, keyword| item.with_keyword(keyword.clone()))
            })
            .collect()
    }
}
