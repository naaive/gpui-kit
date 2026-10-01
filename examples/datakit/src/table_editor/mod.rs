//! The data editor: a table's rows to browse, filter, sort on the server and
//! edit in place, with its DDL beside them.
//!
//! Edits are pending until they are submitted; they are then written in
//! one transaction, and the rows are read again.

mod changes;
mod grid;
mod table_panel;

use gpui_kit::{App, KeyBinding, actions};

pub use table_panel::TablePanel;

actions!(
    table_editor,
    [
        /// Write the pending changes to the database.
        SubmitChanges,
        /// Forget the pending changes.
        RevertChanges,
        /// Add a row.
        AddRow,
        /// Delete the selected row, or undelete it.
        DeleteRows,
        /// Forget the pending changes of the selected row.
        RevertRow,
        /// Set the selected cell to NULL.
        SetNull,
        /// Edit the selected cell.
        EditCell,
        /// Leave the cell being edited without changing it.
        CancelCellEdit,
        /// Read the rows again.
        ReloadRows,
        /// Put copied rows into the cells from the selected one on, adding
        /// rows past the last.
        PasteCells
    ]
);

pub(crate) const CONTEXT: &str = "TableEditor";

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-enter", SubmitChanges, Some(CONTEXT)),
        KeyBinding::new("alt-insert", AddRow, Some(CONTEXT)),
        KeyBinding::new(
            "secondary-backspace",
            DeleteRows,
            Some("TableEditor > DataTable"),
        ),
        KeyBinding::new("delete", DeleteRows, Some("TableEditor > DataTable")),
        KeyBinding::new("f2", EditCell, Some("TableEditor > DataTable")),
        KeyBinding::new("enter", EditCell, Some("TableEditor > DataTable")),
        KeyBinding::new(
            "escape",
            CancelCellEdit,
            Some("TableEditor > DataTable > Input"),
        ),
        KeyBinding::new("secondary-v", PasteCells, Some("TableEditor > DataTable")),
        KeyBinding::new("secondary-r", ReloadRows, Some(CONTEXT)),
        KeyBinding::new("f5", ReloadRows, Some(CONTEXT)),
    ]);
}
