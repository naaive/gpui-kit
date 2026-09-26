//! The page nodes an extension builds, exported from the `launcher` module.
//!
//! Each is a GPUI Shell component whose materializer draws nothing. Methods are
//! recorded as small operation enums; materializing replays them into the
//! launcher's model types and hands the result to the parent in a `Carrier`.
//! A page node (`List`, `Detail`, `Form`) carries a whole
//! [`PageModel`](crate::model::PageModel); every other node carries the model
//! value its parent assembles.

mod action;
mod detail;
mod form;
mod list;
mod shared;

use gpui_shell::{ComponentRegistry, RegistryError};

pub(crate) use action::parse_toast_style;

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    list::register(registry)?;
    detail::register(registry)?;
    form::register(registry)?;
    action::register(registry)?;
    Ok(())
}
