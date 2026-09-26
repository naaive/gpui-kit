//! How an extension's script becomes a [`PageModel`].
//!
//! Two modules make up the extension SDK today:
//!
//! - `launcher`: the page nodes, registered as GPUI Shell components whose
//!   materializers draw nothing and carry a model value instead;
//! - `launcher/api`: host functions returning plain data, such as `launch()`.
//!
//! The launcher renders a command's `ScriptView` itself and takes the
//! [`ScriptModel`] out of the element it returns (see
//! [`crate::pages::ScriptPage`]), so a page is produced synchronously and its
//! callbacks always belong to the current render.

mod carrier;
mod components;
mod host_api;

use std::rc::Rc;

use anyhow::{Result, anyhow};
use gpui_kit::{AnyElement, App, Window};
use gpui_shell::{ComponentRegistry, FrozenComponentRegistry};

pub(super) use host_api::HostApi;

use crate::model::PageModel;

/// The module name extensions import page nodes from.
pub const COMPONENT_MODULE: &str = "launcher";

/// The launcher's component catalog. It holds only page nodes: an extension
/// cannot draw, so no styled component is registered.
pub(super) fn components() -> Result<FrozenComponentRegistry> {
    let mut registry =
        ComponentRegistry::new(gpui_shell::COMPONENT_REGISTRY_API_VERSION, COMPONENT_MODULE)
            .map_err(|error| anyhow!("{error}"))?;
    components::register(&mut registry).map_err(|error| anyhow!("{error}"))?;
    registry.freeze().map_err(|error| anyhow!("{error}"))
}

/// What one render of an extension page produced.
#[derive(Clone)]
pub struct ScriptModel {
    page: PageModel,
    on_query_change: Option<QueryHandler>,
}

impl ScriptModel {
    fn new(page: PageModel, on_query_change: Option<QueryHandler>) -> Self {
        Self {
            page,
            on_query_change,
        }
    }

    pub fn failure(title: &str, message: impl Into<gpui_kit::SharedString>) -> Self {
        Self::new(PageModel::failure(title.to_owned(), message), None)
    }

    /// Takes the model out of the element a command's `render` produced, or
    /// explains what was returned instead.
    pub fn take(element: &mut AnyElement) -> Result<Self, &'static str> {
        carrier::take::<Self>(element)
            .ok_or("a command's `render` must return a `List` from the `launcher` module")
    }

    pub fn page(&self) -> &PageModel {
        &self.page
    }

    pub fn on_query_change(&self) -> Option<&QueryHandler> {
        self.on_query_change.as_ref()
    }
}

/// Receives the search text of a page that searches for itself.
#[derive(Clone)]
pub struct QueryHandler(Rc<dyn Fn(&str, &mut Window, &mut App)>);

impl QueryHandler {
    fn new(handler: impl Fn(&str, &mut Window, &mut App) + 'static) -> Self {
        Self(Rc::new(handler))
    }

    pub fn call(&self, query: &str, window: &mut Window, cx: &mut App) {
        (self.0)(query, window, cx)
    }
}
