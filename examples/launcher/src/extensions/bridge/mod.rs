//! How an extension's script becomes a [`PageModel`].
//!
//! Two modules make up the extension SDK today:
//!
//! - `launcher`: the page nodes, registered as GPUI Shell components whose
//!   materializers draw nothing and carry a model value instead;
//! - `launcher/api`: host functions returning plain data, such as `launch()`.
//!
//! The launcher renders a command's `ScriptView` itself and takes the
//! [`PageModel`] out of the element it returns (see
//! [`crate::pages::ScriptPage`]), so a page is produced synchronously and its
//! callbacks always belong to the current render.

mod carrier;
mod components;
mod host_api;

use anyhow::{Result, anyhow};
use gpui_kit::AnyElement;
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

/// Takes the page out of the element a command's `render` produced, or
/// explains what was returned instead.
pub fn take_page_model(element: &mut AnyElement) -> Result<PageModel, &'static str> {
    carrier::take::<PageModel>(element)
        .ok_or("a command's `render` must return a `List` from the `launcher` module")
}
