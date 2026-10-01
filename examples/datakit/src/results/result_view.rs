use std::{path::PathBuf, sync::Arc, time::Duration};

use datakit_driver::{ColumnInfo, Dialect};
use datakit_sql::{Lexeme, filtered_statement, lex};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    table::{DataTable, TableSelection, TableState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window, div, prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use super::{
    CONTEXT, CopyCells, ExportContext, ExportFormat, FetchState, Page, ResultGrid, RowPages,
    aggregate::{Aggregates, number_text},
    export, export_xlsx, plain_text,
};
use crate::format;

/// One result of a console run: the statement, its rows, and the commands
/// that act on them.
pub struct ResultView {
    /// The statement the console ran.
    statement: Arc<str>,
    title: SharedString,
    table: Entity<TableState<ResultGrid>>,
    condition: Entity<InputState>,
    order_by: Entity<InputState>,
    /// Whether the console is running the filtered statement.
    filtering: bool,
    filter_error: Option<SharedString>,
    /// Whether the selected row is shown beside the grid, one column a
    /// line.
    show_record: bool,
    dialect: Arc<dyn Dialect>,
    elapsed: Duration,
    export_task: Option<Task<()>>,
    _table_subscription: Subscription,
    _subscriptions: Vec<Subscription>,
}

/// What a result asks of the console that ran it.
pub enum ResultViewEvent {
    /// Run this statement and show its rows in place of the current ones.
    Filter(Arc<str>),
}

impl EventEmitter<ResultViewEvent> for ResultView {}

impl ResultView {
    /// A view of `first_page`, fetching the rest from `pages` as the grid
    /// scrolls.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        statement: Arc<str>,
        columns: Arc<[ColumnInfo]>,
        first_page: Page,
        pages: RowPages,
        elapsed: Duration,
        dialect: Arc<dyn Dialect>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title = source_table(&statement)
            .map(SharedString::from)
            .unwrap_or_else(|| t!("results.untitled").into());
        let grid = ResultGrid::new(columns, first_page, pages, window);
        let table = cx.new(|cx| TableState::new(grid, window, cx).cell_selectable(true));
        let table_subscription = cx.observe(&table, |_, _, cx| cx.notify());
        let filter = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let condition = filter("id > 100", window, cx);
        let order_by = filter("id DESC", window, cx);
        let subscriptions = [&condition, &order_by]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.apply_filter(cx);
                    }
                })
            })
            .collect();
        Self {
            statement,
            title,
            table,
            condition,
            order_by,
            filtering: false,
            filter_error: None,
            show_record: false,
            dialect,
            elapsed,
            export_task: None,
            _table_subscription: table_subscription,
            _subscriptions: subscriptions,
        }
    }

    /// The aggregates of the selected column over the rows fetched, or of
    /// the selected block of cells.
    fn column_summary(&self, cx: &Context<Self>) -> Option<SharedString> {
        let aggregates = if let Some(block) = self.selected_block(cx) {
            Aggregates::of(block.into_iter().flatten())
        } else {
            let table = self.table.read(cx);
            let TableSelection::Column(col_ix) = table.selection() else {
                return None;
            };
            let grid = table.delegate();
            let source = grid.source_column(col_ix);
            Aggregates::of(grid.displayed_rows().map(|row| &row[source]))
        };
        let mut parts = vec![
            t!(
                "results.aggregate.count",
                count = format::count(aggregates.count())
            )
            .to_string(),
            t!(
                "results.aggregate.distinct",
                count = format::count(aggregates.distinct())
            )
            .to_string(),
            t!(
                "results.aggregate.nulls",
                count = format::count(aggregates.nulls())
            )
            .to_string(),
        ];
        if let Some(numbers) = aggregates.numbers() {
            parts.extend([
                t!("results.aggregate.sum", value = number_text(numbers.sum())).to_string(),
                t!(
                    "results.aggregate.average",
                    value = number_text(numbers.average())
                )
                .to_string(),
                t!("results.aggregate.min", value = number_text(numbers.min())).to_string(),
                t!("results.aggregate.max", value = number_text(numbers.max())).to_string(),
            ]);
        }
        Some(parts.join(" · ").into())
    }

    /// The row the record view shows: the selected one, or the first.
    fn record_row(&self, cx: &Context<Self>) -> Option<usize> {
        let table = self.table.read(cx);
        let row_ix = match table.selection() {
            TableSelection::Row(row_ix) | TableSelection::Cell(row_ix, _) => row_ix,
            TableSelection::None | TableSelection::Column(_) => 0,
        };
        (row_ix < table.delegate().rows_fetched()).then_some(row_ix)
    }

    /// The selected row, one column a line: easier to read than a wide row.
    fn render_record(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let grid = self.table.read(cx).delegate();
        let row = self.record_row(cx).map(|row_ix| (row_ix, grid.row(row_ix)));
        v_flex()
            .id("record")
            .flex_none()
            .w(rems(20.))
            .h_full()
            .border_l_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(match &row {
                        Some((row_ix, _)) => {
                            t!("results.record_row", row = format::count(row_ix + 1)).to_string()
                        }
                        None => t!("results.record_empty").to_string(),
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when_some(row, |list, (_, row)| {
                        list.children((0..grid.columns().len()).map(|col_ix| {
                            let source = grid.source_column(col_ix);
                            let column = &grid.columns()[source];
                            let value = row[source].display().map(|text| text.into_owned());
                            v_flex()
                                .px_2()
                                .py_1()
                                .gap_0p5()
                                .border_b_1()
                                .border_color(theme.border.opacity(0.5))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .text_xs()
                                        .child(column.name().to_string())
                                        .child(
                                            div()
                                                .text_color(theme.muted_foreground)
                                                .child(column.type_name().to_string()),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family(theme.mono_font_family.clone())
                                        .map(|text| match value {
                                            Some(value) => text.child(value),
                                            None => text
                                                .italic()
                                                .text_color(theme.muted_foreground)
                                                .child("NULL"),
                                        }),
                                )
                        }))
                    }),
            )
    }

    /// Ask the console to read the statement again through the filter, on
    /// the server, so the filter sees every row and not only those fetched.
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        if self.filtering {
            return;
        }
        let statement: Arc<str> = filtered_statement(
            &self.statement,
            &self.condition.read(cx).value(),
            &self.order_by.read(cx).value(),
        )
        .into();
        self.filtering = true;
        self.filter_error = None;
        // The rows still on the server hold the session; let them go.
        self.table
            .update(cx, |table, _| table.delegate_mut().stop_fetching());
        cx.emit(ResultViewEvent::Filter(statement));
        cx.notify();
    }

    /// Show the rows the console read through the filter.
    pub fn show_filtered(
        &mut self,
        columns: Arc<[ColumnInfo]>,
        first_page: Page,
        pages: RowPages,
        elapsed: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let grid = ResultGrid::new(columns, first_page, pages, window);
        self.table = cx.new(|cx| TableState::new(grid, window, cx).cell_selectable(true));
        self._table_subscription = cx.observe(&self.table, |_, _, cx| cx.notify());
        self.elapsed = elapsed;
        self.filtering = false;
        cx.notify();
    }

    /// The filtered statement failed; the rows shown are the earlier ones.
    pub fn filter_failed(&mut self, message: SharedString, cx: &mut Context<Self>) {
        self.filtering = false;
        self.filter_error = Some(message);
        cx.notify();
    }

    #[cfg(test)]
    pub fn condition(&self) -> &Entity<InputState> {
        &self.condition
    }

    #[cfg(test)]
    pub fn displayed_rows(&self, cx: &gpui_kit::App) -> Vec<datakit_driver::Row> {
        self.table
            .read(cx)
            .delegate()
            .displayed_rows()
            .cloned()
            .collect()
    }

    /// What the result's tab is called: the table it reads, when there is one.
    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    /// The selected block of cells when it is more than one cell, as rows
    /// of values.
    fn selected_block<'a>(
        &self,
        cx: &'a Context<Self>,
    ) -> Option<Vec<Vec<&'a datakit_driver::Value>>> {
        let table = self.table.read(cx);
        let (rows, cols) = table.selected_cell_range()?;
        if rows.len() * cols.len() < 2 {
            return None;
        }
        let grid = table.delegate();
        Some(
            rows.map(|row_ix| {
                cols.clone()
                    .map(|col_ix| &grid.row(row_ix)[grid.source_column(col_ix)])
                    .collect()
            })
            .collect(),
        )
    }

    fn copy_cells(&mut self, _: &CopyCells, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(block) = self.selected_block(cx) {
            let text = block
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|value| plain_text(value))
                        .collect::<Vec<_>>()
                        .join("	")
                })
                .collect::<Vec<_>>()
                .join(
                    "
",
                );
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            return;
        }
        let table = self.table.read(cx);
        let grid = table.delegate();
        let text = match table.selection() {
            TableSelection::None => return,
            TableSelection::Cell(row_ix, col_ix) => {
                plain_text(&grid.row(row_ix)[grid.source_column(col_ix)])
            }
            TableSelection::Row(row_ix) => grid
                .row(row_ix)
                .iter()
                .map(plain_text)
                .collect::<Vec<_>>()
                .join("\t"),
            TableSelection::Column(col_ix) => {
                let source = grid.source_column(col_ix);
                grid.displayed_rows()
                    .map(|row| plain_text(&row[source]))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn render_all(&self, format: ExportFormat, cx: &Context<Self>) -> String {
        let grid = self.table.read(cx).delegate();
        let table = source_table(&self.statement).unwrap_or_else(|| "my_table".into());
        let context = ExportContext {
            columns: grid.columns(),
            dialect: &*self.dialect,
            table: &table,
        };
        export(format, &context, grid.displayed_rows())
    }

    fn copy_as(&mut self, format: ExportFormat, cx: &mut Context<Self>) {
        let text = self.render_all(format, cx);
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn export_to_file(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.render_all(format, cx);
        self.save_export(format.extension(), Ok(text.into_bytes()), window, cx);
    }

    fn export_to_xlsx(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let grid = self.table.read(cx).delegate();
        let table = source_table(&self.statement).unwrap_or_default();
        let context = ExportContext {
            columns: grid.columns(),
            dialect: &*self.dialect,
            table: &table,
        };
        let workbook = export_xlsx(&context, grid.displayed_rows());
        self.save_export("xlsx", workbook, window, cx);
    }

    /// Ask where to save `contents` and write them there.
    fn save_export(
        &mut self,
        extension: &str,
        contents: anyhow::Result<Vec<u8>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let directory = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!(
            "{}.{}",
            source_table(&self.statement).unwrap_or_else(|| "result".into()),
            extension
        );
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        self.export_task = Some(cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let write = match contents {
                Ok(contents) => {
                    cx.background_spawn({
                        let path = path.clone();
                        async move { std::fs::write(&path, contents).map_err(anyhow::Error::from) }
                    })
                    .await
                }
                Err(error) => Err(error),
            };
            let _ = cx.update(|window, cx| match write {
                Ok(()) => {}
                Err(error) => window.push_notification(
                    Notification::error(
                        t!(
                            "results.export_failed",
                            path = path.display().to_string(),
                            error = error.to_string()
                        )
                        .to_string(),
                    ),
                    cx,
                ),
            });
        }));
    }

    fn render_status(&self, cx: &Context<Self>) -> impl IntoElement {
        let grid = self.table.read(cx).delegate();
        let rows = grid.rows_fetched();
        let count: SharedString = if grid.has_more_rows() {
            t!("results.rows_more", count = format::count(rows)).into()
        } else {
            t!("results.rows", count = format::count(rows)).into()
        };
        h_flex()
            .gap_2()
            .min_w_0()
            .child(count)
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format::duration(self.elapsed)),
            )
            .when_some(self.column_summary(cx), |row, summary| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(cx.theme().muted_foreground)
                        .child(summary),
                )
            })
            .when(
                self.filtering || matches!(grid.fetch_state(), FetchState::Fetching { .. }),
                |row| row.child(Spinner::new().xsmall()),
            )
            .when_some(
                match grid.fetch_state() {
                    FetchState::Failed(error) => Some(error.clone()),
                    _ => self.filter_error.clone(),
                },
                |row, error| {
                    row.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                },
            )
    }

    fn format_menu(
        view: WeakEntity<Self>,
        menu: PopupMenu,
        action: fn(&mut Self, ExportFormat, &mut Window, &mut Context<Self>),
    ) -> PopupMenu {
        ExportFormat::ALL.iter().fold(menu, |menu, format| {
            let format = *format;
            let view = view.clone();
            menu.item(
                PopupMenuItem::new(format_title(format)).on_click(move |_, window, cx| {
                    let _ = view.update(cx, |view, cx| action(view, format, window, cx));
                }),
            )
        })
    }
}

fn format_title(format: ExportFormat) -> SharedString {
    match format {
        ExportFormat::Tsv => t!("results.format.tsv"),
        ExportFormat::Csv => t!("results.format.csv"),
        ExportFormat::Json => t!("results.format.json"),
        ExportFormat::SqlInsert => t!("results.format.sql_insert"),
        ExportFormat::Markdown => t!("results.format.markdown"),
    }
    .into()
}

/// The relation a `SELECT … FROM name` reads, as written, for naming the
/// result and its exports.
fn source_table(statement: &str) -> Option<String> {
    let tokens: Vec<_> = lex(statement)
        .into_iter()
        .filter(|token| token.lexeme().is_significant())
        .collect();
    let from = tokens
        .iter()
        .position(|token| token.is_keyword(statement, "from"))?;
    // `name` or `schema.name`: identifiers joined by periods.
    let mut parts = Vec::new();
    let mut rest = tokens[from + 1..].iter();
    while let Some(token) = rest.next() {
        if !matches!(token.lexeme(), Lexeme::Word | Lexeme::QuotedIdentifier) {
            break;
        }
        parts.push(token.text(statement));
        if !rest
            .next()
            .is_some_and(|token| token.lexeme() == Lexeme::Period)
        {
            break;
        }
    }
    (!parts.is_empty()).then(|| parts.join("."))
}

impl Render for ResultView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::copy_cells))
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .justify_between()
                    .text_sm()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.render_status(cx))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1()
                            .child(
                                Button::new("record")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::PanelRight)
                                    .selected(self.show_record)
                                    .tooltip(t!("results.record").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.show_record = !this.show_record;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("copy-as")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Copy)
                                    .label(t!("results.copy_as").to_string())
                                    .dropdown_menu({
                                        let view = view.clone();
                                        move |menu, _, _| {
                                            Self::format_menu(
                                                view.clone(),
                                                menu,
                                                |view, format, _, cx| view.copy_as(format, cx),
                                            )
                                        }
                                    }),
                            )
                            .child(
                                Button::new("export")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Download)
                                    .label(t!("results.export").to_string())
                                    .dropdown_menu(move |menu, _, _| {
                                        let xlsx = view.clone();
                                        Self::format_menu(view.clone(), menu, Self::export_to_file)
                                            .item(
                                                PopupMenuItem::new(
                                                    t!("results.format.xlsx").to_string(),
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = xlsx.update(cx, |view, cx| {
                                                        view.export_to_xlsx(window, cx)
                                                    });
                                                }),
                                            )
                                    }),
                            ),
                    ),
            )
            .child(self.render_filter(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(DataTable::new(&self.table).bordered(false).small()),
                    )
                    .when(self.show_record, |body| body.child(self.render_record(cx))),
            )
    }
}

impl ResultView {
    /// The `WHERE` and `ORDER BY` the result is read through; Enter in
    /// either runs it.
    fn render_filter(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let label = |text: &'static str| {
            div()
                .flex_none()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(text)
        };
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .child(label("WHERE"))
            .child(
                div()
                    .flex_1()
                    .min_w(rems(8.))
                    .child(Input::new(&self.condition).xsmall()),
            )
            .child(label("ORDER BY"))
            .child(
                div()
                    .w(rems(12.))
                    .flex_none()
                    .child(Input::new(&self.order_by).xsmall()),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use datakit_driver::{Row, TypeCategory, Value};
    use datakit_driver_postgres::PostgresDialect;
    use datakit_runtime::IoRuntime;
    use futures::StreamExt as _;
    use gpui_kit::component::table::{ColumnSort, TableDelegate as _};
    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size,
    };

    use super::*;
    use crate::results::result_grid::PAGE_SIZE;

    /// `count` rows of `(id, score)`, where scores are a shuffle of ids.
    fn rows(count: i64) -> Vec<anyhow::Result<Row>> {
        (0..count)
            .map(|id| Ok(vec![Value::Int(id), Value::Int((id * 7919) % count)].into()))
            .collect()
    }

    fn open(cx: &mut TestAppContext, runtime: &IoRuntime, count: i64) -> Entity<ResultView> {
        cx.update(gpui_kit::init);
        let mut pages = runtime.forward(futures::stream::iter(rows(count)).chunks(PAGE_SIZE), 1);
        let first = runtime.block_on(Page::fetch(&mut pages));
        let columns: Arc<[ColumnInfo]> = vec![
            ColumnInfo::new("id", "integer", TypeCategory::Integer),
            ColumnInfo::new("score", "integer", TypeCategory::Integer),
        ]
        .into();
        cx.update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(800.), px(600.)),
                    cx,
                ))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    ResultView::new(
                        "select id, score from scores".into(),
                        columns,
                        first,
                        pages,
                        Duration::from_millis(5),
                        Arc::new(PostgresDialect),
                        window,
                        cx,
                    )
                })
            })
            .expect("open a window")
            .1
        })
    }

    /// Run the app until the grid has `rows` rows; pages arrive from the IO
    /// runtime's threads, outside the test executor.
    fn wait_for_rows(view: &Entity<ResultView>, rows: usize, cx: &mut TestAppContext) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            let fetched =
                view.read_with(cx, |view, cx| view.table.read(cx).delegate().rows_fetched());
            if fetched >= rows {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "only {fetched} of {rows} rows arrived"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[gpui_kit::test]
    fn fetching_the_next_page_keeps_the_sort(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = IoRuntime::new().unwrap();
        let view = open(cx, &runtime, 1200);

        view.update(cx, |view, cx| {
            view.table.update(cx, |table, cx| {
                let grid = table.delegate();
                assert_eq!(grid.rows_fetched(), PAGE_SIZE);
                assert!(grid.has_more_rows());
                cx.notify();
            })
        });
        cx.update_window(cx.windows()[0], |_, window, cx| {
            view.update(cx, |view, cx| {
                view.table.update(cx, |table, cx| {
                    table
                        .delegate_mut()
                        .perform_sort(1, ColumnSort::Descending, window, cx);
                    table.delegate_mut().fetch_next_page(cx);
                })
            })
        })
        .unwrap();
        wait_for_rows(&view, 2 * PAGE_SIZE, cx);

        view.read_with(cx, |view, cx| {
            let grid = view.table.read(cx).delegate();
            let scores: Vec<i64> = grid
                .displayed_rows()
                .map(|row| match row[1] {
                    Value::Int(score) => score,
                    _ => unreachable!(),
                })
                .collect();
            assert_eq!(scores.len(), 2 * PAGE_SIZE);
            assert!(
                scores.windows(2).all(|pair| pair[0] >= pair[1]),
                "rows from the second page are sorted in"
            );
            assert!(grid.has_more_rows(), "200 rows are still on the server");
        });
    }

    #[gpui_kit::test]
    fn the_last_short_page_completes_the_result(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = IoRuntime::new().unwrap();
        let view = open(cx, &runtime, 700);

        view.update(cx, |view, cx| {
            view.table
                .update(cx, |table, cx| table.delegate_mut().fetch_next_page(cx))
        });
        wait_for_rows(&view, 700, cx);
        view.read_with(cx, |view, cx| {
            let grid = view.table.read(cx).delegate();
            assert!(!grid.has_more_rows());
            assert!(matches!(grid.fetch_state(), FetchState::Complete));
        });
    }

    #[test]
    fn results_are_named_after_the_table_they_read() {
        assert_eq!(
            source_table("select * from orders o where 1=1").as_deref(),
            Some("orders")
        );
        assert_eq!(
            source_table("SELECT a FROM public.\"Line Items\"").as_deref(),
            Some("public.\"Line Items\"")
        );
        assert_eq!(source_table("select 1"), None);
    }
}
