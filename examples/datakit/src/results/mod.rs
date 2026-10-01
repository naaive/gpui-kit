//! The rows a statement returned: a virtualized grid that fetches more as it
//! scrolls, sorts what it has, and copies or exports it.

mod aggregate;
mod export;
mod plan_view;
mod result_grid;
mod result_view;

use datakit_driver::RowStream;
use futures::StreamExt as _;
use gpui_kit::{App, KeyBinding, actions};

pub use export::{ExportContext, ExportFormat, export, export_xlsx, plain_text};
pub use plan_view::PlanView;
pub use result_grid::{FetchState, PAGE_SIZE, Page, ResultGrid, RowPages};
pub use result_view::{ResultView, ResultViewEvent};

use crate::services::Services;

actions!(
    results,
    [
        /// Copy the selected cell, row or column.
        CopyCells
    ]
);

pub(crate) const CONTEXT: &str = "ResultView";

/// The rows of `stream`, read a page at a time on the IO runtime. Dropping
/// the pages stops the statement.
pub fn result_pages(stream: RowStream, cx: &App) -> RowPages {
    Services::global(cx).forward(stream.chunks(result_grid::PAGE_SIZE), 1)
}

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("secondary-c", CopyCells, Some(CONTEXT))]);
}
