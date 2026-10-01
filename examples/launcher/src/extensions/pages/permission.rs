use std::{fmt::Write as _, path::PathBuf, rc::Rc};

use gpui_kit::{Context, SharedString, Window};

use super::{continue_launch, show_failure};
use crate::{
    extensions::{
        Extension, LaunchRequest,
        host::Services,
        permissions::{PermissionQuestion, RequestedCapabilities, Severity},
    },
    model::{
        Action, ActionPanel, DetailModel, Effect, Metadata, MetadataValue, PageModel, RunHandler,
    },
    pages::Page,
};

/// Asks whether an extension may do what its `gpui-shell.json` requests,
/// before its code first runs, and again when an update asks for more.
pub struct PermissionPage {
    id: SharedString,
    name: SharedString,
    root: PathBuf,
    version: Option<String>,
    requested: Rc<RequestedCapabilities>,
    question: PermissionQuestion,
    services: Services,
    request: LaunchRequest,
}

impl PermissionPage {
    pub fn new(
        extension: &Extension,
        requested: RequestedCapabilities,
        question: PermissionQuestion,
        services: Services,
        request: LaunchRequest,
    ) -> Self {
        let version = gpui_shell::plugin::PluginManifest::read(extension.root())
            .ok()
            .map(|manifest| manifest.version().to_owned());
        Self {
            id: extension.id().clone(),
            name: extension.name().clone(),
            root: extension.root().to_path_buf(),
            version,
            requested: Rc::new(requested),
            question,
            services,
            request,
        }
    }

    fn markdown(&self) -> String {
        let name = &self.name;
        let mut text = String::new();
        if self.question.is_first_time() {
            writeln!(text, "## Allow “{name}” to run?\n").ok();
        } else {
            writeln!(text, "## Allow “{name}” to do more?\n").ok();
            writeln!(
                text,
                "“{name}” changed and now asks for more than you allowed before. \
                 New requests are marked **New**.\n"
            )
            .ok();
        }

        let (severe, normal): (Vec<_>, Vec<_>) = self
            .question
            .requested()
            .iter()
            .partition(|item| item.severity() == Severity::High);
        let line = |item: &crate::extensions::permissions::CapabilityRequest| {
            if !self.question.is_first_time() && self.question.is_new(item) {
                format!("- {} — **New**\n", item.description())
            } else {
                format!("- {}\n", item.description())
            }
        };
        if !severe.is_empty() {
            writeln!(text, "### Full access to your computer\n").ok();
            for item in &severe {
                text.push_str(&line(item));
            }
            writeln!(
                text,
                "\nThis is the same as running a program you downloaded. \
                 Allow it only if you trust the extension’s author.\n"
            )
            .ok();
        }
        if !normal.is_empty() {
            writeln!(
                text,
                "{}\n",
                if severe.is_empty() {
                    "It asks to:"
                } else {
                    "It also asks to:"
                }
            )
            .ok();
            for item in &normal {
                text.push_str(&line(item));
            }
            text.push('\n');
        }
        if self.requested.has_storage() {
            writeln!(
                text,
                "Like every extension, it can keep its own settings.\n"
            )
            .ok();
        }
        writeln!(
            text,
            "If you don’t allow this, the command still opens, and whatever needs \
             these permissions fails."
        )
        .ok();
        text
    }

    fn decide(&self, allow: bool) -> RunHandler {
        let (id, requested) = (self.id.clone(), self.requested.clone());
        let (services, request) = (self.services.clone(), self.request.clone());
        RunHandler::new(move |(), _, cx| {
            match services.permissions.decide(&id, &requested, allow) {
                Ok(_) => continue_launch(&services, &request, cx),
                Err(error) => show_failure(&services, "Couldn’t save the decision", &error, cx),
            }
        })
    }
}

impl Page for PermissionPage {
    fn title(&self) -> SharedString {
        "Permissions".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let detail = DetailModel::new(self.markdown())
            .with_metadata(Metadata::new(
                "Extension",
                MetadataValue::Text(self.name.clone()),
            ))
            .with_metadata(Metadata::new("ID", MetadataValue::Text(self.id.clone())));
        let detail = match &self.version {
            Some(version) => detail.with_metadata(Metadata::new(
                "Version",
                MetadataValue::Text(version.clone().into()),
            )),
            None => detail,
        };
        let detail = detail
            .with_metadata(Metadata::new(
                "Location",
                MetadataValue::Text(self.root.display().to_string().into()),
            ))
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Allow", Effect::Run(self.decide(true))))
                    .with_action(Action::new("Don’t Allow", Effect::Run(self.decide(false)))),
            );
        PageModel::Detail(detail)
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
