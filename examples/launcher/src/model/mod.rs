//! What a page shows, as plain data.
//!
//! Every screen the launcher draws is a [`PageModel`]. Built-in pages build it
//! in Rust and extension pages build it in JavaScript, and one renderer in
//! `ui` draws both, which is what keeps an extension indistinguishable from a
//! built-in command. Nothing here holds a view or a window; behavior travels as
//! [`Callback`]s and [`Effect`]s that the window carries out.

mod action;
mod callback;
mod detail;
mod form;
mod image;
mod page;

pub use action::{
    Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation, Effect, Submenu,
    Toast, ToastStyle,
};
pub use callback::{Callback, FormHandler, PushHandler, RunHandler, TextHandler};
pub use detail::{DetailModel, Metadata, MetadataValue, Tag};
pub use form::{Choice, Control, Field, FormModel, FormValue, FormValues};
pub use image::{Image, Tone};
pub use page::{Accessory, Dropdown, Item, ItemId, Layout, ListModel, PageModel, Section};
