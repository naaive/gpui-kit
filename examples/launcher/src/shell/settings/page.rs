use gpui_kit::{
    App, AppContext as _, Context, Entity, SharedString, Window,
    component::{
        WindowExt as _,
        notification::{Notification, NotificationType},
    },
};

use super::{
    Appearance, FieldErrors, PopToRoot, RETENTION_DAYS, WINDOW_GAPS, WindowMode, field, from_form,
};
use crate::{
    model::{
        Action, ActionPanel, Choice, Control, Effect, Field, FormHandler, FormModel, FormValue,
        FormValues, PageModel,
    },
    pages::Page,
    shell::{hotkey::HotkeyStatus, launcher},
};

/// The settings form, a built-in Form page like any extension's.
///
/// Saved settings are the launcher's, not the page's: the page reads them on
/// every render and keeps only what the user submitted last, so a rejected
/// submission shows the typed values beside their errors.
pub struct SettingsPage {
    draft: Option<FormValues>,
    errors: FieldErrors,
}

impl SettingsPage {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            draft: None,
            errors: FieldErrors::new(),
        })
    }

    /// Validates, saves and applies a submitted form.
    fn submit(&mut self, values: FormValues, window: &mut Window, cx: &mut Context<Self>) {
        let current = launcher::settings(cx);
        match from_form(&values, &current) {
            Err(errors) => {
                self.draft = Some(values);
                self.errors = errors;
            }
            Ok(settings) => {
                self.draft = None;
                self.errors.clear();
                let notification = match launcher::update_settings(settings, window, cx) {
                    Ok(()) => Notification::new()
                        .title("Settings saved")
                        .with_type(NotificationType::Success),
                    Err(error) => Notification::new()
                        .title("Couldn’t save settings")
                        .message(format!("{error:#}"))
                        .with_type(NotificationType::Error),
                };
                window.push_notification(notification, cx);
            }
        }
        cx.notify();
    }

    /// The value a text field shows: the rejected draft, else the saved value.
    fn text(&self, id: &str, saved: impl Into<SharedString>) -> SharedString {
        match self.draft.as_ref().and_then(|draft| draft.get(id)) {
            Some(FormValue::Text(text)) => text.clone(),
            Some(FormValue::Empty) => SharedString::default(),
            Some(FormValue::Bool(_)) | None => saved.into(),
        }
    }

    fn with_error(&self, field: Field) -> Field {
        match self.errors.get(field.id().as_ref()) {
            Some(error) => field.with_error(error.clone()),
            None => field,
        }
    }
}

impl Page for SettingsPage {
    fn title(&self) -> SharedString {
        "Settings".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let settings = launcher::settings(cx);
        let status = launcher::hotkey_status(cx);
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, window, cx| {
            page.update(cx, |page, cx| page.submit(values, window, cx))
                .ok();
        });

        let shortcut = Field::new(
            field::SUMMON_SHORTCUT,
            "Summon shortcut",
            Control::Text {
                placeholder: Some(super::DEFAULT_SHORTCUT.into()),
                value: self.text(
                    field::SUMMON_SHORTCUT,
                    settings.summon_shortcut().to_owned(),
                ),
            },
        );
        // A refused registration belongs next to the shortcut it refused,
        // unless the field already has a validation error to show.
        let shortcut = match (&status, self.errors.contains_key(field::SUMMON_SHORTCUT)) {
            (Some(HotkeyStatus::Failed(_)), false) => {
                shortcut.with_error(status.as_ref().map(ToString::to_string).unwrap_or_default())
            }
            (Some(status), _) => shortcut.with_info(status.to_string()),
            (None, _) => shortcut,
        };

        let appearance = match self.draft.as_ref().and_then(|d| d.get(field::APPEARANCE)) {
            Some(FormValue::Text(value)) => value.clone(),
            _ => settings.appearance().value().into(),
        };
        let directory = settings
            .extension_directory()
            .map(|path| path.display().to_string())
            .unwrap_or_default();

        PageModel::Form(
            FormModel::new()
                .with_field(self.with_error(shortcut))
                .with_field(
                    self.with_error(Field::new(
                        field::APPEARANCE,
                        "Appearance",
                        Control::Dropdown {
                            choices: Appearance::ALL
                                .into_iter()
                                .map(|appearance| {
                                    Choice::new(appearance.value(), appearance.title())
                                })
                                .collect(),
                            value: Some(appearance),
                        },
                    )),
                )
                .with_field(
                    self.with_error(Field::new(
                        field::WINDOW_MODE,
                        "Window mode",
                        Control::Dropdown {
                            choices: WindowMode::ALL
                                .into_iter()
                                .map(|mode| Choice::new(mode.value(), mode.title()))
                                .collect(),
                            value: Some(
                                match self.draft.as_ref().and_then(|d| d.get(field::WINDOW_MODE)) {
                                    Some(FormValue::Text(value)) => value.clone(),
                                    _ => settings.window_mode().value().into(),
                                },
                            ),
                        },
                    ))
                    .with_info("Compact shows only the search field until you type."),
                )
                .with_field(
                    self.with_error(Field::new(
                        field::POP_TO_ROOT,
                        "Return to root search",
                        Control::Dropdown {
                            choices: PopToRoot::ALL
                                .into_iter()
                                .map(|when| Choice::new(when.value(), when.title()))
                                .collect(),
                            value: Some(
                                match self.draft.as_ref().and_then(|d| d.get(field::POP_TO_ROOT)) {
                                    Some(FormValue::Text(value)) => value.clone(),
                                    _ => settings.pop_to_root().value().into(),
                                },
                            ),
                        },
                    ))
                    .with_info(
                        "When the launcher, opened again, starts over instead of showing the \
                         command you left.",
                    ),
                )
                .with_field(
                    self.with_error(
                        Field::new(
                            field::EXTENSION_DIRECTORY,
                            "Extensions folder",
                            Control::Text {
                                placeholder: Some("None".into()),
                                value: self.text(field::EXTENSION_DIRECTORY, directory),
                            },
                        )
                        .with_info(
                            "Extensions here are loaded ahead of the bundled ones. \
                             A change takes effect the next time the launcher opens.",
                        ),
                    ),
                )
                .with_field(
                    Field::new(
                        field::CLIPBOARD_HISTORY,
                        "Clipboard history",
                        Control::Checkbox {
                            label: "Record what is copied".into(),
                            value: match self
                                .draft
                                .as_ref()
                                .and_then(|d| d.get(field::CLIPBOARD_HISTORY))
                            {
                                Some(FormValue::Bool(record)) => *record,
                                _ => settings.is_recording_clipboard(),
                            },
                        },
                    )
                    .with_info("Content a password manager marks as private is never recorded."),
                )
                .with_field(
                    Field::new(
                        field::CLIPBOARD_IGNORED_APPS,
                        "Ignore copies from",
                        Control::Text {
                            placeholder: Some("keepass, 1password".into()),
                            value: self.text(
                                field::CLIPBOARD_IGNORED_APPS,
                                settings.clipboard_ignored_apps().join(", "),
                            ),
                        },
                    )
                    .with_info("Applications by program name, separated by commas."),
                )
                .with_field(
                    self.with_error(Field::new(
                        field::CLIPBOARD_RETENTION,
                        "Keep clipboard history for",
                        Control::Dropdown {
                            choices: RETENTION_DAYS
                                .iter()
                                .map(|(days, title)| Choice::new(days.to_string(), *title))
                                .collect(),
                            value: Some(
                                match self
                                    .draft
                                    .as_ref()
                                    .and_then(|d| d.get(field::CLIPBOARD_RETENTION))
                                {
                                    Some(FormValue::Text(days)) => days.clone(),
                                    _ => settings
                                        .clipboard_retention_days()
                                        .unwrap_or(0)
                                        .to_string()
                                        .into(),
                                },
                            ),
                        },
                    )),
                )
                .with_field(
                    Field::new(
                        field::SNIPPET_EXPANSION,
                        "Snippets",
                        Control::Checkbox {
                            label: "Expand keywords as I type in any application".into(),
                            value: match self
                                .draft
                                .as_ref()
                                .and_then(|d| d.get(field::SNIPPET_EXPANSION))
                            {
                                Some(FormValue::Bool(expand)) => *expand,
                                _ => settings.expands_snippets(),
                            },
                        },
                    )
                    .with_info(if cfg!(target_os = "windows") {
                        "Typing a snippet's keyword replaces it with the snippet. Snippets that \
                         ask for arguments are not expanded."
                    } else {
                        "Available on Windows."
                    }),
                )
                .with_field(
                    Field::new(
                        field::SNIPPET_IGNORED_APPS,
                        "Don’t expand snippets in",
                        Control::Text {
                            placeholder: Some("code, windowsterminal".into()),
                            value: self.text(
                                field::SNIPPET_IGNORED_APPS,
                                settings.snippet_ignored_apps().join(", "),
                            ),
                        },
                    )
                    .with_info("Applications by program name, separated by commas."),
                )
                .with_field(
                    self.with_error(
                        Field::new(
                            field::CALENDAR_FEEDS,
                            "Calendars",
                            Control::TextArea {
                                placeholder: Some(
                                    "https://calendar.google.com/calendar/ical/…/basic.ics".into(),
                                ),
                                value: self.text(
                                    field::CALENDAR_FEEDS,
                                    settings.calendar_feeds().join("\n"),
                                ),
                            },
                        )
                        .with_info(
                            "One iCal address or .ics file per line, for My Schedule. Google \
                             Calendar: Settings → Integrate calendar → Secret address in iCal \
                             format. Outlook: Settings → Calendar → Shared calendars → Publish.",
                        ),
                    ),
                )
                .with_field(
                    self.with_error(Field::new(
                        field::WINDOW_GAP,
                        "Window gap",
                        Control::Dropdown {
                            choices: WINDOW_GAPS
                                .iter()
                                .map(|(gap, title)| Choice::new(gap.to_string(), *title))
                                .collect(),
                            value: Some(
                                match self.draft.as_ref().and_then(|d| d.get(field::WINDOW_GAP)) {
                                    Some(FormValue::Text(gap)) => gap.clone(),
                                    _ => settings.window_gap().to_string().into(),
                                },
                            ),
                        },
                    )),
                )
                .with_fields(crate::hyper_key::is_supported().then(|| {
                    self.with_error(Field::new(
                        field::HYPER_KEY,
                        "Hyper Key",
                        Control::Dropdown {
                            choices: crate::hyper_key::HyperKey::ALL
                                .into_iter()
                                .map(|key| Choice::new(key.value(), key.title()))
                                .collect(),
                            value: Some(
                                match self.draft.as_ref().and_then(|d| d.get(field::HYPER_KEY)) {
                                    Some(FormValue::Text(value)) => value.clone(),
                                    _ => settings.hyper_key().value().into(),
                                },
                            ),
                        },
                    ))
                    .with_info(
                        "Holding Caps Lock presses Ctrl+Shift+Alt+Win, for hotkeys such \
                             as hyper-k.",
                    )
                }))
                .with_actions(
                    ActionPanel::new().with_action(Action::new("Save", Effect::SubmitForm(submit))),
                ),
        )
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use crate::pages::PageHandle;

    fn field<'a>(form: &'a FormModel, id: &str) -> &'a Field {
        form.fields()
            .iter()
            .find(|field| field.id().as_ref() == id)
            .unwrap()
    }

    fn form(page: &PageHandle, window: &mut VisualTestContext) -> FormModel {
        window.update(|window, cx| match page.model(window, cx) {
            PageModel::Form(form) => form,
            _ => panic!("settings is a Form"),
        })
    }

    #[gpui::test]
    fn test_settings_page_shows_rejected_values_with_their_errors(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_empty_window();
        let page = window.update(|window, cx| super::super::settings_page(window, cx).unwrap());
        assert_eq!(window.update(|_, cx| page.title(cx)).as_ref(), "Settings");

        let initial = form(&page, window);
        assert_eq!(
            field(&initial, field::SUMMON_SHORTCUT).control(),
            &Control::Text {
                placeholder: Some("alt-space".into()),
                value: "alt-space".into()
            }
        );
        assert_eq!(
            initial.actions().primary().unwrap().title().as_ref(),
            "Save"
        );

        let Effect::SubmitForm(submit) = initial.actions().primary().unwrap().effect().clone()
        else {
            panic!("Save submits the form");
        };
        window.update(|window, cx| {
            submit.call(
                FormValues::new()
                    .with(field::SUMMON_SHORTCUT, FormValue::Text("q".into()))
                    .with(field::APPEARANCE, FormValue::Text("dark".into())),
                window,
                cx,
            )
        });

        let rejected = form(&page, window);
        let shortcut = field(&rejected, field::SUMMON_SHORTCUT);
        assert_eq!(
            shortcut.control(),
            &Control::Text {
                placeholder: Some("alt-space".into()),
                value: "q".into()
            },
            "the typed value is kept"
        );
        assert!(shortcut.error().is_some());
        assert_eq!(
            field(&rejected, field::APPEARANCE)
                .control()
                .initial_value(),
            FormValue::Text("dark".into())
        );
    }
}
