//! The data editor's grid: the table's rows with the pending edits laid over
//! them, one cell at a time editable in place.
//!
//! Rows arrive a page at a time, each page a statement of its own that the
//! server finishes: a result left open would hold the table's locks for as
//! long as the editor shows it.

use std::{rc::Rc, sync::Arc};

use datakit_driver::{ColumnInfo, Row, Value};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, h_flex,
    input::{Input, InputState},
    menu::PopupMenu,
    table::{Column, ColumnSort, TableDelegate, TableState},
};
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use super::{
    DeleteRows, RevertRow, SetNull,
    changes::{ChangeSet, RowState},
};
use crate::results::{CopyCells, PAGE_SIZE, plain_text};

/// Called when the person sorts by a column: the data editor sorts on the
/// server, so it reads the rows again.
pub type OnSort = Rc<dyn Fn(usize, ColumnSort, &mut Window, &mut App)>;

/// Called when the grid scrolls near its end and there are more rows.
pub type OnLoadMore = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct EditableGrid {
    columns: Arc<[ColumnInfo]>,
    table_columns: Vec<Column>,
    rows: Vec<Row>,
    changes: ChangeSet,
    has_more: bool,
    loading_more: bool,
    /// Why the last page could not be read.
    failure: Option<SharedString>,
    /// The cell being edited, and the input it is edited in.
    editing: Option<(usize, usize, Entity<InputState>)>,
    read_only: bool,
    on_sort: OnSort,
    on_load_more: OnLoadMore,
}

impl EditableGrid {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        columns: Arc<[ColumnInfo]>,
        rows: Vec<Row>,
        has_more: bool,
        sort: Option<(usize, ColumnSort)>,
        read_only: bool,
        on_sort: OnSort,
        on_load_more: OnLoadMore,
        window: &Window,
    ) -> Self {
        let rem = window.rem_size();
        let sample = &rows[..rows.len().min(200)];
        let table_columns = columns
            .iter()
            .enumerate()
            .map(|(ix, column)| {
                let widest = sample
                    .iter()
                    .filter_map(|row| row[ix].display())
                    .map(|text| text.lines().next().unwrap_or_default().chars().count())
                    .max()
                    .unwrap_or_default();
                let characters = widest.max(column.name().chars().count()).clamp(4, 48) as f32;
                let definition = Column::new(
                    SharedString::from(format!("c{ix}")),
                    SharedString::from(column.name().to_string()),
                )
                .width(rems(characters * 0.55 + 2.).to_pixels(rem))
                .min_width(rems(3.).to_pixels(rem))
                .sortable();
                let definition = match sort {
                    Some((sorted, ColumnSort::Ascending)) if sorted == ix => definition.ascending(),
                    Some((sorted, ColumnSort::Descending)) if sorted == ix => {
                        definition.descending()
                    }
                    _ => definition,
                };
                if column.category().is_numeric() {
                    definition.text_right()
                } else {
                    definition
                }
            })
            .collect();
        Self {
            columns,
            table_columns,
            rows,
            changes: ChangeSet::default(),
            has_more,
            loading_more: false,
            failure: None,
            editing: None,
            read_only,
            on_sort,
            on_load_more,
        }
    }

    pub fn columns(&self) -> &Arc<[ColumnInfo]> {
        &self.columns
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn changes(&self) -> &ChangeSet {
        &self.changes
    }

    pub fn changes_mut(&mut self) -> &mut ChangeSet {
        &mut self.changes
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether the rows may be changed; it is known once the catalog has
    /// read the relation.
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
        if read_only {
            self.editing = None;
        }
    }

    pub fn has_more_rows(&self) -> bool {
        self.has_more
    }

    pub fn is_loading_more(&self) -> bool {
        self.loading_more
    }

    pub fn failure(&self) -> Option<&SharedString> {
        self.failure.as_ref()
    }

    /// Add the rows of the next page. New rows go after the existing ones;
    /// rows being inserted stay after the end.
    pub fn append_page(&mut self, rows: Vec<Row>, has_more: bool) {
        self.rows.extend(rows);
        self.has_more = has_more;
        self.loading_more = false;
    }

    pub fn fail_page(&mut self, error: SharedString) {
        self.failure = Some(error);
        self.loading_more = false;
        self.has_more = false;
    }

    pub fn editing(&self) -> Option<(usize, usize, &Entity<InputState>)> {
        self.editing
            .as_ref()
            .map(|(row, column, input)| (*row, *column, input))
    }

    pub fn set_editing(&mut self, editing: Option<(usize, usize, Entity<InputState>)>) {
        self.editing = editing;
    }

    /// The value a cell shows, after the pending edits; `None` for a column
    /// of a new row the person has not set, which takes its default.
    pub fn value(&self, row: usize, column: usize) -> Option<&Value> {
        self.changes.value(&self.rows, row, column)
    }

    pub fn row_state(&self, row: usize) -> RowState {
        self.changes.row_state(row, self.rows.len())
    }
}

impl TableDelegate for EditableGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.table_columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len() + self.changes.inserted_rows()
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
        if let Some((row, column, input)) = &self.editing
            && *row == row_ix
            && *column == col_ix
        {
            return div()
                .w_full()
                .child(Input::new(input).xsmall().bordered(false))
                .into_any_element();
        }
        let theme = cx.theme();
        let numeric = self.columns[col_ix].category().is_numeric();
        let state = self.row_state(row_ix);
        let edited = self.changes.is_edited(row_ix, col_ix, self.rows.len());
        let cell = div()
            .w_full()
            .truncate()
            .when(numeric, |cell| cell.text_right())
            .when(edited, |cell| cell.bg(theme.warning.opacity(0.18)))
            .when(state == RowState::Inserted, |cell| {
                cell.bg(theme.success.opacity(0.12))
            })
            .when(state == RowState::Deleted, |cell| {
                cell.line_through().text_color(theme.muted_foreground)
            });
        match self.value(row_ix, col_ix) {
            None => cell
                .text_color(theme.muted_foreground)
                .child(t!("table.default").to_string()),
            Some(Value::Null) => cell.text_color(theme.muted_foreground).child("NULL"),
            Some(value) => {
                let text = value.display().unwrap_or_default();
                let line = text.lines().next().unwrap_or_default();
                let shown: SharedString = if line.len() < text.len() {
                    format!("{line}…").into()
                } else {
                    line.to_string().into()
                };
                cell.child(shown)
            }
        }
        .into_any_element()
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.value(row_ix, col_ix)
            .map(plain_text)
            .unwrap_or_default()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        (self.on_sort)(col_ix, sort, window, cx);
    }

    fn context_menu(
        &mut self,
        _: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let menu = menu.menu(t!("results.copy").to_string(), Box::new(CopyCells));
        if self.read_only {
            return menu;
        }
        menu.separator()
            .menu(t!("table.set_null").to_string(), Box::new(SetNull))
            .menu(t!("table.delete_rows").to_string(), Box::new(DeleteRows))
            .menu(t!("table.revert_row").to_string(), Box::new(RevertRow))
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
        self.has_more && !self.loading_more
    }

    fn load_more_threshold(&self) -> usize {
        PAGE_SIZE / 5
    }

    fn load_more(&mut self, window: &mut Window, cx: &mut Context<TableState<Self>>) {
        if !self.has_more || self.loading_more {
            return;
        }
        self.loading_more = true;
        (self.on_load_more)(window, cx);
    }
}
