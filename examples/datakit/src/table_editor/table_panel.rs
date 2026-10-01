use std::{rc::Rc, sync::Arc};

use datakit_driver::{ColumnInfo, Connection, DataSourceId, Row, StatementOutcome, Value};
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelInfo, PanelState},
    h_flex,
    input::{Editor, EditorState, Input, InputEvent, InputState},
    notification::Notification,
    resizable::{h_resizable, resizable_panel},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tab::{Tab, TabBar},
    table::{ColumnSort, DataTable, TableEvent, TableSelection, TableState},
    text::TextView,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, WeakEntity, Window, div, prelude::FluentBuilder as _, px, rems,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use super::{
    AddRow, CONTEXT, CancelCellEdit, DeleteRows, EditCell, PasteCells, ReloadRows, RevertChanges,
    RevertRow, SetNull, SubmitChanges,
    changes::{RowState, parse_tsv, typed_value},
    grid::EditableGrid,
};
use crate::{
    datasource::{DataSource, DataSources, describe_error},
    format,
    objects::{self, ObjectPath, ObjectRef},
    results::{CopyCells, PAGE_SIZE, plain_text},
    services::Services,
};

/// A data editor's identity in a saved layout.
#[derive(Serialize, Deserialize)]
struct SavedTable {
    data_source: DataSourceId,
    schema: String,
    relation: String,
    #[serde(default)]
    condition: String,
    #[serde(default)]
    order_by: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditorTab {
    Data,
    Ddl,
}

enum Loading {
    Idle,
    Running { _task: Task<()> },
    Failed(SharedString),
}

enum Count {
    None,
    Counting { _task: Task<()> },
    Counted(u64),
    Failed(SharedString),
}

/// A table's rows, editable in place.
pub struct TablePanel {
    focus_handle: FocusHandle,
    data_source_id: DataSourceId,
    data_source: Option<Entity<DataSource>>,
    schema: Arc<str>,
    relation: Arc<str>,
    condition: Entity<InputState>,
    order_by: Entity<InputState>,
    table: Option<Entity<TableState<EditableGrid>>>,
    /// The columns that identify a row, as result column positions.
    key: Vec<usize>,
    loading: Loading,
    session: Option<Arc<dyn Connection>>,
    count: Count,
    /// The `WHERE` and `ORDER BY` of the rows shown, which later pages use.
    query: (String, String),
    load_more_task: Option<Task<()>>,
    submitting: Option<Task<()>>,
    sort: Option<(usize, ColumnSort)>,
    tab: EditorTab,
    ddl: Entity<EditorState>,
    show_value_editor: bool,
    value_editor: Entity<EditorState>,
    /// The cell the value editor shows.
    value_cell: Option<(usize, usize)>,
    _subscriptions: Vec<Subscription>,
    table_subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for TablePanel {}

impl TablePanel {
    pub const NAME: &str = "Table";

    /// A data editor for the relation `object` names.
    pub fn new(object: &ObjectRef, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ObjectPath::Relation { schema, relation } = object.path() else {
            unreachable!("a data editor is for relations");
        };
        let data_source = object.data_source().clone();
        let id = data_source.read(cx).profile().id().clone();
        Self::build(
            id,
            Some(data_source),
            schema.clone(),
            relation.clone(),
            "",
            "",
            window,
            cx,
        )
    }

    /// The data editor a saved layout describes.
    pub fn restore(state: &PanelState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = match &state.info {
            PanelInfo::Panel(value) => serde_json::from_value::<SavedTable>(value.clone()).ok(),
            _ => None,
        };
        let saved = saved.unwrap_or(SavedTable {
            data_source: DataSourceId::from(""),
            schema: String::new(),
            relation: String::new(),
            condition: String::new(),
            order_by: String::new(),
        });
        let data_source = DataSources::global(cx).read(cx).get(&saved.data_source, cx);
        Self::build(
            saved.data_source,
            data_source,
            saved.schema.into(),
            saved.relation.into(),
            &saved.condition,
            &saved.order_by,
            window,
            cx,
        )
    }

    /// Whether this editor shows `object`.
    pub fn shows(&self, object: &ObjectRef, cx: &App) -> bool {
        object.data_source().read(cx).profile().id() == &self.data_source_id
            && object.path() == &ObjectPath::relation(self.schema.clone(), self.relation.clone())
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        data_source_id: DataSourceId,
        data_source: Option<Entity<DataSource>>,
        schema: Arc<str>,
        relation: Arc<str>,
        condition: &str,
        order_by: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter =
            |placeholder: &str, value: &str, window: &mut Window, cx: &mut Context<Self>| {
                let placeholder = SharedString::from(placeholder.to_string());
                let value = SharedString::from(value.to_string());
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(placeholder)
                        .default_value(value)
                })
            };
        let condition = filter("id > 100", condition, window, cx);
        let order_by = filter("id DESC", order_by, window, cx);
        let ddl = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
        });
        let value_editor = cx.new(|cx| EditorState::new(window, cx).soft_wrap(true));
        let mut subscriptions = Vec::new();
        for input in [&condition, &order_by] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.reload(window, cx);
                    }
                },
            ));
        }
        if let Some(data_source) = &data_source {
            // The DDL, the key and whether rows can change follow the
            // catalog, which may arrive after the rows.
            subscriptions.push(cx.observe_in(data_source, window, |this, _, window, cx| {
                this.show_ddl(window, cx);
                this.refresh_identity(cx);
            }));
        }
        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            data_source_id,
            data_source,
            schema,
            relation,
            condition,
            order_by,
            table: None,
            key: Vec::new(),
            loading: Loading::Idle,
            session: None,
            count: Count::None,
            query: Default::default(),
            load_more_task: None,
            submitting: None,
            sort: None,
            tab: EditorTab::Data,
            ddl,
            show_value_editor: false,
            value_editor,
            value_cell: None,
            _subscriptions: subscriptions,
            table_subscriptions: Vec::new(),
        };
        panel.show_ddl(window, cx);
        panel.reload(window, cx);
        panel
    }

    fn object(&self) -> Option<ObjectRef> {
        Some(ObjectRef::new(
            self.data_source.clone()?,
            ObjectPath::relation(self.schema.clone(), self.relation.clone()),
        ))
    }

    fn show_ddl(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(object) = self.object() else {
            return;
        };
        if let Some(data_source) = &self.data_source {
            data_source.update(cx, |data_source, cx| {
                data_source.ensure(
                    crate::datasource::CatalogRequest::Objects(self.schema.clone()),
                    cx,
                )
            });
        }
        let ddl = object.ddl(cx).unwrap_or_default();
        if self.ddl.read(cx).value().as_ref() != ddl {
            self.ddl.update(cx, |editor, cx| {
                editor.set_value(ddl, window, cx);
                editor.set_readonly(true, cx);
            });
        }
    }

    /// Whether rows can be changed: a table, with a data source.
    fn is_read_only(&self, cx: &App) -> bool {
        let Some(data_source) = &self.data_source else {
            return true;
        };
        if data_source.read(cx).is_read_only() {
            return true;
        }
        let path = ObjectPath::relation(self.schema.clone(), self.relation.clone());
        !matches!(
            path.resolve(data_source.read(cx).catalog()),
            Some(objects::CatalogObject::Relation(relation)) if relation.relation_type().is_table()
        )
    }

    fn has_changes(&self, cx: &App) -> bool {
        self.table
            .as_ref()
            .is_some_and(|table| !table.read(cx).delegate().changes().is_empty())
    }

    /// Read one page of rows — at most [`PAGE_SIZE`], and whether there are
    /// more — from `offset` with the filter and order of the last reload.
    /// The statement runs to its end, so it holds no lock afterwards.
    fn query_page(
        &mut self,
        offset: usize,
        cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<(Arc<[ColumnInfo]>, Vec<Row>, bool)>>> {
        let data_source = self.data_source.clone()?;
        let (condition, order_by) = self.query.clone();
        let sql: Arc<str> = data_source
            .read(cx)
            .dialect()
            .select_page(
                &self.schema,
                &self.relation,
                &condition,
                &order_by,
                PAGE_SIZE as u64 + 1,
                offset as u64,
            )
            .into();
        let session = self.session.clone().filter(|session| !session.is_closed());
        let connecting = match &session {
            Some(_) => None,
            None => Some(data_source.read(cx).open_session(cx)),
        };
        let work = Services::global(cx).spawn(async move {
            let connection = match (session, connecting) {
                (Some(session), _) => session,
                (None, Some(connecting)) => connecting.await?,
                (None, None) => unreachable!("a session or a way to open one"),
            };
            let rows = match connection.execute(sql).await? {
                StatementOutcome::Rows(mut stream) => {
                    let columns = stream.columns().clone();
                    let mut rows = Vec::new();
                    while let Some(row) = stream.next().await {
                        rows.push(row?);
                    }
                    (columns, rows)
                }
                StatementOutcome::Command(_) => (Arc::from([]), Vec::new()),
            };
            Ok((connection, rows))
        });
        Some(cx.spawn(async move |this, cx| {
            let (connection, (columns, mut rows)) = work.await?;
            let _ = this.update(cx, |this, _| this.session = Some(connection));
            let has_more = rows.len() > PAGE_SIZE;
            rows.truncate(PAGE_SIZE);
            Ok((columns, rows, has_more))
        }))
    }

    /// Read the rows again with the current filter and order. Pending
    /// changes are dropped.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let condition = self.condition.read(cx).value().to_string();
        let mut order_by = self.order_by.read(cx).value().to_string();
        // Without an order of its own the database may return a row it just
        // changed anywhere; the primary key keeps rows where they were.
        if order_by.trim().is_empty() {
            order_by = self.primary_key_order(cx);
        }
        self.query = (condition, order_by);
        let Some(page) = self.query_page(0, cx) else {
            return;
        };
        let read_only = self.is_read_only(cx);
        let sort = self.sort;
        let panel = cx.entity().downgrade();
        let on_sort: super::grid::OnSort = Rc::new(move |column, sort, window, cx| {
            let _ = panel.update(cx, |this, cx| this.sort_by(column, sort, window, cx));
        });
        let panel = cx.entity().downgrade();
        let on_load_more: super::grid::OnLoadMore = Rc::new(move |_, cx| {
            let _ = panel.update(cx, |this, cx| this.load_more(cx));
        });
        self.count = Count::None;
        let task = cx.spawn_in(window, async move |this, cx| {
            let page = page.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let (columns, rows, has_more) = match page {
                    Ok(page) => page,
                    Err(error) => {
                        this.loading = Loading::Failed(describe_error(&error));
                        cx.notify();
                        return;
                    }
                };
                this.key = this.key_columns(&columns, cx);
                let grid = EditableGrid::new(
                    columns,
                    rows,
                    has_more,
                    sort,
                    read_only,
                    on_sort,
                    on_load_more,
                    window,
                );
                let table = cx.new(|cx| TableState::new(grid, window, cx).cell_selectable(true));
                this.table_subscriptions = vec![
                    cx.subscribe_in(&table, window, Self::on_table_event),
                    cx.observe(&table, |_, _, cx| cx.notify()),
                ];
                this.table = Some(table);
                // The catalog may have arrived while the rows were read.
                this.refresh_identity(cx);
                this.value_cell = None;
                this.loading = Loading::Idle;
                cx.emit(PanelEvent::LayoutChanged);
                cx.notify();
            });
        });
        self.loading = Loading::Running { _task: task };
        cx.notify();
    }

    /// Read the page after the rows the grid has.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.table.clone() else {
            return;
        };
        let offset = table.read(cx).delegate().rows().len();
        let Some(page) = self.query_page(offset, cx) else {
            return;
        };
        self.load_more_task = Some(cx.spawn(async move |_, cx| {
            let page = page.await;
            let _ = table.update(cx, |table, cx| {
                match page {
                    Ok((_, rows, has_more)) => table.delegate_mut().append_page(rows, has_more),
                    Err(error) => table.delegate_mut().fail_page(describe_error(&error)),
                }
                cx.notify();
            });
        }));
    }

    /// `ORDER BY` text for the relation's primary key, or nothing when it
    /// has none or is not read yet.
    fn primary_key_order(&self, cx: &App) -> String {
        let Some(data_source) = &self.data_source else {
            return String::new();
        };
        let source = data_source.read(cx);
        let dialect = source.dialect();
        let path = ObjectPath::relation(self.schema.clone(), self.relation.clone());
        match path.resolve(source.catalog()) {
            Some(objects::CatalogObject::Relation(relation)) => relation
                .primary_key()
                .iter()
                .map(|column| dialect.quote_identifier(&column.name()))
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        }
    }

    /// Recompute the key and whether rows can change from the catalog.
    fn refresh_identity(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.table.clone() else {
            return;
        };
        let read_only = self.is_read_only(cx);
        let columns = table.read(cx).delegate().columns().clone();
        self.key = self.key_columns(&columns, cx);
        table.update(cx, |table, cx| {
            if table.delegate().is_read_only() != read_only {
                table.delegate_mut().set_read_only(read_only);
                cx.notify();
            }
        });
    }

    /// The columns that identify a row: the table's key, or every column
    /// when it has none.
    fn key_columns(&self, columns: &[datakit_driver::ColumnInfo], cx: &App) -> Vec<usize> {
        let names: Vec<Arc<str>> = self
            .data_source
            .as_ref()
            .and_then(|data_source| {
                let path = ObjectPath::relation(self.schema.clone(), self.relation.clone());
                match path.resolve(data_source.read(cx).catalog()) {
                    Some(objects::CatalogObject::Relation(relation)) => Some(
                        relation
                            .row_identity()
                            .iter()
                            .map(|column| column.name())
                            .collect(),
                    ),
                    _ => None,
                }
            })
            .unwrap_or_default();
        let key: Vec<usize> = names
            .iter()
            .filter_map(|name| columns.iter().position(|column| column.name() == *name))
            .collect();
        if key.is_empty() || key.len() < names.len() {
            (0..columns.len()).collect()
        } else {
            key
        }
    }

    fn sort_by(
        &mut self,
        column: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .table
            .as_ref()
            .map(|table| table.read(cx).delegate().columns()[column].name())
        else {
            return;
        };
        let dialect = match &self.data_source {
            Some(data_source) => data_source.read(cx).dialect(),
            None => return,
        };
        let order = match sort {
            ColumnSort::Ascending => format!("{} ASC", dialect.quote_identifier(&name)),
            ColumnSort::Descending => format!("{} DESC", dialect.quote_identifier(&name)),
            ColumnSort::Default => String::new(),
        };
        self.sort = (sort != ColumnSort::Default).then_some((column, sort));
        self.order_by
            .update(cx, |input, cx| input.set_value(order, window, cx));
        self.reload(window, cx);
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<EditableGrid>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::DoubleClickedCell(row, column) => {
                self.start_edit(*row, *column, window, cx)
            }
            TableEvent::SelectCell(row, column) => {
                self.value_cell = Some((*row, *column));
                self.show_value(window, cx);
            }
            _ => {}
        }
    }

    /// The selected cell, or the first cell of the selected row.
    fn selected_cell(&self, cx: &App) -> Option<(usize, usize)> {
        match self.table.as_ref()?.read(cx).selection() {
            TableSelection::Cell(row, column) => Some((row, column)),
            TableSelection::Row(row) => Some((row, 0)),
            _ => None,
        }
    }

    fn start_edit(
        &mut self,
        row: usize,
        column: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = self.table.clone() else {
            return;
        };
        let grid = table.read(cx).delegate();
        if grid.is_read_only() || grid.row_state(row) == RowState::Deleted {
            return;
        }
        let text = grid.value(row, column).map(plain_text).unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_edit(window, cx),
                _ => {}
            },
        );
        self.table_subscriptions.push(subscription);
        table.update(cx, |table, cx| {
            table
                .delegate_mut()
                .set_editing(Some((row, column, input.clone())));
            cx.notify();
        });
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
    }

    fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.table.clone() else {
            return;
        };
        let Some((row, column, text)) = table
            .read(cx)
            .delegate()
            .editing()
            .map(|(row, column, input)| (row, column, input.read(cx).value().to_string()))
        else {
            return;
        };
        table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let value = typed_value(&text, &grid.columns()[column]);
            let unchanged = grid
                .value(row, column)
                .is_some_and(|current| plain_text(current) == text);
            let rows = grid.rows().to_vec();
            if !unchanged {
                grid.changes_mut().set(&rows, row, column, value);
            }
            grid.set_editing(None);
            cx.notify();
        });
        self.focus_table(window, cx);
        self.show_value(window, cx);
    }

    fn cancel_edit(&mut self, _: &CancelCellEdit, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(table) = &self.table {
            table.update(cx, |table, cx| {
                table.delegate_mut().set_editing(None);
                cx.notify();
            });
        }
        self.focus_table(window, cx);
    }

    fn focus_table(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(table) = &self.table {
            let focus = table.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
    }

    /// Whether a cell is being edited. The grid's own keys then belong to
    /// the cell's input: the actions bound to them step aside.
    fn is_editing(&self, cx: &App) -> bool {
        self.table
            .as_ref()
            .is_some_and(|table| table.read(cx).delegate().editing().is_some())
    }

    fn edit_cell(&mut self, _: &EditCell, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_editing(cx) {
            cx.propagate();
            return;
        }
        if let Some((row, column)) = self.selected_cell(cx) {
            self.start_edit(row, column, window, cx);
        }
    }

    fn set_null(&mut self, _: &SetNull, window: &mut Window, cx: &mut Context<Self>) {
        let Some((row, column)) = self.selected_cell(cx) else {
            return;
        };
        self.set_cell(row, column, Value::Null, window, cx);
    }

    fn set_cell(
        &mut self,
        row: usize,
        column: usize,
        value: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(table) = &self.table else {
            return;
        };
        table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            if grid.is_read_only() {
                return;
            }
            let rows = grid.rows().to_vec();
            grid.changes_mut().set(&rows, row, column, value);
            cx.notify();
        });
        self.show_value(window, cx);
    }

    fn add_row(&mut self, _: &AddRow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = self.table.clone() else {
            return;
        };
        if table.read(cx).delegate().is_read_only() {
            return;
        }
        let row = table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.changes_mut().insert_row();
            let row = grid.rows().len() + grid.changes().inserted_rows() - 1;
            table.set_selected_cell(row, 0, cx);
            table.scroll_to_row(row, cx);
            cx.notify();
            row
        });
        self.start_edit(row, 0, window, cx);
    }

    fn delete_rows(&mut self, _: &DeleteRows, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_editing(cx) {
            cx.propagate();
            return;
        }
        let Some(table) = self.table.clone() else {
            return;
        };
        // Every row the selected block of cells spans.
        let Some(rows) = table
            .read(cx)
            .selected_cell_range()
            .map(|(rows, _)| rows)
            .or_else(|| self.selected_cell(cx).map(|(row, _)| row..row + 1))
        else {
            return;
        };
        table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            if grid.is_read_only() {
                return;
            }
            let existing = grid.rows().len();
            let rows: Vec<usize> = rows.collect();
            grid.changes_mut().toggle_delete(&rows, existing);
            cx.notify();
        });
    }

    fn revert_row(&mut self, _: &RevertRow, _: &mut Window, cx: &mut Context<Self>) {
        let Some((row, _)) = self.selected_cell(cx) else {
            return;
        };
        if let Some(table) = &self.table {
            table.update(cx, |table, cx| {
                let grid = table.delegate_mut();
                let existing = grid.rows().len();
                grid.changes_mut().revert_row(row, existing);
                cx.notify();
            });
        }
    }

    fn revert(&mut self, _: &RevertChanges, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(table) = &self.table {
            table.update(cx, |table, cx| {
                *table.delegate_mut().changes_mut() = Default::default();
                cx.notify();
            });
        }
    }

    /// The statements the pending changes need.
    fn pending_statements(&self, cx: &App) -> Vec<String> {
        let (Some(table), Some(data_source)) = (&self.table, &self.data_source) else {
            return Vec::new();
        };
        let grid = table.read(cx).delegate();
        let dialect = data_source.read(cx).dialect();
        grid.changes().statements(
            &*dialect,
            &self.schema,
            &self.relation,
            grid.columns(),
            &self.key,
            grid.rows(),
        )
    }

    fn submit(&mut self, _: &SubmitChanges, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting.is_some() {
            return;
        }
        let Some(data_source) = self.data_source.clone() else {
            return;
        };
        let changes = self.pending_statements(cx);
        if changes.is_empty() {
            return;
        }
        let dialect = data_source.read(cx).dialect();
        // One transaction: all of the changes are written, or none.
        let statements: Vec<String> = if dialect.supports_transactions() {
            std::iter::once(dialect.begin_transaction().to_string())
                .chain(changes)
                .chain(std::iter::once(dialect.commit().to_string()))
                .collect()
        } else {
            changes
        };
        let task = data_source.read(cx).run_statements(statements, cx);
        self.submitting = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.submitting = None;
                match result {
                    Ok(()) => this.reload(window, cx),
                    Err(error) => window.push_notification(
                        Notification::error(
                            t!("table.submit_failed", error = describe_error(&error)).to_string(),
                        ),
                        cx,
                    ),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let script = objects::script(&self.pending_statements(cx));
        let markdown = SharedString::from(format!("```sql\n{script}```"));
        let copy = script.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let markdown = markdown.clone();
            let copy = copy.clone();
            dialog
                .title(t!("table.preview_title").to_string())
                .w(px(640.))
                .content(move |content, _, _| {
                    let copy = copy.clone();
                    content
                        .child(
                            div()
                                .id("pending-sql")
                                .max_h(px(420.))
                                .overflow_y_scrollbar()
                                .child(TextView::markdown("pending-sql", markdown.clone())),
                        )
                        .child(
                            h_flex().justify_end().child(
                                Button::new("copy-pending")
                                    .outline()
                                    .small()
                                    .icon(IconName::Copy)
                                    .label(t!("table.copy_sql").to_string())
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            ),
                        )
                })
        });
    }

    /// Count the rows the filter selects, on the server, in a session of
    /// its own: the editor's may still be reading rows.
    fn count_rows(&mut self, cx: &mut Context<Self>) {
        let Some(data_source) = self.data_source.clone() else {
            return;
        };
        let session = data_source.read(cx).open_session(cx);
        let condition = self.condition.read(cx).value().to_string();
        let sql: Arc<str> = data_source
            .read(cx)
            .dialect()
            .count_rows(&self.schema, &self.relation, &condition)
            .into();
        let work = Services::global(cx).spawn(async move {
            let session = session.await?;
            match session.execute(sql).await? {
                StatementOutcome::Rows(mut rows) => {
                    let row = rows.next().await.transpose()?;
                    let count = row
                        .and_then(|row| row.first().and_then(Value::display).map(|t| t.to_string()))
                        .and_then(|text| text.parse::<u64>().ok())
                        .unwrap_or(0);
                    // Read to the end so the session is free again.
                    while rows.next().await.is_some() {}
                    Ok(count)
                }
                StatementOutcome::Command(_) => Ok(0),
            }
        });
        self.count = Count::Counting {
            _task: cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.count = match result {
                        Ok(count) => Count::Counted(count),
                        Err(error) => Count::Failed(describe_error(&error)),
                    };
                    cx.notify();
                });
            }),
        };
        cx.notify();
    }

    /// Show the selected cell in the value editor.
    fn show_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.show_value_editor {
            return;
        }
        let (Some(table), Some((row, column))) = (&self.table, self.value_cell) else {
            return;
        };
        let grid = table.read(cx).delegate();
        let text = match grid.value(row, column) {
            Some(Value::Text(text))
                if grid.columns()[column].category() == datakit_driver::TypeCategory::Json =>
            {
                serde_json::from_str::<serde_json::Value>(text)
                    .ok()
                    .and_then(|json| serde_json::to_string_pretty(&json).ok())
                    .unwrap_or_else(|| text.to_string())
            }
            Some(value) => plain_text(value),
            None => String::new(),
        };
        self.value_editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
    }

    fn apply_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(table), Some((row, column))) = (&self.table, self.value_cell) else {
            return;
        };
        let text = self.value_editor.read(cx).value().to_string();
        let value = typed_value(&text, &table.read(cx).delegate().columns()[column]);
        self.set_cell(row, column, value, window, cx);
    }

    /// Put the clipboard's rows into the cells from the selected one on.
    /// An empty field is `NULL`, as copying writes it.
    fn paste_cells(&mut self, _: &PasteCells, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_editing(cx) {
            cx.propagate();
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(table) = self.table.clone() else {
            return;
        };
        let start = match table.read(cx).selection() {
            TableSelection::Cell(row, column) => (row, column),
            TableSelection::Row(row) => (row, 0),
            _ => return,
        };
        let pasted = parse_tsv(&text);
        table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            if grid.is_read_only() {
                return;
            }
            let rows = grid.rows().to_vec();
            let columns = grid.columns().clone();
            for (offset, fields) in pasted.iter().enumerate() {
                let row = start.0 + offset;
                if row >= rows.len() + grid.changes().inserted_rows() {
                    grid.changes_mut().insert_row();
                }
                for (column, field) in (start.1..columns.len()).zip(fields) {
                    let value = if field.is_empty() {
                        Value::Null
                    } else {
                        typed_value(field, &columns[column])
                    };
                    grid.changes_mut().set(&rows, row, column, value);
                }
            }
            cx.notify();
        });
        self.show_value(window, cx);
    }

    fn copy_cells(&mut self, _: &CopyCells, _: &mut Window, cx: &mut Context<Self>) {
        let Some(table) = &self.table else {
            return;
        };
        let table = table.read(cx);
        let grid = table.delegate();
        let text = match table.selection() {
            // A block of cells, a line a row.
            TableSelection::Cell(..) => {
                let Some((rows, columns)) = table.selected_cell_range() else {
                    return;
                };
                rows.map(|row| {
                    columns
                        .clone()
                        .map(|column| grid.value(row, column).map(plain_text).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("	")
                })
                .collect::<Vec<_>>()
                .join(
                    "
",
                )
            }
            TableSelection::Row(row) => (0..grid.columns().len())
                .map(|column| grid.value(row, column).map(plain_text).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\t"),
            _ => return,
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let read_only = self.is_read_only(cx);
        let pending = self
            .table
            .as_ref()
            .map(|table| table.read(cx).delegate().changes().len())
            .unwrap_or_default();
        let submitting = self.submitting.is_some();
        let label = |text: SharedString| {
            div()
                .flex_none()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(text)
        };
        let tint = self
            .data_source
            .as_ref()
            .and_then(|data_source| data_source.read(cx).tint(cx));
        let commands = self
            .data_source
            .as_ref()
            .is_some_and(|data_source| data_source.read(cx).dialect().statements_are_lines());
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .when_some(tint, |toolbar, tint| toolbar.bg(tint))
            .child(
                Button::new("reload")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .tooltip_with_action(t!("table.reload").to_string(), &ReloadRows, Some(CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .child(
                Button::new("add-row")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .tooltip_with_action(t!("table.add_row").to_string(), &AddRow, Some(CONTEXT))
                    .disabled(read_only)
                    .on_click(cx.listener(|this, _, window, cx| this.add_row(&AddRow, window, cx))),
            )
            .child(
                Button::new("delete-row")
                    .ghost()
                    .small()
                    .icon(IconName::Minus)
                    .tooltip(t!("table.delete_rows").to_string())
                    .disabled(read_only)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.delete_rows(&DeleteRows, window, cx)
                        }),
                    ),
            )
            .child(div().w_px().h_4().mx_1().bg(theme.border))
            .child(
                Button::new("submit")
                    .small()
                    .when(pending > 0, |button| button.primary())
                    .when(pending == 0, |button| button.ghost())
                    .icon(IconName::Check)
                    .label(if pending > 0 {
                        t!("table.submit_count", count = pending).to_string()
                    } else {
                        t!("table.submit").to_string()
                    })
                    .loading(submitting)
                    .disabled(pending == 0 || submitting)
                    .tooltip_with_action(
                        t!("table.submit_tooltip").to_string(),
                        &SubmitChanges,
                        Some(CONTEXT),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.submit(&SubmitChanges, window, cx)),
                    ),
            )
            .child(
                Button::new("revert")
                    .ghost()
                    .small()
                    .icon(IconName::Undo2)
                    .tooltip(t!("table.revert").to_string())
                    .disabled(pending == 0)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.revert(&RevertChanges, window, cx)),
                    ),
            )
            .child(
                Button::new("preview")
                    .ghost()
                    .small()
                    .icon(IconName::FileCode)
                    .tooltip(t!("table.preview").to_string())
                    .disabled(pending == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.preview(window, cx))),
            )
            // A database of commands reads a key's value whole; there is
            // nothing to filter or order with SQL.
            .when(!commands, |toolbar| {
                toolbar
                    .child(div().w_px().h_4().mx_1().bg(theme.border))
                    .child(label("WHERE".into()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(8.))
                            .child(Input::new(&self.condition).small()),
                    )
                    .child(label("ORDER BY".into()))
                    .child(
                        div()
                            .w(rems(12.))
                            .flex_none()
                            .child(Input::new(&self.order_by).small()),
                    )
            })
            .when(commands, |toolbar| toolbar.child(div().flex_1()))
            .child(
                Button::new("value-editor")
                    .ghost()
                    .small()
                    .icon(IconName::PanelRight)
                    .selected(self.show_value_editor)
                    .tooltip(t!("table.value_editor").to_string())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.show_value_editor = !this.show_value_editor;
                        this.show_value(window, cx);
                        cx.notify();
                    })),
            )
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let grid = self.table.as_ref().map(|table| table.read(cx).delegate());
        let rows: Option<SharedString> = grid.map(|grid| {
            if grid.has_more_rows() {
                t!(
                    "results.rows_more",
                    count = format::count(grid.rows().len())
                )
                .into()
            } else {
                t!("results.rows", count = format::count(grid.rows().len())).into()
            }
        });
        let fetching = grid.is_some_and(|grid| grid.is_loading_more());
        let all_key = grid.is_some_and(|grid| {
            !grid.is_read_only()
                && self.key.len() == grid.columns().len()
                && grid.columns().len() > 1
        });
        h_flex()
            .flex_none()
            .px_2()
            .py_0p5()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .border_t_1()
            .border_color(theme.border)
            .when_some(rows, |status, rows| status.child(rows))
            .when(fetching, |status| status.child(Spinner::new().xsmall()))
            .when_some(
                grid.and_then(|grid| grid.failure().cloned()),
                |status, error| status.child(div().text_color(theme.danger).child(error)),
            )
            .child(match &self.count {
                Count::None => Button::new("count")
                    .ghost()
                    .xsmall()
                    .label(t!("table.count").to_string())
                    .on_click(cx.listener(|this, _, _, cx| this.count_rows(cx)))
                    .into_any_element(),
                Count::Counting { .. } => Spinner::new().xsmall().into_any_element(),
                Count::Counted(count) => div()
                    .child(t!("table.counted", count = format::count(*count as usize)).to_string())
                    .into_any_element(),
                Count::Failed(error) => div()
                    .text_color(theme.danger)
                    .child(error.clone())
                    .into_any_element(),
            })
            .when(self.is_read_only(cx), |status| {
                status.child(t!("table.read_only").to_string())
            })
            .when(all_key, |status| {
                status.child(
                    div()
                        .text_color(theme.warning)
                        .child(t!("table.no_key").to_string()),
                )
            })
    }

    fn render_data(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let grid: gpui_kit::AnyElement = match (&self.table, &self.loading) {
            (_, Loading::Failed(error)) => div()
                .p_4()
                .text_sm()
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element(),
            (Some(table), _) => DataTable::new(table)
                .bordered(false)
                .small()
                .into_any_element(),
            (None, _) => h_flex()
                .size_full()
                .justify_center()
                .child(Spinner::new())
                .into_any_element(),
        };
        if !self.show_value_editor {
            return div().size_full().child(grid).into_any_element();
        }
        let editable = !self.is_read_only(cx) && self.value_cell.is_some();
        h_resizable("table-value-editor")
            .child(resizable_panel().child(grid))
            .child(
                resizable_panel().size(px(320.)).child(
                    v_flex()
                        .size_full()
                        .border_l_1()
                        .border_color(theme.border)
                        .child(
                            Editor::new(&self.value_editor)
                                .bordered(false)
                                .flex_1()
                                .font_family(theme.mono_font_family.clone())
                                .text_size(theme.mono_font_size),
                        )
                        .child(
                            h_flex().p_1().justify_end().child(
                                Button::new("apply-value")
                                    .small()
                                    .outline()
                                    .label(t!("table.apply_value").to_string())
                                    .disabled(!editable)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.apply_value(window, cx)
                                    })),
                            ),
                        ),
                ),
            )
            .into_any_element()
    }
}

impl Focusable for TablePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.table {
            Some(table) => table.read(cx).focus_handle(cx),
            None => self.focus_handle.clone(),
        }
    }
}

impl BasePanel for TablePanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn dump(&self, cx: &App) -> PanelState {
        let mut state = PanelState::new(Self::NAME);
        let saved = SavedTable {
            data_source: self.data_source_id.clone(),
            schema: self.schema.to_string(),
            relation: self.relation.to_string(),
            condition: self.condition.read(cx).value().to_string(),
            order_by: self.order_by.read(cx).value().to_string(),
        };
        state.info = PanelInfo::panel(serde_json::to_value(saved).unwrap_or_default());
        state
    }
}

impl Panel for TablePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pending = self.has_changes(cx);
        h_flex()
            .gap_1()
            .child(
                Icon::new(IconName::Table).xsmall().text_color(
                    self.data_source
                        .as_ref()
                        .and_then(|data_source| data_source.read(cx).color())
                        .map_or(cx.theme().muted_foreground, |color| color.hsla(cx)),
                ),
            )
            .child(SharedString::from(self.relation.to_string()))
            .when(pending, |title| {
                title.child(div().size_1p5().rounded_full().bg(cx.theme().warning))
            })
    }

    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.relation.to_string().into())
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TablePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view: WeakEntity<Self> = cx.entity().downgrade();
        let selected = match self.tab {
            EditorTab::Data => 0,
            EditorTab::Ddl => 1,
        };
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::revert))
            .on_action(cx.listener(Self::add_row))
            .on_action(cx.listener(Self::delete_rows))
            .on_action(cx.listener(Self::revert_row))
            .on_action(cx.listener(Self::set_null))
            .on_action(cx.listener(Self::edit_cell))
            .on_action(cx.listener(Self::cancel_edit))
            .on_action(cx.listener(Self::copy_cells))
            .on_action(cx.listener(Self::paste_cells))
            .on_action(cx.listener(|this, _: &ReloadRows, window, cx| this.reload(window, cx)))
            .child(
                TabBar::new("table-tabs")
                    .small()
                    .child(Tab::new().label(t!("table.data").to_string()))
                    .child(Tab::new().label("DDL"))
                    .selected_index(selected)
                    .on_click(move |ix: &usize, _, cx| {
                        let ix = *ix;
                        let _ = view.update(cx, |this, cx| {
                            this.tab = if ix == 0 {
                                EditorTab::Data
                            } else {
                                EditorTab::Ddl
                            };
                            cx.notify();
                        });
                    }),
            )
            .map(|panel| match self.tab {
                EditorTab::Data => panel
                    .child(self.render_toolbar(cx))
                    .child(div().flex_1().min_h_0().child(self.render_data(cx)))
                    .child(self.render_status(cx)),
                EditorTab::Ddl => panel.child(
                    div().flex_1().min_h_0().child(
                        Editor::new(&self.ddl)
                            .bordered(false)
                            .h_full()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(cx.theme().mono_font_size),
                    ),
                ),
            })
    }
}
