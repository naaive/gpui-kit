use gpui_kit::{
    Context, Entity, IntoElement as _, Render as _, SharedString, Subscription, Window,
};
use gpui_shell::ScriptView;

use super::Page;
use crate::{extensions::ScriptModel, model::PageModel};

/// An extension command's page.
///
/// The command's `ScriptView` is never mounted in the window. This page renders
/// it directly and takes the model out of what it returns, so the launcher
/// draws everything and the callbacks in the model always belong to the
/// current render. The model is rebuilt only when the script asked to be
/// rendered again.
pub struct ScriptPage {
    title: SharedString,
    view: Entity<ScriptView>,
    model: Option<ScriptModel>,
    _observe: Subscription,
}

impl ScriptPage {
    pub fn new(title: SharedString, view: Entity<ScriptView>, cx: &mut Context<Self>) -> Self {
        let _observe = cx.observe(&view, |page, _, cx| {
            page.model = None;
            cx.notify();
        });
        Self {
            title,
            view,
            model: None,
            _observe,
        }
    }

    fn build(&self, window: &mut Window, cx: &mut Context<Self>) -> ScriptModel {
        let mut element = self
            .view
            .update(cx, |view, cx| view.render(window, cx).into_any_element());
        if let Some(error) = self.view.read(cx).build_error() {
            return ScriptModel::failure("The command failed", error.to_owned());
        }
        ScriptModel::take(&mut element)
            .unwrap_or_else(|reason| ScriptModel::failure("The command returned no page", reason))
    }
}

impl Page for ScriptPage {
    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn model(&mut self, window: &mut Window, cx: &mut Context<Self>) -> PageModel {
        if self.model.is_none() || self.view.read(cx).is_dirty() {
            self.model = Some(self.build(window, cx));
        }
        self.model
            .as_ref()
            .map(|model| model.page().clone())
            .unwrap_or_else(|| PageModel::failure("The command returned no page", ""))
    }

    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handler) = self
            .model
            .as_ref()
            .and_then(|model| model.on_query_change().cloned())
        {
            handler.call(query, window, cx);
        }
    }
}
