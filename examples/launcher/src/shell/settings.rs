//! The launcher's own settings: the summon shortcut, the theme, where
//! extensions are loaded from.
//!
//! They are stored as `settings.json` in the launcher's data directory. A
//! missing file means the defaults; an unreadable one is reported and the
//! defaults are used without overwriting it, so a hand edit with a typo is
//! not silently lost.

mod page;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use gpui_kit::{App, SharedString, Window};
use serde::{Deserialize, Serialize};

pub use page::SettingsPage;

use super::hotkey::{DEFAULT_SHORTCUT, parse_shortcut};
use crate::{
    model::{FormValue, FormValues},
    pages::{self, PageHandle},
};

/// Builds the settings page.
pub fn settings_page(_window: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(SettingsPage::new(cx)))
}

/// Where `settings.json` lives, if the platform has a data directory.
pub fn settings_path() -> Option<PathBuf> {
    super::data_directory().map(|directory| directory.join("settings.json"))
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follows the system's light or dark appearance.
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    /// The value a form submits for this appearance.
    pub fn value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|appearance| appearance.value() == value)
    }
}

/// How the launcher opens: whole, or as just its search field until
/// something is typed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Default,
    Compact,
}

impl WindowMode {
    pub const ALL: [Self; 2] = [Self::Default, Self::Compact];

    pub fn value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Compact => "compact",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Compact => "Compact",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.value() == value)
    }
}

/// When the launcher, summoned again, starts over at the root search
/// rather than on the command it was left on.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PopToRoot {
    Immediately,
    #[default]
    After90Seconds,
    Never,
}

impl PopToRoot {
    pub const ALL: [Self; 3] = [Self::Immediately, Self::After90Seconds, Self::Never];

    pub fn value(self) -> &'static str {
        match self {
            Self::Immediately => "immediately",
            Self::After90Seconds => "after_90_seconds",
            Self::Never => "never",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Immediately => "Immediately",
            Self::After90Seconds => "After 90 Seconds",
            Self::Never => "Never",
        }
    }

    fn from_value(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|when| when.value() == value)
    }

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
}

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
        }
    }
}

impl Settings {
    #[cfg(test)]
    pub fn with_summon_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.summon_shortcut = shortcut.into();
        self
    }

    #[cfg(test)]
    pub fn with_appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    #[cfg(test)]
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

/// The ids of the settings form's fields.
pub(crate) mod field {
    pub const SUMMON_SHORTCUT: &str = "summon_shortcut";
    pub const APPEARANCE: &str = "appearance";
    pub const WINDOW_MODE: &str = "window_mode";
    pub const POP_TO_ROOT: &str = "pop_to_root";
    pub const EXTENSION_DIRECTORY: &str = "extension_directory";
    pub const CLIPBOARD_HISTORY: &str = "clipboard_history";
    pub const CLIPBOARD_RETENTION: &str = "clipboard_retention_days";
    pub const SNIPPET_EXPANSION: &str = "snippet_expansion";
    pub const WINDOW_GAP: &str = "window_gap";
    pub const HYPER_KEY: &str = "hyper_key";
    pub const CLIPBOARD_IGNORED_APPS: &str = "clipboard_ignored_apps";
    pub const SNIPPET_IGNORED_APPS: &str = "snippet_ignored_apps";
    pub const CALENDAR_FEEDS: &str = "calendar_feeds";
}

/// Validation messages by field id.
pub type FieldErrors = BTreeMap<&'static str, SharedString>;

/// Reads a submitted settings form.
///
/// A field missing from `values` keeps its value from `current`, so a form
/// renderer that submits only what changed cannot reset the others.
pub fn from_form(values: &FormValues, current: &Settings) -> Result<Settings, FieldErrors> {
    let mut errors = FieldErrors::new();
    let text = |id: &str| match values.get(id) {
        Some(FormValue::Text(text)) => Some(text.trim().to_string()),
        Some(FormValue::Empty) => Some(String::new()),
        Some(FormValue::Bool(_)) | None => None,
    };

    let summon_shortcut = match text(field::SUMMON_SHORTCUT) {
        Some(shortcut) => match parse_shortcut(&shortcut) {
            Ok(_) => shortcut,
            Err(error) => {
                errors.insert(field::SUMMON_SHORTCUT, error.to_string().into());
                current.summon_shortcut.clone()
            }
        },
        None => current.summon_shortcut.clone(),
    };

    let appearance = match text(field::APPEARANCE) {
        Some(value) => Appearance::from_value(&value).unwrap_or_else(|| {
            errors.insert(field::APPEARANCE, "Choose System, Light or Dark.".into());
            current.appearance
        }),
        None => current.appearance,
    };

    let window_mode = match text(field::WINDOW_MODE) {
        Some(value) => WindowMode::from_value(&value).unwrap_or_else(|| {
            errors.insert(field::WINDOW_MODE, "Choose Default or Compact.".into());
            current.window_mode
        }),
        None => current.window_mode,
    };

    let pop_to_root = match text(field::POP_TO_ROOT) {
        Some(value) => PopToRoot::from_value(&value).unwrap_or_else(|| {
            errors.insert(field::POP_TO_ROOT, "Choose when to start over.".into());
            current.pop_to_root
        }),
        None => current.pop_to_root,
    };

    let extension_directory = match text(field::EXTENSION_DIRECTORY) {
        Some(path) if path.is_empty() => None,
        Some(path) => {
            let directory = expand_home(&path);
            if directory.is_dir() {
                Some(directory)
            } else {
                errors.insert(
                    field::EXTENSION_DIRECTORY,
                    "No folder exists at this path.".into(),
                );
                current.extension_directory.clone()
            }
        }
        None => current.extension_directory.clone(),
    };

    let clipboard_history = match values.get(field::CLIPBOARD_HISTORY) {
        Some(FormValue::Bool(record)) => *record,
        _ => current.clipboard_history,
    };
    let snippet_expansion = match values.get(field::SNIPPET_EXPANSION) {
        Some(FormValue::Bool(expand)) => *expand,
        _ => current.snippet_expansion,
    };
    let clipboard_retention_days = match text(field::CLIPBOARD_RETENTION) {
        Some(days) => match days.parse::<u32>() {
            Ok(days) if RETENTION_DAYS.iter().any(|(known, _)| *known == days) => days,
            _ => {
                errors.insert(
                    field::CLIPBOARD_RETENTION,
                    "Choose how long to keep entries.".into(),
                );
                current.clipboard_retention_days
            }
        },
        None => current.clipboard_retention_days,
    };

    let window_gap = match text(field::WINDOW_GAP) {
        Some(gap) => match gap.parse::<u32>() {
            Ok(gap) if WINDOW_GAPS.iter().any(|(known, _)| *known == gap) => gap,
            _ => {
                errors.insert(field::WINDOW_GAP, "Choose a gap.".into());
                current.window_gap
            }
        },
        None => current.window_gap,
    };

    let clipboard_ignored_apps = match text(field::CLIPBOARD_IGNORED_APPS) {
        Some(list) => parse_app_list(&list),
        None => current.clipboard_ignored_apps.clone(),
    };

    let snippet_ignored_apps = match text(field::SNIPPET_IGNORED_APPS) {
        Some(list) => parse_app_list(&list),
        None => current.snippet_ignored_apps.clone(),
    };

    let calendar_feeds = match text(field::CALENDAR_FEEDS) {
        Some(list) => {
            let feeds: Vec<String> = list
                .lines()
                .map(str::trim)
                .filter(|feed| !feed.is_empty())
                .map(str::to_owned)
                .collect();
            let invalid = feeds.iter().find(|feed| {
                !(feed.starts_with("https://")
                    || feed.starts_with("http://")
                    || feed.starts_with("webcal://")
                    || expand_home(feed).is_file())
            });
            if let Some(invalid) = invalid {
                errors.insert(
                    field::CALENDAR_FEEDS,
                    format!("“{invalid}” is neither a web address nor a file.").into(),
                );
            }
            feeds
        }
        None => current.calendar_feeds.clone(),
    };

    let hyper_key = match text(field::HYPER_KEY) {
        Some(value) => crate::hyper_key::HyperKey::from_value(&value).unwrap_or_else(|| {
            errors.insert(field::HYPER_KEY, "Choose what Caps Lock does.".into());
            current.hyper_key
        }),
        None => current.hyper_key,
    };

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Settings {
        summon_shortcut,
        appearance,
        window_mode,
        pop_to_root,
        extension_directory,
        clipboard_history,
        clipboard_retention_days,
        snippet_expansion,
        window_gap,
        hyper_key,
        clipboard_ignored_apps,
        snippet_ignored_apps,
        calendar_feeds,
        light_theme: current.light_theme.clone(),
        dark_theme: current.dark_theme.clone(),
    })
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

    fn form(entries: &[(&'static str, &str)]) -> FormValues {
        entries
            .iter()
            .fold(FormValues::new(), |values, (id, text)| {
                values.with(*id, FormValue::Text(text.to_string().into()))
            })
    }

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
    fn test_from_form() {
        let directory = tempfile::tempdir().unwrap();
        let current = Settings::default();
        let settings = from_form(
            &form(&[
                (field::SUMMON_SHORTCUT, " ctrl-space "),
                (field::APPEARANCE, "dark"),
                (
                    field::EXTENSION_DIRECTORY,
                    directory.path().to_str().unwrap(),
                ),
            ]),
            &current,
        )
        .unwrap();
        assert_eq!(settings.summon_shortcut(), "ctrl-space");
        assert_eq!(settings.appearance(), Appearance::Dark);
        assert_eq!(settings.extension_directory(), Some(directory.path()));

        let cleared = from_form(&form(&[(field::EXTENSION_DIRECTORY, "")]), &settings).unwrap();
        assert_eq!(cleared.extension_directory(), None);
        assert_eq!(
            cleared.summon_shortcut(),
            "ctrl-space",
            "fields that were not submitted keep their values"
        );
    }

    #[test]
    fn test_from_form_reports_every_invalid_field() {
        let errors = from_form(
            &form(&[
                (field::SUMMON_SHORTCUT, "k"),
                (field::APPEARANCE, "sepia"),
                (field::EXTENSION_DIRECTORY, "/no/such/launcher/extensions"),
            ]),
            &Settings::default(),
        )
        .unwrap_err();
        assert_eq!(
            errors.keys().copied().collect::<Vec<_>>(),
            [
                field::APPEARANCE,
                field::EXTENSION_DIRECTORY,
                field::SUMMON_SHORTCUT
            ]
        );
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
