//! Every command the root search can open, grouped the way the Extensions
//! settings list them: the launcher's own features first, then each
//! extension.

use gpui_kit::SharedString;

use crate::{
    extensions::Catalog,
    model::{Image, Item},
    sources::{CommandSource as _, ExtensionCommands, system::SystemCommands},
};

/// A built-in feature whose commands are listed together, and which may
/// have settings of its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Feature {
    Launcher,
    ClipboardHistory,
    Snippets,
    Quicklinks,
    SearchFiles,
    Calculator,
    WindowManagement,
    Calendar,
    FloatingNotes,
    Focus,
    Media,
    Timers,
    ScriptCommands,
    Utilities,
    System,
    SystemSettings,
}

impl Feature {
    /// The order the features are listed in.
    const ALL: [Self; 16] = [
        Self::Launcher,
        Self::ClipboardHistory,
        Self::Snippets,
        Self::Quicklinks,
        Self::SearchFiles,
        Self::Calculator,
        Self::WindowManagement,
        Self::Calendar,
        Self::FloatingNotes,
        Self::Focus,
        Self::Media,
        Self::Timers,
        Self::ScriptCommands,
        Self::Utilities,
        Self::System,
        Self::SystemSettings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Launcher => "Launcher",
            Self::ClipboardHistory => "Clipboard History",
            Self::Snippets => "Snippets",
            Self::Quicklinks => "Quicklinks",
            Self::SearchFiles => "Search Files",
            Self::Calculator => "Calculator",
            Self::WindowManagement => "Window Management",
            Self::Calendar => "Calendar & Reminders",
            Self::FloatingNotes => "Floating Notes",
            Self::Focus => "Focus",
            Self::Media => "Media",
            Self::Timers => "Timers",
            Self::ScriptCommands => "Script Commands",
            Self::Utilities => "Utilities",
            Self::System => "System",
            Self::SystemSettings => "System Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Launcher => "search",
            Self::ClipboardHistory => "clipboard-list",
            Self::Snippets => "text-cursor-input",
            Self::Quicklinks => "link",
            Self::SearchFiles => "file-search",
            Self::Calculator => "calculator",
            Self::WindowManagement => "app-window",
            Self::Calendar => "calendar",
            Self::FloatingNotes => "sticky-note",
            Self::Focus => "target",
            Self::Media => "music",
            Self::Timers => "timer",
            Self::ScriptCommands => "square-terminal",
            Self::Utilities => "wrench",
            Self::System => "monitor",
            Self::SystemSettings => "settings",
        }
    }

    /// What the feature is for, shown above its settings.
    pub fn description(self) -> &'static str {
        match self {
            Self::Launcher => "The launcher's own commands: settings, extensions and themes.",
            Self::ClipboardHistory => "Everything you copy, searchable and ready to paste again.",
            Self::Snippets => "Text you save once and insert by name or by a keyword.",
            Self::Quicklinks => {
                "Web addresses and folders you open by name, with the query filled in."
            }
            Self::SearchFiles => "Files and folders by name, from the folders you choose.",
            Self::Calculator => "Math, units, currencies, dates and time zones, as you type.",
            Self::WindowManagement => "Moves and resizes the frontmost window.",
            Self::Calendar => "Your schedule and reminders.",
            Self::FloatingNotes => "A note that stays above other windows.",
            Self::Focus => "Timed sessions without distractions.",
            Self::Media => "Controls for what is playing.",
            Self::Timers => "Timers and a stopwatch.",
            Self::ScriptCommands => "Your own scripts, run as commands.",
            Self::Utilities => "Emoji, colors, translation, screenshots and more.",
            Self::System => "Power, sound, displays and connections.",
            Self::SystemSettings => "Pages of the system's settings.",
        }
    }

    /// The feature a built-in command belongs to, by its id.
    fn of(id: &str) -> Self {
        let name = id.split_once('/').map_or(id, |(_, name)| name);
        match id {
            _ if id.starts_with("window/") => Self::WindowManagement,
            _ if id.starts_with("settings/") => Self::SystemSettings,
            _ if name.contains("clipboard") => Self::ClipboardHistory,
            _ if name.contains("snippet") => Self::Snippets,
            _ if name.contains("quicklink") => Self::Quicklinks,
            _ if name.contains("script") => Self::ScriptCommands,
            _ if name == "search-files" => Self::SearchFiles,
            _ if name.contains("calculator") => Self::Calculator,
            _ if name.contains("note") => Self::FloatingNotes,
            _ if name.contains("focus") => Self::Focus,
            _ if name.contains("schedule") || name.contains("reminder") => Self::Calendar,
            _ if name.contains("timer") || name == "stopwatch" => Self::Timers,
            _ if matches!(
                name,
                "now-playing" | "play-pause" | "next-track" | "previous-track"
            ) =>
            {
                Self::Media
            }
            _ if matches!(
                name,
                "settings"
                    | "extensions"
                    | "store"
                    | "quit"
                    | "toggle-appearance"
                    | "change-theme"
                    | "export-data"
                    | "import-data"
            ) =>
            {
                Self::Launcher
            }
            _ if name.starts_with("search-")
                || matches!(
                    name,
                    "translate"
                        | "define-word"
                        | "pick-color"
                        | "system-monitor"
                        | "switch-windows"
                )
                || name.contains("emoji")
                || name.contains("color") =>
            {
                Self::Utilities
            }
            _ => Self::System,
        }
    }
}

/// One command, as the Extensions settings list it.
#[derive(Clone)]
pub struct Command {
    /// The root search item id, which aliases, hotkeys and the disabled set
    /// are keyed by.
    pub id: String,
    pub title: SharedString,
    pub image: Option<Image>,
    /// "Command", "Settings", "Menu Bar"…
    pub kind: SharedString,
}

impl Command {
    fn of(item: &Item) -> Self {
        Self {
            id: item.id().as_str().to_owned(),
            title: item.title().clone(),
            image: item.image().cloned(),
            kind: item
                .accessories()
                .iter()
                .rev()
                .find_map(|accessory| accessory.label().cloned())
                .unwrap_or_else(|| "Command".into()),
        }
    }
}

/// What a group of commands is.
#[derive(Clone)]
pub enum Owner {
    BuiltIn(Feature),
    /// An installed extension, by id.
    Extension {
        id: SharedString,
        description: Option<SharedString>,
        author: Option<SharedString>,
        version: SharedString,
    },
}

#[derive(Clone)]
pub struct Group {
    pub owner: Owner,
    pub title: SharedString,
    pub image: Image,
    pub commands: Vec<Command>,
}

impl Group {
    /// A stable key for selection and expansion.
    pub fn key(&self) -> String {
        match &self.owner {
            Owner::BuiltIn(feature) => format!("built-in:{}", feature.title()),
            Owner::Extension { id, .. } => format!("extension:{id}"),
        }
    }

    pub fn command(&self, id: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.id == id)
    }
}

/// Every command, by feature and extension. Built-in features without a
/// command on this platform are left out.
pub fn groups(catalog: &Catalog) -> Vec<Group> {
    let built_in = SystemCommands::new(true).commands();
    let mut groups: Vec<Group> = Feature::ALL
        .iter()
        .map(|&feature| Group {
            owner: Owner::BuiltIn(feature),
            title: feature.title().into(),
            image: Image::Icon(feature.icon().into()),
            commands: built_in
                .iter()
                .filter(|item| Feature::of(item.id().as_str()) == feature)
                .map(Command::of)
                .collect(),
        })
        // The calculator has settings even though its answers are not
        // commands of their own.
        .filter(|group| {
            !group.commands.is_empty() || matches!(group.owner, Owner::BuiltIn(Feature::Calculator))
        })
        .collect();

    let items = ExtensionCommands::new(catalog).commands();
    let mut seen = std::collections::HashSet::new();
    for (extension, _) in catalog.commands() {
        if !seen.insert(extension.id().clone()) {
            continue;
        }
        let ids: Vec<String> = extension
            .commands()
            .iter()
            .map(|command| command.id().to_string())
            .collect();
        groups.push(Group {
            owner: Owner::Extension {
                id: extension.id().clone(),
                description: extension.description().cloned(),
                author: extension.author().cloned(),
                version: extension.version().clone(),
            },
            title: extension.name().clone(),
            image: Image::Icon(extension.icon().cloned().unwrap_or_else(|| "puzzle".into())),
            commands: items
                .iter()
                .filter(|item| ids.iter().any(|id| id == item.id().as_str()))
                .map(Command::of)
                .collect(),
        });
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_built_in_commands_find_their_feature() {
        assert_eq!(
            Feature::of("system/clipboard-history"),
            Feature::ClipboardHistory
        );
        assert_eq!(Feature::of("system/create-snippet"), Feature::Snippets);
        assert_eq!(Feature::of("window/left-half"), Feature::WindowManagement);
        assert_eq!(Feature::of("settings/display"), Feature::SystemSettings);
        assert_eq!(Feature::of("system/settings"), Feature::Launcher);
        assert_eq!(Feature::of("system/search-emoji"), Feature::Utilities);
        assert_eq!(Feature::of("system/start-timer"), Feature::Timers);
        assert_eq!(Feature::of("system/sleep"), Feature::System);
    }
}
