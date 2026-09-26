//! What a page shows, as plain data.
//!
//! Every screen the launcher draws is a [`PageModel`]. Built-in pages build it
//! in Rust and extension pages build it in JavaScript, and one renderer in
//! `ui` draws both, which is what keeps an extension indistinguishable from a
//! built-in command. Nothing here holds a view or a window.

mod action;
mod page;

pub use action::{Action, Effect, RunHandler, ToastStyle};
pub use page::{Item, ItemId, ListModel, PageModel, Section};
