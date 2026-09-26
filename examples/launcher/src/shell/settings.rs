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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    /// A GPUI keystroke, such as `alt-space`.
    summon_shortcut: String,
    appearance: Appearance,
    /// Loaded ahead of the bundled extensions.
    extension_directory: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            summon_shortcut: DEFAULT_SHORTCUT.into(),
            appearance: Appearance::default(),
            extension_directory: None,
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

    pub fn extension_directory(&self) -> Option<&Path> {
        self.extension_directory.as_deref()
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
    pub const EXTENSION_DIRECTORY: &str = "extension_directory";
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

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Settings {
        summon_shortcut,
        appearance,
        extension_directory,
    })
}

/// Expands a leading `~` to the home directory, as a shell would.
fn expand_home(path: &str) -> PathBuf {
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
