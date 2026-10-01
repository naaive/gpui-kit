use std::collections::BTreeMap;

use anyhow::Result;
use gpui_kit::{App, Context, SharedString, WeakEntity, Window};
use serde_json::Value;

use super::{REQUIRED, continue_launch, show_failure};
use crate::{
    extensions::{
        LaunchRequest, PreferenceInput, PreferenceManifest,
        host::Services,
        preferences::{PreferenceScope, is_empty},
    },
    model::{
        Action, ActionPanel, Choice, Control, Effect, Field, FormHandler, FormModel, FormValue,
        FormValues, PageModel,
    },
    pages::Page,
};

/// Edits an extension's preferences, as declared in its `launcher.json`.
///
/// Opened before a command whose required preferences are missing, and then
/// continues to the command; or opened from the extension manager to change
/// them later.
pub struct PreferencesPage {
    title: SharedString,
    sections: Vec<(PreferenceScope, Vec<PreferenceManifest>)>,
    services: Services,
    /// The command to open once the preferences are saved.
    then: Option<LaunchRequest>,
    errors: BTreeMap<SharedString, SharedString>,
}

impl PreferencesPage {
    pub fn new(
        title: impl Into<SharedString>,
        sections: Vec<(PreferenceScope, Vec<PreferenceManifest>)>,
        services: Services,
        then: Option<LaunchRequest>,
    ) -> Self {
        Self {
            title: title.into(),
            sections,
            services,
            then,
            errors: BTreeMap::new(),
        }
    }

    fn declarations(&self) -> impl Iterator<Item = (&PreferenceScope, &PreferenceManifest)> {
        self.sections.iter().flat_map(|(scope, declarations)| {
            declarations
                .iter()
                .map(move |declaration| (scope, declaration))
        })
    }

    fn field(&self, scope: &PreferenceScope, declaration: &PreferenceManifest) -> Result<Field> {
        let id = field_id(scope, declaration);
        let value = self.services.preferences.value(scope, declaration)?;
        let text = || -> SharedString {
            value
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
                .into()
        };
        let placeholder = declaration.description.clone().map(SharedString::from);
        let control = match declaration.input {
            PreferenceInput::Text => Control::Text {
                placeholder,
                value: text(),
            },
            PreferenceInput::Password => Control::Password {
                placeholder,
                value: text(),
            },
            PreferenceInput::Checkbox => Control::Checkbox {
                label: declaration
                    .label
                    .clone()
                    .unwrap_or_else(|| declaration.title.clone())
                    .into(),
                value: value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            },
            PreferenceInput::Dropdown => Control::Dropdown {
                choices: declaration
                    .choices
                    .iter()
                    .map(|choice| Choice::new(choice.value.clone(), choice.title.clone()))
                    .collect(),
                value: value
                    .as_ref()
                    .and_then(Value::as_str)
                    .map(|value| value.to_owned().into()),
            },
        };
        let field = Field::new(id.clone(), declaration.title.clone(), control);
        let field = match (&declaration.description, declaration.input) {
            // A checkbox has no placeholder to carry the description.
            (Some(description), PreferenceInput::Checkbox) => field.with_info(description.clone()),
            _ => field,
        };
        Ok(match self.errors.get(&id) {
            Some(error) => field.with_error(error.clone()),
            None => field,
        })
    }

    /// Validates and saves the submitted values, then continues.
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let mut changes = Vec::new();
        let mut errors = BTreeMap::new();
        for (scope, declaration) in self.declarations() {
            let id = field_id(scope, declaration);
            let submitted = values.get(&id).map(|value| match value {
                FormValue::Text(text) if text.trim().is_empty() => None,
                FormValue::Text(text) => Some(Value::String(text.to_string())),
                FormValue::Bool(value) => Some(Value::Bool(*value)),
                FormValue::Empty | FormValue::List(_) => None,
            });
            // A field the form did not report keeps its value.
            let reported = submitted.is_some();
            let value = match submitted {
                Some(value) => value,
                None => match self.services.preferences.stored(scope, declaration) {
                    Ok(value) => value,
                    Err(error) => {
                        show_failure(&self.services, "Couldn’t read preferences", &error, cx);
                        return;
                    }
                },
            };
            let effective = value.clone().or_else(|| declaration.default.clone());
            if declaration.required && is_empty(effective.as_ref()) {
                errors.insert(id, SharedString::from(REQUIRED));
            }
            if reported {
                changes.push((scope.clone(), declaration.clone(), value));
            }
        }
        self.errors = errors;
        if !self.errors.is_empty() {
            cx.notify();
            return;
        }
        for (scope, declaration, value) in changes {
            if let Err(error) = self.services.preferences.set(&scope, &declaration, value) {
                show_failure(&self.services, "Couldn’t save preferences", &error, cx);
                return;
            }
        }
        match &self.then {
            Some(request) => continue_launch(&self.services, request, cx),
            None => self.services.effects.request(Effect::Pop, cx),
        }
    }
}

fn field_id(scope: &PreferenceScope, declaration: &PreferenceManifest) -> SharedString {
    format!("{}:{}", scope.key(), declaration.name).into()
}

fn submit_handler(page: WeakEntity<PreferencesPage>) -> FormHandler {
    FormHandler::new(move |values, _, cx: &mut App| {
        page.update(cx, |page, cx| page.submit(values, cx)).ok();
    })
}

impl Page for PreferencesPage {
    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let fields: Result<Vec<Field>> = self
            .declarations()
            .map(|(scope, declaration)| self.field(scope, declaration))
            .collect();
        let fields = match fields {
            Ok(fields) => fields,
            Err(error) => {
                return PageModel::failure("Couldn’t read preferences", format!("{error:#}"));
            }
        };
        let save = Action::new(
            if self.then.is_some() {
                "Continue"
            } else {
                "Save"
            },
            Effect::SubmitForm(submit_handler(cx.entity().downgrade())),
        );
        PageModel::Form(
            fields
                .into_iter()
                .fold(FormModel::new(), FormModel::with_field)
                .with_actions(ActionPanel::new().with_action(save)),
        )
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
