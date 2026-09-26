use gpui_kit::{Context, Entity, SharedString, Subscription, Window};
use gpui_shell::ScriptView;

use super::Page;
use crate::{
    extensions::{render_for_extension, take_page_model},
    model::PageModel,
};

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
    model: Option<PageModel>,
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

    fn build(&self, window: &mut Window, cx: &mut Context<Self>) -> PageModel {
        // Rendering inside the extension's identity lets `Action.launch` name
        // a sibling command by its bare name, whichever page is on top. The
        // window asks for a model from its event handlers too, outside any
        // frame, so the view is asked for its description, never for its own
        // failure surface.
        let extension: SharedString = self.view.read(cx).policy().application().to_owned().into();
        let element = render_for_extension(&extension, || {
            self.view
                .update(cx, |view, cx| view.render_description(window, cx))
        });
        match element {
            Ok(mut element) => take_page_model(&mut element).unwrap_or_else(|reason| {
                PageModel::failure("The command returned no page", reason)
            }),
            Err(error) => PageModel::failure("The command failed", error),
        }
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
            .clone()
            .unwrap_or_else(|| PageModel::failure("The command returned no page", ""))
    }

    /// Renders the script again: a page pushed over this one runs in the same
    /// script (an edit form saving a note), and GPUI Shell's `cx.notify()`
    /// reaches only the view whose code is running, which was the page above.
    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| view.refresh(cx));
    }

    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let handler = match &self.model {
            Some(PageModel::List(list)) => list.on_query_change().cloned(),
            _ => None,
        };
        if let Some(handler) = handler {
            handler.call(query.to_owned().into(), window, cx);
        }
    }
}
