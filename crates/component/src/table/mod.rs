use gpui::{App, actions};

mod column;
mod data_table;
mod delegate;
mod loading;
mod state;
mod table;

pub use column::*;
pub use data_table::*;
pub use delegate::*;
pub use state::*;
pub use table::*;

actions!(
    table,
    [
        /// Extend the selected block of cells one row up.
        ExtendSelectionUp,
        /// Extend the selected block of cells one row down.
        ExtendSelectionDown,
        /// Extend the selected block of cells one column left.
        ExtendSelectionLeft,
        /// Extend the selected block of cells one column right.
        ExtendSelectionRight
    ]
);

pub(crate) fn init(cx: &mut App) {
    data_table::init(cx);
}
