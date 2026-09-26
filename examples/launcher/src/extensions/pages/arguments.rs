use std::collections::BTreeMap;

use gpui_kit::{App, Context, SharedString, WeakEntity, Window};

use super::{REQUIRED, continue_launch, submitted_text};
use crate::{
    extensions::{
        ArgumentInput, ArgumentManifest, ExtensionCommand, LaunchRequest, host::Services,
    },
    model::{
        Action, ActionPanel, Control, Effect, Field, FormHandler, FormModel, FormValues, PageModel,
    },
    pages::Page,
};

/// Collects a command's arguments when it was opened without the required
/// ones, then opens it with them.
pub struct ArgumentsPage {
    title: SharedString,
    arguments: Vec<ArgumentManifest>,
    request: LaunchRequest,
    services: Services,
    errors: BTreeMap<SharedString, SharedString>,
}

impl ArgumentsPage {
    pub fn new(command: &ExtensionCommand, request: LaunchRequest, services: Services) -> Self {
        Self {
            title: command.title().clone(),
            arguments: command.arguments().to_vec(),
            request,
            services,
            errors: BTreeMap::new(),
        }
    }

    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        self.errors.clear();
        let mut request = LaunchRequest::new(self.request.command().clone());
        for (name, value) in self.request.arguments() {
            request = request.with_argument(name.clone(), value.clone());
        }
        for argument in &self.arguments {
            let value = submitted_text(&values, &argument.name)
                .or_else(|| {
                    self.request
                        .arguments()
                        .get(argument.name.as_str())
                        .cloned()
                })
                .unwrap_or_default();
            if argument.required && value.trim().is_empty() {
                self.errors
                    .insert(argument.name.clone().into(), REQUIRED.into());
            }
            request = request.with_argument(argument.name.clone(), value);
        }
        if self.errors.is_empty() {
            continue_launch(&self.services, &request, cx);
        } else {
            cx.notify();
        }
    }
}

fn submit_handler(page: WeakEntity<ArgumentsPage>) -> FormHandler {
    FormHandler::new(move |values, _, cx: &mut App| {
        page.update(cx, |page, cx| page.submit(values, cx)).ok();
    })
}

impl Page for ArgumentsPage {
    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let form = self
            .arguments
            .iter()
            .fold(FormModel::new(), |form, argument| {
                let value = self
                    .request
                    .arguments()
                    .get(argument.name.as_str())
                    .cloned()
                    .unwrap_or_default();
                let placeholder = Some(argument.placeholder.clone().into());
                let control = match argument.input {
                    ArgumentInput::Text => Control::Text { placeholder, value },
                    ArgumentInput::Password => Control::Password { placeholder, value },
                };
                let field =
                    Field::new(argument.name.clone(), argument.placeholder.clone(), control);
                form.with_field(match self.errors.get(argument.name.as_str()) {
                    Some(error) => field.with_error(error.clone()),
                    None => field,
                })
            });
        PageModel::Form(
            form.with_actions(ActionPanel::new().with_action(Action::new(
                "Open Command",
                Effect::SubmitForm(submit_handler(cx.entity().downgrade())),
            ))),
        )
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
