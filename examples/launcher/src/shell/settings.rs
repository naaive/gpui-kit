//! The launcher's own settings: the summon shortcut, the theme, where
//! extensions are loaded from.
//!
//! They are stored as `settings.json` in the launcher's data directory. A
//! missing file means the defaults; an unreadable one is reported and the
//! defaults are used without overwriting it, so a hand edit with a typo is
//! not silently lost.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use super::hotkey::DEFAULT_SHORTCUT;

/// Where `settings.json` lives, if the platform has a data directory.
pub fn settings_path() -> Option<PathBuf> {
    super::data_directory().map(|directory| directory.join("settings.json"))
}

/// A setting with a fixed set of choices, stored by `value` and shown by
/// `title`.
macro_rules! choices {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $( $(#[$variant_meta:meta])* $variant:ident => ($value:literal, $title:literal), )+
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )+
        }

        // Not every setting is read back by both its value and its title.
        #[allow(dead_code)]
        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub fn value(self) -> &'static str {
                match self { $(Self::$variant => $value,)+ }
            }

            pub fn title(self) -> &'static str {
                match self { $(Self::$variant => $title,)+ }
            }

            pub fn from_value(value: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|choice| choice.value() == value)
            }

            pub fn from_title(title: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|choice| choice.title() == title)
            }
        }
    };
}

choices! {
    /// How large the launcher draws its text.
    pub enum TextSize {
        #[default]
        Standard => ("standard", "Standard"),
        Large => ("large", "Large"),
    }
}

impl TextSize {
    /// The window's `rem`, from which every size in the launcher is derived.
    pub fn rem(self) -> f32 {
        match self {
            Self::Standard => 16.,
            Self::Large => 17.5,
        }
    }
}

choices! {
    /// The display the launcher opens on.
    pub enum ShowOn {
        #[default]
        MouseScreen => ("mouse", "Screen containing the pointer"),
        ActiveWindowScreen => ("active_window", "Screen with the active window"),
        PrimaryScreen => ("primary", "Primary screen"),
    }
}

choices! {
    /// What Esc does on a command's page.
    pub enum EscapeKey {
        #[default]
        NavigateBack => ("back", "Go back, then close the window"),
        Close => ("close", "Close the window"),
    }
}

choices! {
    /// The keys, besides the arrows, that move the selection.
    pub enum NavigationKeys {
        #[default]
        Emacs => ("emacs", "Ctrl-N and Ctrl-P"),
        Vim => ("vim", "Ctrl-J and Ctrl-K"),
    }
}

choices! {
    /// How loosely the root search matches what is typed.
    pub enum SearchSensitivity {
        /// Only titles that contain the words typed.
        Low => ("low", "Low"),
        #[default]
        Medium => ("medium", "Medium"),
        /// Letters may be spread across the title.
        High => ("high", "High"),
    }
}

choices! {
    /// What Enter does to a Clipboard History entry.
    pub enum ClipboardAction {
        #[default]
        Paste => ("paste", "Paste to Active App"),
        Copy => ("copy", "Copy to Clipboard"),
    }
}

choices! {
    /// How the calculator writes and reads decimal numbers.
    pub enum DecimalSeparator {
        /// As the system's region does.
        #[default]
        Auto => ("auto", "System"),
        Dot => ("dot", "Dot (1,234.5)"),
        Comma => ("comma", "Comma (1.234,5)"),
    }
}

choices! {
    pub enum Appearance {
        /// Follows the system's light or dark appearance.
        #[default]
        System => ("system", "System"),
        Light => ("light", "Light"),
        Dark => ("dark", "Dark"),
    }
}

choices! {
    /// How the launcher opens: whole, or as just its search field until
    /// something is typed.
    pub enum WindowMode {
        #[default]
        Default => ("default", "Default"),
        Compact => ("compact", "Compact"),
    }
}

choices! {
    /// When the launcher, summoned again, starts over at the root search
    /// rather than on the command it was left on.
    pub enum PopToRoot {
        Immediately => ("immediately", "Immediately"),
        #[default]
        After90Seconds => ("after_90_seconds", "After 90 Seconds"),
        Never => ("never", "Never"),
    }
}

impl PopToRoot {
    /// Whether a launcher hidden `hidden_for` ago comes back where it was.
    pub fn keeps(self, hidden_for: std::time::Duration) -> bool {
        match self {
            Self::Immediately => false,
            Self::After90Seconds => hidden_for < std::time::Duration::from_secs(90),
            Self::Never => true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    /// A GPUI keystroke, such as `alt-space`.
    summon_shortcut: String,
    appearance: Appearance,
    window_mode: WindowMode,
    pop_to_root: PopToRoot,
    /// Loaded ahead of the bundled extensions.
    extension_directory: Option<PathBuf>,
    /// Where the Extension Store's listing comes from: `owner/repo@ref/path`
    /// on GitHub, or a folder; `None` for the default.
    store_source: Option<String>,
    /// Whether what is copied is recorded in Clipboard History.
    clipboard_history: bool,
    /// How many days copied entries are kept; 0 keeps them until deleted.
    clipboard_retention_days: u32,
    /// Whether typing a snippet's keyword in any application expands it.
    snippet_expansion: bool,
    /// Pixels Window Management leaves between windows and around them.
    window_gap: u32,
    /// What Caps Lock does.
    hyper_key: crate::hyper_key::HyperKey,
    /// Applications whose copies Clipboard History leaves out, by their
    /// executable's name (`keepass`), in lowercase.
    clipboard_ignored_apps: Vec<String>,
    /// Applications snippet keywords are not expanded in, by executable
    /// name, in lowercase.
    snippet_ignored_apps: Vec<String>,
    /// iCalendar feeds My Schedule shows: web addresses or `.ics` files.
    calendar_feeds: Vec<String>,
    /// The theme for light appearance, by name; `None` for the default.
    light_theme: Option<String>,
    /// The theme for dark appearance.
    dark_theme: Option<String>,
    /// Whether the launcher starts when you sign in.
    launch_at_login: bool,
    /// Whether the launcher keeps an icon in the menu bar or tray.
    show_tray_icon: bool,
    text_size: TextSize,
    /// Whether the compact window lists favorites before anything is typed.
    favorites_in_compact: bool,
    show_on: ShowOn,
    escape_key: EscapeKey,
    navigation_keys: NavigationKeys,
    search_sensitivity: SearchSensitivity,
    /// An HTTP proxy for the launcher's and extensions' requests, such as
    /// `http://127.0.0.1:7890`; `None` uses the system's.
    proxy: Option<String>,
    clipboard_action: ClipboardAction,
    decimal_separator: DecimalSeparator,
    /// Folders Search Files looks in; empty for the home folder.
    file_search_roots: Vec<String>,
    /// Folder names Search Files skips, such as `node_modules`.
    file_search_excluded: Vec<String>,
    /// The browser quicklinks open in, by application path or name; `None`
    /// for the system's default.
    quicklink_browser: Option<String>,
}

/// Folders Search Files skips unless told otherwise: build output and
/// dependencies nobody searches for by name.
pub const DEFAULT_EXCLUDED_FOLDERS: [&str; 6] = [
    "node_modules",
    "target",
    ".git",
    "AppData",
    "Library",
    "__pycache__",
];

/// Password managers, whose copies are left out unless the user says
/// otherwise; most also mark their copies private.
const PASSWORD_MANAGERS: [&str; 6] = [
    "1password",
    "bitwarden",
    "keepass",
    "keepassxc",
    "lastpass",
    "dashlane",
];

/// The choices for the gap between windows, in pixels.
pub const WINDOW_GAPS: [(u32, &str); 5] = [
    (0, "None"),
    (4, "4 px"),
    (8, "8 px"),
    (12, "12 px"),
    (16, "16 px"),
];

/// The choices for how long clipboard history is kept, in days.
pub const RETENTION_DAYS: [(u32, &str); 6] = [
    (1, "1 day"),
    (7, "7 days"),
    (30, "30 days"),
    (90, "3 months"),
    (365, "1 year"),
    (0, "Until deleted"),
];

impl Default for Settings {
    fn default() -> Self {
        Self {
            summon_shortcut: DEFAULT_SHORTCUT.into(),
            appearance: Appearance::default(),
            window_mode: WindowMode::default(),
            pop_to_root: PopToRoot::default(),
            extension_directory: None,
            store_source: None,
            clipboard_history: true,
            clipboard_retention_days: 90,
            snippet_expansion: false,
            window_gap: 0,
            hyper_key: crate::hyper_key::HyperKey::Off,
            clipboard_ignored_apps: PASSWORD_MANAGERS.map(str::to_owned).to_vec(),
            snippet_ignored_apps: Vec::new(),
            calendar_feeds: Vec::new(),
            light_theme: None,
            dark_theme: None,
            launch_at_login: false,
            show_tray_icon: true,
            text_size: TextSize::default(),
            favorites_in_compact: true,
            show_on: ShowOn::default(),
            escape_key: EscapeKey::default(),
            navigation_keys: NavigationKeys::default(),
            search_sensitivity: SearchSensitivity::default(),
            proxy: None,
            clipboard_action: ClipboardAction::default(),
            decimal_separator: DecimalSeparator::default(),
            file_search_roots: Vec::new(),
            file_search_excluded: DEFAULT_EXCLUDED_FOLDERS.map(str::to_owned).to_vec(),
            quicklink_browser: None,
        }
    }
}

impl Settings {
    pub fn with_summon_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.summon_shortcut = shortcut.into();
        self
    }

    pub fn with_appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn with_extension_directory(mut self, directory: Option<PathBuf>) -> Self {
        self.extension_directory = directory;
        self
    }

    pub fn summon_shortcut(&self) -> &str {
        &self.summon_shortcut
    }

    pub fn appearance(&self) -> Appearance {
        self.appearance
    }

    pub fn window_mode(&self) -> WindowMode {
        self.window_mode
    }

    pub fn pop_to_root(&self) -> PopToRoot {
        self.pop_to_root
    }

    pub fn extension_directory(&self) -> Option<&Path> {
        self.extension_directory.as_deref()
    }

    /// The store's listing, as the user set it; see
    /// [`crate::extensions::store::StoreSource`].
    pub fn store_source(&self) -> Option<&str> {
        self.store_source.as_deref()
    }

    pub fn is_recording_clipboard(&self) -> bool {
        self.clipboard_history
    }

    pub fn expands_snippets(&self) -> bool {
        self.snippet_expansion
    }

    pub fn window_gap(&self) -> u32 {
        self.window_gap
    }

    pub fn hyper_key(&self) -> crate::hyper_key::HyperKey {
        self.hyper_key
    }

    pub fn clipboard_ignored_apps(&self) -> &[String] {
        &self.clipboard_ignored_apps
    }

    /// Whether copies from the application `name` are left out.
    pub fn is_clipboard_ignored(&self, name: &str) -> bool {
        self.clipboard_ignored_apps
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
    }

    /// The theme chosen for dark or light appearance.
    pub fn theme(&self, dark: bool) -> Option<&str> {
        match dark {
            true => self.dark_theme.as_deref(),
            false => self.light_theme.as_deref(),
        }
    }

    pub fn with_theme(mut self, dark: bool, name: Option<String>) -> Self {
        match dark {
            true => self.dark_theme = name,
            false => self.light_theme = name,
        }
        self
    }

    pub fn with_appearance_value(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn launch_at_login(&self) -> bool {
        self.launch_at_login
    }

    pub fn shows_tray_icon(&self) -> bool {
        self.show_tray_icon
    }

    pub fn text_size(&self) -> TextSize {
        self.text_size
    }

    pub fn shows_favorites_in_compact(&self) -> bool {
        self.favorites_in_compact
    }

    pub fn show_on(&self) -> ShowOn {
        self.show_on
    }

    pub fn escape_key(&self) -> EscapeKey {
        self.escape_key
    }

    pub fn navigation_keys(&self) -> NavigationKeys {
        self.navigation_keys
    }

    pub fn search_sensitivity(&self) -> SearchSensitivity {
        self.search_sensitivity
    }

    pub fn proxy(&self) -> Option<&str> {
        self.proxy.as_deref()
    }

    pub fn clipboard_action(&self) -> ClipboardAction {
        self.clipboard_action
    }

    pub fn decimal_separator(&self) -> DecimalSeparator {
        self.decimal_separator
    }

    pub fn file_search_roots(&self) -> &[String] {
        &self.file_search_roots
    }

    pub fn file_search_excluded(&self) -> &[String] {
        &self.file_search_excluded
    }

    pub fn quicklink_browser(&self) -> Option<&str> {
        self.quicklink_browser.as_deref()
    }

    pub fn with_window_mode(mut self, mode: WindowMode) -> Self {
        self.window_mode = mode;
        self
    }

    pub fn with_pop_to_root(mut self, when: PopToRoot) -> Self {
        self.pop_to_root = when;
        self
    }

    pub fn with_store_source(mut self, source: Option<String>) -> Self {
        self.store_source = source;
        self
    }

    pub fn with_clipboard_history(mut self, record: bool) -> Self {
        self.clipboard_history = record;
        self
    }

    /// Days clipboard entries are kept; 0 keeps them until deleted.
    pub fn with_clipboard_retention_days(mut self, days: u32) -> Self {
        self.clipboard_retention_days = days;
        self
    }

    pub fn with_snippet_expansion(mut self, expand: bool) -> Self {
        self.snippet_expansion = expand;
        self
    }

    pub fn with_window_gap(mut self, gap: u32) -> Self {
        self.window_gap = gap;
        self
    }

    pub fn with_hyper_key(mut self, hyper_key: crate::hyper_key::HyperKey) -> Self {
        self.hyper_key = hyper_key;
        self
    }

    pub fn with_clipboard_ignored_apps(mut self, apps: Vec<String>) -> Self {
        self.clipboard_ignored_apps = apps;
        self
    }

    pub fn with_snippet_ignored_apps(mut self, apps: Vec<String>) -> Self {
        self.snippet_ignored_apps = apps;
        self
    }

    pub fn with_calendar_feeds(mut self, feeds: Vec<String>) -> Self {
        self.calendar_feeds = feeds;
        self
    }

    pub fn with_launch_at_login(mut self, launch: bool) -> Self {
        self.launch_at_login = launch;
        self
    }

    pub fn with_show_tray_icon(mut self, show: bool) -> Self {
        self.show_tray_icon = show;
        self
    }

    pub fn with_text_size(mut self, size: TextSize) -> Self {
        self.text_size = size;
        self
    }

    pub fn with_favorites_in_compact(mut self, show: bool) -> Self {
        self.favorites_in_compact = show;
        self
    }

    pub fn with_show_on(mut self, show_on: ShowOn) -> Self {
        self.show_on = show_on;
        self
    }

    pub fn with_escape_key(mut self, escape_key: EscapeKey) -> Self {
        self.escape_key = escape_key;
        self
    }

    pub fn with_navigation_keys(mut self, keys: NavigationKeys) -> Self {
        self.navigation_keys = keys;
        self
    }

    pub fn with_search_sensitivity(mut self, sensitivity: SearchSensitivity) -> Self {
        self.search_sensitivity = sensitivity;
        self
    }

    pub fn with_proxy(mut self, proxy: Option<String>) -> Self {
        self.proxy = proxy;
        self
    }

    pub fn with_clipboard_action(mut self, action: ClipboardAction) -> Self {
        self.clipboard_action = action;
        self
    }

    pub fn with_decimal_separator(mut self, separator: DecimalSeparator) -> Self {
        self.decimal_separator = separator;
        self
    }

    pub fn with_file_search_roots(mut self, roots: Vec<String>) -> Self {
        self.file_search_roots = roots;
        self
    }

    pub fn with_file_search_excluded(mut self, folders: Vec<String>) -> Self {
        self.file_search_excluded = folders;
        self
    }

    pub fn with_quicklink_browser(mut self, browser: Option<String>) -> Self {
        self.quicklink_browser = browser;
        self
    }

    pub fn calendar_feeds(&self) -> &[String] {
        &self.calendar_feeds
    }

    pub fn snippet_ignored_apps(&self) -> &[String] {
        &self.snippet_ignored_apps
    }

    /// Whether snippet keywords typed in the application `name` stay.
    pub fn is_snippet_ignored(&self, name: &str) -> bool {
        self.snippet_ignored_apps
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(name))
    }

    /// These settings, with copies from `name` left out too.
    pub fn with_clipboard_ignored_app(mut self, name: &str) -> Self {
        if !self.is_clipboard_ignored(name) {
            self.clipboard_ignored_apps.push(name.to_lowercase());
        }
        self
    }

    /// Days clipboard entries are kept; `None` keeps them until deleted.
    pub fn clipboard_retention_days(&self) -> Option<u32> {
        (self.clipboard_retention_days > 0).then_some(self.clipboard_retention_days)
    }

    /// Reads settings; a missing file yields the defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid settings", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Writes settings through a temporary file, so a crash mid-write never
    /// leaves a truncated `settings.json`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("cannot create {}", directory.display()))?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, path)
            .with_context(|| format!("cannot write {}", path.display()))
    }
}

/// `KeePass.exe, 1password` → `["keepass", "1password"]`.
pub fn parse_app_list(list: &str) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for app in list.split([',', ';', '\n']) {
        let app = app.trim().to_lowercase();
        let app = app.strip_suffix(".exe").unwrap_or(&app).trim().to_owned();
        if !app.is_empty() && !apps.contains(&app) {
            apps.push(app);
        }
    }
    apps
}

/// Expands a leading `~` to the home directory, as a shell would.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_builder_and_defaults() {
        let settings = Settings::default();
        assert_eq!(settings.summon_shortcut(), "alt-space");
        assert_eq!(settings.appearance(), Appearance::System);
        assert_eq!(settings.extension_directory(), None);

        let settings = settings
            .with_summon_shortcut("ctrl-space")
            .with_appearance(Appearance::Dark)
            .with_extension_directory(Some("/x".into()));
        assert_eq!(settings.summon_shortcut(), "ctrl-space");
        assert_eq!(settings.appearance(), Appearance::Dark);
        assert_eq!(settings.extension_directory(), Some(Path::new("/x")));
    }

    #[test]
    fn test_settings_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("settings.json");
        assert_eq!(
            Settings::load(&path).unwrap(),
            Settings::default(),
            "a missing file means the defaults"
        );

        let settings = Settings::default()
            .with_summon_shortcut("ctrl-shift-space")
            .with_appearance(Appearance::Light)
            .with_extension_directory(Some("/extensions".into()));
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), settings);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"appearance\": \"light\""), "{text}");

        std::fs::write(&path, r#"{ "appearance": "dark" }"#).unwrap();
        assert_eq!(
            Settings::load(&path).unwrap(),
            Settings::default().with_appearance(Appearance::Dark),
            "missing keys take their defaults"
        );

        std::fs::write(&path, "{ not json").unwrap();
        assert!(Settings::load(&path).is_err());
    }

    #[test]
    fn test_expand_home() {
        let Some(home) = dirs::home_dir() else {
            return;
        };
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("~/extensions"), home.join("extensions"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
        assert_eq!(expand_home("/abs"), PathBuf::from("/abs"));
    }
}
