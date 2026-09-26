//! Installed applications, found the way each platform's own launcher finds
//! them: `.app` bundles on macOS, XDG desktop entries on Linux, Start Menu
//! shortcuts on Windows.
//!
//! Scanning touches hundreds of files, so it runs on a background executor and
//! returns plain [`Application`]s; they become items (which hold UI callbacks
//! and cannot cross threads) on the foreground.

#[cfg(any(target_os = "macos", test))]
mod bundle;
#[cfg(any(target_os = "linux", test))]
mod desktop_entry;
#[cfg(any(target_os = "windows", test))]
mod start_menu;

use std::path::PathBuf;

use gpui_kit::SharedString;
use notify::{RecursiveMode, Watcher as _};

use super::{CommandSource, process::CommandLine};
use crate::model::{Accessory, Action, Effect, Image, Item, ItemId};

/// One installed application.
#[derive(Clone, Debug, PartialEq)]
pub struct Application {
    id: String,
    name: SharedString,
    subtitle: Option<SharedString>,
    keywords: Vec<SharedString>,
    /// The bundle, desktop entry or shortcut the application was found as.
    location: PathBuf,
    icon: Option<PathBuf>,
    launch: Launch,
}

/// How an application starts.
#[derive(Clone, Debug, PartialEq)]
pub enum Launch {
    /// Opening its location with the system starts it: a bundle, a shortcut.
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "windows", test)),
        allow(
            dead_code,
            reason = "Linux applications are desktop entries, which run"
        )
    )]
    Open,
    /// Its location describes a command line to run: a desktop entry.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    Run(CommandLine),
}

impl Application {
    pub fn new(name: impl Into<SharedString>, location: PathBuf, launch: Launch) -> Self {
        Self {
            id: format!("app:{}", location.display()),
            name: name.into(),
            subtitle: None,
            keywords: Vec::new(),
            location,
            icon: None,
            launch,
        }
    }

    /// The id defaults to the location; a desktop entry is identified by its
    /// desktop file id instead, which survives moving between data dirs.
    #[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub fn with_subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Adds a keyword unless it repeats the name or another keyword.
    pub fn with_keyword(mut self, keyword: impl Into<SharedString>) -> Self {
        let keyword = keyword.into();
        if !keyword.trim().is_empty() && keyword != self.name && !self.keywords.contains(&keyword) {
            self.keywords.push(keyword);
        }
        self
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub fn with_icon(mut self, icon: PathBuf) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn item(&self) -> Item {
        let open = match &self.launch {
            Launch::Open => Effect::OpenPath(self.location.clone()),
            Launch::Run(command) => command
                .clone()
                .effect(format!("Couldn’t open “{}”", self.name)),
        };
        let image = match &self.icon {
            Some(icon) => Image::File(icon.clone()),
            None => Image::Icon("app-window".into()),
        };
        let item = Item::new(ItemId::new(self.id.clone()), self.name.clone())
            .with_image(image)
            .with_accessory(Accessory::text("Application"))
            .with_action(Action::new("Open Application", open))
            .with_action(
                Action::new(REVEAL_TITLE, Effect::RevealPath(self.location.clone()))
                    .with_shortcut("secondary-shift-f"),
            )
            .with_action(
                Action::new(
                    "Copy Path",
                    Effect::Copy(self.location.display().to_string().into()),
                )
                .with_shortcut("secondary-shift-c"),
            );
        let item = match &self.subtitle {
            Some(subtitle) => item.with_subtitle(subtitle.clone()),
            None => item,
        };
        self.keywords
            .iter()
            .fold(item, |item, keyword| item.with_keyword(keyword.clone()))
    }
}

#[cfg(target_os = "macos")]
const REVEAL_TITLE: &str = "Show in Finder";
#[cfg(target_os = "windows")]
const REVEAL_TITLE: &str = "Show in File Explorer";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const REVEAL_TITLE: &str = "Show in File Manager";

/// The applications found by the last scan.
pub struct Applications {
    items: Vec<Item>,
}

impl Applications {
    pub fn new(applications: &[Application]) -> Self {
        Self {
            items: applications.iter().map(Application::item).collect(),
        }
    }
}

impl CommandSource for Applications {
    fn title(&self) -> SharedString {
        "Applications".into()
    }

    fn commands(&self) -> Vec<Item> {
        self.items.clone()
    }
}

/// Where this platform keeps applications, most specific first: an entry in
/// an earlier directory shadows one with the same id in a later one.
pub fn default_directories() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        bundle::default_directories()
    }
    #[cfg(target_os = "linux")]
    {
        desktop_entry::default_directories()
    }
    #[cfg(target_os = "windows")]
    {
        start_menu::default_directories()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Vec::new()
    }
}

/// Reads every application under `directories`, sorted by name. Blocking;
/// call it on a background executor.
pub fn scan(directories: &[PathBuf]) -> Vec<Application> {
    #[cfg(target_os = "macos")]
    let mut applications = bundle::scan(
        directories,
        &bundle::preferred_languages(),
        dirs::cache_dir()
            .map(|dir| dir.join("gpui-kit-launcher").join("icons"))
            .as_deref(),
    );
    #[cfg(target_os = "linux")]
    let mut applications = desktop_entry::scan(
        directories,
        &desktop_entry::Environment::current(),
        &desktop_entry::IconLookup::current(),
    );
    #[cfg(target_os = "windows")]
    let mut applications = start_menu::scan(directories);
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let mut applications: Vec<Application> = {
        let _ = directories;
        Vec::new()
    };

    applications.sort_by_cached_key(|application| application.name.to_lowercase());
    // Spell Chinese names now, off the UI thread, so the first keystroke does
    // not pay for it.
    for application in &applications {
        crate::search::pinyin::spellings(&application.name);
    }
    applications
}

/// Calls `on_change` from a background thread whenever something changes in
/// `directories`. The returned watcher stops watching when dropped; `None`
/// when no directory could be watched.
pub fn watch(
    directories: &[PathBuf],
    on_change: impl Fn() + Send + 'static,
) -> Option<notify::RecommendedWatcher> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !event.kind.is_access()
        {
            on_change();
        }
    })
    .map_err(|error| tracing::warn!("cannot watch applications: {error}"))
    .ok()?;
    let mut watching = false;
    for directory in directories.iter().filter(|directory| directory.is_dir()) {
        match watcher.watch(directory, RecursiveMode::Recursive) {
            Ok(()) => watching = true,
            Err(error) => tracing::warn!("cannot watch {}: {error}", directory.display()),
        }
    }
    watching.then_some(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_application_item_opens_reveals_and_copies() {
        let application = Application::new(
            "Visual Studio Code",
            PathBuf::from("/Applications/Visual Studio Code.app"),
            Launch::Open,
        )
        .with_keyword("Code")
        .with_keyword("Code")
        .with_keyword("Visual Studio Code");
        assert_eq!(application.keywords, ["Code"]);

        let item = application.item();
        assert_eq!(
            item.id().as_str(),
            "app:/Applications/Visual Studio Code.app"
        );
        let effects: Vec<&Effect> = item
            .actions()
            .actions()
            .map(|action| action.effect())
            .collect();
        assert!(matches!(effects[0], Effect::OpenPath(_)));
        assert!(matches!(effects[1], Effect::RevealPath(_)));
        assert!(
            matches!(effects[2], Effect::Copy(path) if path == "/Applications/Visual Studio Code.app")
        );
        assert_eq!(item.image(), Some(&Image::Icon("app-window".into())));
    }
}
