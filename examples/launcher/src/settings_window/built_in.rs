//! The settings of the launcher's own features, as the Extensions settings
//! show them beside the feature's commands.

use std::rc::Rc;

use gpui_kit::SharedString;

use super::inventory::Feature;
use crate::shell::settings::{
    ClipboardAction, DecimalSeparator, RETENTION_DAYS, Settings, WINDOW_GAPS, expand_home,
    parse_app_list,
};

/// How a setting is edited.
#[derive(Clone)]
pub enum Control {
    Switch {
        value: Rc<dyn Fn(&Settings) -> bool>,
        set: Rc<dyn Fn(Settings, bool) -> Settings>,
    },
    Dropdown {
        /// Values and titles.
        choices: Vec<(SharedString, SharedString)>,
        value: Rc<dyn Fn(&Settings) -> SharedString>,
        set: Rc<dyn Fn(Settings, &str) -> Settings>,
    },
    /// Saved when the field loses focus or Enter is pressed; an error keeps
    /// the old value.
    Text {
        placeholder: SharedString,
        multiline: bool,
        value: Rc<dyn Fn(&Settings) -> String>,
        set: Rc<dyn Fn(Settings, &str) -> Result<Settings, String>>,
    },
}

#[derive(Clone)]
pub struct Preference {
    /// Unique within the launcher's settings, for keeping a field's state.
    pub id: &'static str,
    pub title: SharedString,
    pub description: Option<SharedString>,
    pub control: Control,
}

fn switch(
    id: &'static str,
    title: &str,
    value: fn(&Settings) -> bool,
    set: fn(Settings, bool) -> Settings,
) -> Preference {
    Preference {
        id,
        title: title.to_owned().into(),
        description: None,
        control: Control::Switch {
            value: Rc::new(value),
            set: Rc::new(set),
        },
    }
}

fn dropdown(
    id: &'static str,
    title: &str,
    choices: Vec<(SharedString, SharedString)>,
    value: impl Fn(&Settings) -> SharedString + 'static,
    set: impl Fn(Settings, &str) -> Settings + 'static,
) -> Preference {
    Preference {
        id,
        title: title.to_owned().into(),
        description: None,
        control: Control::Dropdown {
            choices,
            value: Rc::new(value),
            set: Rc::new(set),
        },
    }
}

fn text(
    id: &'static str,
    title: &str,
    placeholder: &str,
    multiline: bool,
    value: impl Fn(&Settings) -> String + 'static,
    set: impl Fn(Settings, &str) -> Result<Settings, String> + 'static,
) -> Preference {
    Preference {
        id,
        title: title.to_owned().into(),
        description: None,
        control: Control::Text {
            placeholder: placeholder.to_owned().into(),
            multiline,
            value: Rc::new(value),
            set: Rc::new(set),
        },
    }
}

impl Preference {
    fn describe(mut self, description: &str) -> Self {
        self.description = Some(description.to_owned().into());
        self
    }
}

/// One entry per line, blank lines dropped.
fn lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The settings of `feature`; empty when it has none.
pub fn preferences(feature: Feature) -> Vec<Preference> {
    match feature {
        Feature::ClipboardHistory => vec![
            switch(
                "clipboard_history",
                "Record clipboard history",
                Settings::is_recording_clipboard,
                Settings::with_clipboard_history,
            ),
            dropdown(
                "clipboard_retention",
                "Keep history for",
                RETENTION_DAYS
                    .iter()
                    .map(|(days, title)| (days.to_string().into(), (*title).into()))
                    .collect(),
                |settings| {
                    settings
                        .clipboard_retention_days()
                        .unwrap_or(0)
                        .to_string()
                        .into()
                },
                |settings, value| match value.parse() {
                    Ok(days) => settings.with_clipboard_retention_days(days),
                    Err(_) => settings,
                },
            ),
            dropdown(
                "clipboard_action",
                "Primary action",
                ClipboardAction::ALL
                    .iter()
                    .map(|action| (action.value().into(), action.title().into()))
                    .collect(),
                |settings| settings.clipboard_action().value().into(),
                |settings, value| match ClipboardAction::from_value(value) {
                    Some(action) => settings.with_clipboard_action(action),
                    None => settings,
                },
            )
            .describe("What Enter does to the selected entry."),
            text(
                "clipboard_ignored_apps",
                "Ignore copies from",
                "keepass, 1password",
                false,
                |settings| settings.clipboard_ignored_apps().join(", "),
                |settings, value| Ok(settings.with_clipboard_ignored_apps(parse_app_list(value))),
            )
            .describe("Applications by executable name, separated by commas."),
        ],
        Feature::Snippets => vec![
            switch(
                "snippet_expansion",
                "Expand keywords as you type",
                Settings::expands_snippets,
                Settings::with_snippet_expansion,
            ),
            text(
                "snippet_ignored_apps",
                "Don’t expand in",
                "code, windowsterminal",
                false,
                |settings| settings.snippet_ignored_apps().join(", "),
                |settings, value| Ok(settings.with_snippet_ignored_apps(parse_app_list(value))),
            )
            .describe("Applications by executable name, separated by commas."),
        ],
        Feature::Calendar => vec![
            text(
                "calendar_feeds",
                "Calendars",
                "https://example.com/calendar.ics",
                true,
                |settings| settings.calendar_feeds().join("\n"),
                |settings, value| {
                    let feeds = lines(value);
                    match feeds.iter().find(|feed| {
                        !(feed.starts_with("https://")
                            || feed.starts_with("http://")
                            || feed.starts_with("webcal://")
                            || expand_home(feed).is_file())
                    }) {
                        Some(invalid) => {
                            Err(format!("“{invalid}” is neither a web address nor a file."))
                        }
                        None => Ok(settings.with_calendar_feeds(feeds)),
                    }
                },
            )
            .describe("iCalendar feeds or .ics files, one per line, for My Schedule."),
        ],
        Feature::WindowManagement => vec![dropdown(
            "window_gap",
            "Gap between windows",
            WINDOW_GAPS
                .iter()
                .map(|(gap, title)| (gap.to_string().into(), (*title).into()))
                .collect(),
            |settings| settings.window_gap().to_string().into(),
            |settings, value| match value.parse() {
                Ok(gap) => settings.with_window_gap(gap),
                Err(_) => settings,
            },
        )],
        Feature::SearchFiles => vec![
            text(
                "file_search_roots",
                "Folders to search",
                "~",
                true,
                |settings| settings.file_search_roots().join("\n"),
                |settings, value| {
                    let roots = lines(value);
                    match roots.iter().find(|root| !expand_home(root).is_dir()) {
                        Some(missing) => Err(format!("No folder exists at “{missing}”.")),
                        None => Ok(settings.with_file_search_roots(roots)),
                    }
                },
            )
            .describe("One per line. Leave empty to search your home folder."),
            text(
                "file_search_excluded",
                "Skip folders named",
                "node_modules, target",
                false,
                |settings| settings.file_search_excluded().join(", "),
                |settings, value| {
                    Ok(settings.with_file_search_excluded(
                        value
                            .split([',', '\n'])
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned)
                            .collect(),
                    ))
                },
            ),
        ],
        Feature::Quicklinks => vec![
            text(
                "quicklink_browser",
                "Open web links in",
                "System default",
                false,
                |settings| settings.quicklink_browser().unwrap_or_default().to_owned(),
                |settings, value| {
                    let value = value.trim();
                    Ok(settings
                        .with_quicklink_browser((!value.is_empty()).then(|| value.to_owned())))
                },
            )
            .describe("A browser’s name or path, such as firefox. Leave empty for the default."),
        ],
        Feature::Calculator => vec![dropdown(
            "decimal_separator",
            "Decimal separator",
            DecimalSeparator::ALL
                .iter()
                .map(|separator| (separator.value().into(), separator.title().into()))
                .collect(),
            |settings| settings.decimal_separator().value().into(),
            |settings, value| match DecimalSeparator::from_value(value) {
                Some(separator) => settings.with_decimal_separator(separator),
                None => settings,
            },
        )],
        Feature::Launcher
        | Feature::FloatingNotes
        | Feature::Focus
        | Feature::Media
        | Feature::Timers
        | Feature::ScriptCommands
        | Feature::Utilities
        | Feature::System
        | Feature::SystemSettings => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(feature: Feature, id: &str, value: &str) -> Result<Settings, String> {
        let preference = preferences(feature)
            .into_iter()
            .find(|preference| preference.id == id)
            .unwrap();
        let settings = Settings::default();
        match preference.control {
            Control::Text { set, .. } => set(settings, value),
            Control::Dropdown { set, .. } => Ok(set(settings, value)),
            Control::Switch { set, .. } => Ok(set(settings, value == "true")),
        }
    }

    #[test]
    fn test_built_in_preferences_parse_and_validate() {
        let settings = apply(
            Feature::ClipboardHistory,
            "clipboard_ignored_apps",
            "KeePass.exe, 1password",
        )
        .unwrap();
        assert_eq!(settings.clipboard_ignored_apps(), ["keepass", "1password"]);

        let settings = apply(Feature::ClipboardHistory, "clipboard_retention", "7").unwrap();
        assert_eq!(settings.clipboard_retention_days(), Some(7));

        let settings = apply(Feature::ClipboardHistory, "clipboard_action", "copy").unwrap();
        assert_eq!(settings.clipboard_action(), ClipboardAction::Copy);

        assert!(apply(Feature::Calendar, "calendar_feeds", "not a feed").is_err());
        let settings = apply(
            Feature::Calendar,
            "calendar_feeds",
            "https://a.example/cal.ics\n\n webcal://b.example ",
        )
        .unwrap();
        assert_eq!(
            settings.calendar_feeds(),
            ["https://a.example/cal.ics", "webcal://b.example"]
        );

        assert!(
            apply(
                Feature::SearchFiles,
                "file_search_roots",
                "/no/such/launcher/folder"
            )
            .is_err()
        );
        let settings = apply(Feature::SearchFiles, "file_search_excluded", "a, b,\nc").unwrap();
        assert_eq!(settings.file_search_excluded(), ["a", "b", "c"]);

        let settings = apply(Feature::Quicklinks, "quicklink_browser", "  ").unwrap();
        assert_eq!(settings.quicklink_browser(), None);
    }
}
