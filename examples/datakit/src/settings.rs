//! What a person chose for DataKit as a whole: its appearance, its
//! language, how SQL is formatted. Kept in `settings.json`.

use std::path::PathBuf;

use gpui_kit::component::{
    IndexPath, Theme, ThemeMode, WindowExt as _,
    checkbox::Checkbox,
    form::{field, v_form},
    select::{Select, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _, Render,
    SharedString, Subscription, Window, rems,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use crate::services::Services;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    #[default]
    System,
    English,
    SimplifiedChinese,
    TraditionalChinese,
}

impl Language {
    /// The locale the interface uses, `system` being the system's.
    fn locale(self, system: &'static str) -> &'static str {
        match self {
            Language::System => system,
            Language::English => "en",
            Language::SimplifiedChinese => "zh-CN",
            Language::TraditionalChinese => "zh-HK",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    appearance: Appearance,
    language: Language,
    uppercase_keywords: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            appearance: Appearance::System,
            language: Language::System,
            uppercase_keywords: true,
        }
    }
}

impl Global for Settings {}

/// The locale of the system, kept for when the person chooses it again.
struct SystemLocale(&'static str);

impl Global for SystemLocale {}

impl Settings {
    /// Read the settings and apply the language. The workspace applies the
    /// appearance, and applies any later change.
    pub fn init(system_locale: &'static str, cx: &mut App) {
        let settings: Settings = std::fs::read(Self::path(cx))
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default();
        rust_i18n::set_locale(settings.language.locale(system_locale));
        cx.set_global(SystemLocale(system_locale));
        cx.set_global(settings);
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    fn path(cx: &App) -> PathBuf {
        Services::global(cx).data_directory().join("settings.json")
    }

    pub fn uppercase_keywords(&self) -> bool {
        self.uppercase_keywords
    }

    /// Whether the window follows the system's light or dark appearance.
    pub fn follows_system_appearance(&self) -> bool {
        self.appearance == Appearance::System
    }

    /// Show the chosen appearance in `window`.
    pub fn apply_appearance(window: &mut Window, cx: &mut App) {
        match Self::global(cx).appearance {
            Appearance::System => Theme::sync_system_appearance(Some(window), cx),
            Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
            Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        }
        window.refresh();
    }

    fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
        let mut settings = Self::global(cx).clone();
        change(&mut settings);
        let path = Self::path(cx);
        let json = serde_json::to_vec_pretty(&settings).unwrap_or_default();
        cx.set_global(settings);
        cx.background_spawn(async move {
            if let Some(directory) = path.parent() {
                let _ = std::fs::create_dir_all(directory);
            }
            if let Err(error) = std::fs::write(&path, json) {
                tracing::error!("couldn’t save the settings: {error}");
            }
        })
        .detach();
    }
}

#[derive(Clone)]
struct Choice<T: Clone + PartialEq + 'static> {
    value: T,
    title: SharedString,
}

impl<T: Clone + PartialEq + 'static> SelectItem for Choice<T> {
    type Value = T;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &T {
        &self.value
    }
}

/// The settings, in a dialog.
pub struct SettingsDialog {
    appearance: Entity<SelectState<Vec<Choice<Appearance>>>>,
    language: Entity<SelectState<Vec<Choice<Language>>>>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsDialog {
    pub fn open(window: &mut Window, cx: &mut App) {
        let dialog = cx.new(|cx| Self::new(window, cx));
        let width = rems(28.).to_pixels(window.rem_size());
        window.open_dialog(cx, move |modal, _, _| {
            modal
                .title(t!("settings.title").to_string())
                .w(width)
                .child(dialog.clone())
        });
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::global(cx).clone();
        let appearances = vec![
            Choice {
                value: Appearance::System,
                title: t!("settings.appearance_system").into(),
            },
            Choice {
                value: Appearance::Light,
                title: t!("settings.appearance_light").into(),
            },
            Choice {
                value: Appearance::Dark,
                title: t!("settings.appearance_dark").into(),
            },
        ];
        let languages = vec![
            Choice {
                value: Language::System,
                title: t!("settings.language_system").into(),
            },
            Choice {
                value: Language::English,
                title: "English".into(),
            },
            Choice {
                value: Language::SimplifiedChinese,
                title: "简体中文".into(),
            },
            Choice {
                value: Language::TraditionalChinese,
                title: "繁體中文".into(),
            },
        ];
        let selected_appearance = appearances
            .iter()
            .position(|choice| choice.value == settings.appearance)
            .map(IndexPath::new);
        let selected_language = languages
            .iter()
            .position(|choice| choice.value == settings.language)
            .map(IndexPath::new);
        let appearance =
            cx.new(|cx| SelectState::new(appearances, selected_appearance, window, cx));
        let language = cx.new(|cx| SelectState::new(languages, selected_language, window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &appearance,
                window,
                |_, _, event: &SelectEvent<Vec<Choice<Appearance>>>, _, cx| {
                    if let SelectEvent::Confirm(Some(appearance)) = event {
                        let appearance = *appearance;
                        Settings::update(cx, |settings| settings.appearance = appearance);
                    }
                },
            ),
            cx.subscribe_in(
                &language,
                window,
                |_, _, event: &SelectEvent<Vec<Choice<Language>>>, _, cx| {
                    if let SelectEvent::Confirm(Some(language)) = event {
                        let language = *language;
                        // The language is set before the settings change,
                        // so whatever observes them speaks it.
                        let system = cx.global::<SystemLocale>().0;
                        rust_i18n::set_locale(language.locale(system));
                        Settings::update(cx, |settings| settings.language = language);
                    }
                },
            ),
        ];
        Self {
            appearance,
            language,
            _subscriptions: subscriptions,
        }
    }
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let uppercase = Settings::global(cx).uppercase_keywords;
        v_form()
            .child(
                field()
                    .label(t!("settings.appearance").to_string())
                    .child(Select::new(&self.appearance)),
            )
            .child(
                field()
                    .label(t!("settings.language").to_string())
                    .child(Select::new(&self.language)),
            )
            .child(
                field().child(
                    Checkbox::new("uppercase-keywords")
                        .label(t!("settings.uppercase_keywords").to_string())
                        .checked(uppercase)
                        .on_click(cx.listener(|_, checked: &bool, _, cx| {
                            let checked = *checked;
                            Settings::update(cx, |settings| settings.uppercase_keywords = checked);
                            cx.notify();
                        })),
                ),
            )
    }
}
