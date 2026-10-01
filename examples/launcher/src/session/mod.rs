//! Navigation and selection, independent of how anything is drawn.

mod navigator;
mod rows;

pub use navigator::{Entry, EntryId, Navigator};
pub use rows::{Line, Row, Rows};
