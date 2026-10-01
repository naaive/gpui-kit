use std::sync::Arc;

use datakit_driver::{ColumnInfo, DatabaseError, Row, Value};
use datakit_runtime::RemoteStream;
use futures::StreamExt as _;
use gpui_kit::component::{
    ActiveTheme as _, h_flex,
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
};
use gpui_kit::{
    App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use super::CopyCells;
use crate::datasource::describe_error;

/// The rows of a result still on their way, a page at a time.
pub type RowPages = RemoteStream<Vec<anyhow::Result<Row>>>;

/// How many rows are fetched at a time: on execution, and each time the grid
/// scrolls near its end.
pub const PAGE_SIZE: usize = 500;

/// One page of rows, and what fetching it found out.
pub struct Page {
    rows: Vec<Row>,
    error: Option<SharedString>,
    database_error: Option<DatabaseError>,
    /// Whether the result has no rows after these.
    exhausted: bool,
}

impl Page {
    /// Fetch the next page from `pages`.
    pub async fn fetch(pages: &mut RowPages) -> Self {
        let Some(items) = pages.next().await else {
            return Self {
                rows: Vec::new(),
                error: None,
                database_error: None,
                exhausted: true,
            };
        };
        // A page shorter than asked for is the last one.
        let exhausted = items.len() < PAGE_SIZE;
        let mut rows = Vec::with_capacity(items.len());
        let mut error = None;
        let mut database_error = None;
        for item in items {
            match item {
                Ok(row) => rows.push(row),
                Err(failure) => {
                    error = Some(describe_error(&failure));
                    database_error = failure.downcast_ref::<DatabaseError>().cloned();
                    break;
                }
            }
        }
        Self {
            exhausted: exhausted || error.is_some(),
            rows,
            error,
            database_error,
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// The server's description of [`Self::error`], when the server sent it.
    pub fn database_error(&self) -> Option<&DatabaseError> {
        self.database_error.as_ref()
    }
}

/// Whether more rows can be fetched.
pub enum FetchState {
    /// More rows are waiting on the server.
    Idle,
    Fetching {
        _task: Task<()>,
    },
    /// Every row has been fetched.
    Complete,
    /// The server failed partway; the rows before the failure are kept.
    Failed(SharedString),
}

/// The rows of one result, as the grid's data source.
///
/// Rows are kept in the order the server sent them; sorting only changes
/// [`Self::order`], so the original order comes back when the sort is
/// cleared and rows fetched later are sorted in.
pub struct ResultGrid {
    columns: Arc<[ColumnInfo]>,
    table_columns: Vec<Column>,
    /// The result column shown at each position; columns can be dragged.
    column_order: Vec<usize>,
    rows: Vec<Row>,
    order: Option<Vec<usize>>,
    sort: Option<(usize, ColumnSort)>,
    pages: Option<RowPages>,
    fetch: FetchState,
}

impl ResultGrid {
    pub fn new(
        columns: Arc<[ColumnInfo]>,
        first_page: Page,
        pages: RowPages,
        window: &Window,
    ) -> Self {
        let rem = window.rem_size();
        // Wide enough for the name and the first page's values, within
        // reason; people resize the rest.
        let sample = &first_page.rows[..first_page.rows.len().min(200)];
        let table_columns = columns
            .iter()
            .enumerate()
            .map(|(ix, column)| {
                let widest_value = sample
                    .iter()
                    .filter_map(|row| row[ix].display())
                    .map(|text| text.lines().next().unwrap_or_default().chars().count())
                    .max()
                    .unwrap_or_default();
                let characters =
                    widest_value.max(column.name().chars().count()).clamp(4, 48) as f32;
                let column_def = Column::new(
                    SharedString::from(format!("c{ix}")),
                    SharedString::from(column.name().to_string()),
                )
                .width(rems(characters * 0.55 + 2.).to_pixels(rem))
                .min_width(rems(3.).to_pixels(rem))
                .sortable();
                if column.category().is_numeric() {
                    column_def.text_right()
                } else {
                    column_def
                }
            })
            .collect();
        let fetch = match (&first_page.error, first_page.exhausted) {
            (Some(error), _) => FetchState::Failed(error.clone()),
            (None, true) => FetchState::Complete,
            (None, false) => FetchState::Idle,
        };
        Self {
            column_order: (0..columns.len()).collect(),
            columns,
            table_columns,
            pages: (!first_page.exhausted).then_some(pages),
            rows: first_page.rows,
            order: None,
            sort: None,
            fetch,
        }
    }

    pub fn columns(&self) -> &Arc<[ColumnInfo]> {
        &self.columns
    }

    /// The result column shown at display position `col_ix`.
    pub fn source_column(&self, col_ix: usize) -> usize {
        self.column_order[col_ix]
    }

    pub fn rows_fetched(&self) -> usize {
        self.rows.len()
    }

    pub fn fetch_state(&self) -> &FetchState {
        &self.fetch
    }

    pub fn has_more_rows(&self) -> bool {
        self.pages.is_some()
    }

    /// The row shown at `row_ix`, after sorting.
    pub fn row(&self, row_ix: usize) -> &Row {
        match &self.order {
            Some(order) => &self.rows[order[row_ix]],
            None => &self.rows[row_ix],
        }
    }

    /// Rows in the order they are shown.
    pub fn displayed_rows(&self) -> impl Iterator<Item = &Row> {
        (0..self.rows.len()).map(|row_ix| self.row(row_ix))
    }

    /// Stop reading rows; dropping the pages ends the statement on the
    /// server, so the session can run another.
    pub fn stop_fetching(&mut self) {
        self.pages = None;
        if !matches!(self.fetch, FetchState::Failed(_)) {
            self.fetch = FetchState::Complete;
        }
    }

    /// Fetch the next page, unless one is on its way or there is none.
    pub fn fetch_next_page(&mut self, cx: &mut Context<TableState<Self>>) {
        if !matches!(self.fetch, FetchState::Idle) {
            return;
        }
        let Some(mut pages) = self.pages.take() else {
            return;
        };
        self.fetch = FetchState::Fetching {
            _task: cx.spawn(async move |table, cx| {
                let page = Page::fetch(&mut pages).await;
                let _ = table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    grid.rows.extend(page.rows);
                    grid.fetch = match page.error {
                        Some(error) => FetchState::Failed(error),
                        None if page.exhausted => FetchState::Complete,
                        None => {
                            grid.pages = Some(pages);
                            FetchState::Idle
                        }
                    };
                    grid.apply_sort();
                    cx.notify();
                });
            }),
        };
        cx.notify();
    }

    fn apply_sort(&mut self) {
        let Some((col_ix, sort)) = self.sort else {
            self.order = None;
            return;
        };
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by(|a, b| {
            let ordering = self.rows[*a][col_ix].sort_cmp(&self.rows[*b][col_ix]);
            match sort {
                ColumnSort::Descending => ordering.reverse(),
                _ => ordering,
            }
        });
        self.order = Some(order);
    }
}

/// The first line of `text`, short enough to lay out quickly; the full
/// value is one copy away.
fn cell_text(text: &str) -> SharedString {
    const LIMIT: usize = 256;
    let line = text.lines().next().unwrap_or_default();
    let truncated = line.len() < text.len();
    match line.char_indices().nth(LIMIT) {
        Some((end, _)) => format!("{}…", &line[..end]).into(),
        None if truncated => format!("{line}…").into(),
        None => line.to_string().into(),
    }
}

impl TableDelegate for ResultGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.table_columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.table_columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let source = self.column_order[col_ix];
        let numeric = self.columns[source].category().is_numeric();
        let cell = div()
            .w_full()
            .truncate()
            .when(numeric, |cell| cell.text_right());
        match &self.row(row_ix)[source] {
            Value::Null => cell.text_color(cx.theme().muted_foreground).child("NULL"),
            value => cell.child(cell_text(&value.display().unwrap_or_default())),
        }
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        super::plain_text(&self.row(row_ix)[self.column_order[col_ix]])
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.sort = match sort {
            ColumnSort::Default => None,
            sort => Some((self.column_order[col_ix], sort)),
        };
        self.apply_sort();
        cx.notify();
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let column = self.table_columns.remove(col_ix);
        self.table_columns.insert(to_ix, column);
        let source = self.column_order.remove(col_ix);
        self.column_order.insert(to_ix, source);
    }

    fn context_menu(
        &mut self,
        _: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        menu.menu(t!("results.copy").to_string(), Box::new(CopyCells))
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .justify_center()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(t!("results.no_rows").to_string())
    }

    fn has_more(&self, _: &App) -> bool {
        self.pages.is_some()
    }

    fn load_more_threshold(&self) -> usize {
        PAGE_SIZE / 5
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        self.fetch_next_page(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_show_the_first_line_and_mark_what_is_hidden() {
        assert_eq!(cell_text("short"), "short");
        assert_eq!(cell_text("first\nsecond"), "first…");
        let long = "x".repeat(300);
        assert_eq!(cell_text(&long).chars().count(), 257);
    }
}
