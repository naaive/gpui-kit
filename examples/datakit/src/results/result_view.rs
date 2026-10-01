use std::{path::PathBuf, sync::Arc, time::Duration};

use datakit_driver::{ColumnInfo, Dialect};
use datakit_sql::{Lexeme, lex};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    spinner::Spinner,
    table::{DataTable, TableSelection, TableState},
    v_flex,
};
use gpui_kit::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
    div, prelude::FluentBuilder as _,
};
use rust_i18n::t;

use super::{
    CONTEXT, CopyCells, ExportContext, ExportFormat, FetchState, Page, ResultGrid, RowPages,
    export, plain_text,
};
use crate::format;

/// One result of a console run: the statement, its rows, and the commands
/// that act on them.
pub struct ResultView {
    statement: Arc<str>,
    title: SharedString,
    table: Entity<TableState<ResultGrid>>,
    dialect: Arc<dyn Dialect>,
    elapsed: Duration,
    export_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

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
        let subscriptions = vec![cx.observe(&table, |_, _, cx| cx.notify())];
        Self {
            statement,
            title,
            table,
            dialect,
            elapsed,
            export_task: None,
            _subscriptions: subscriptions,
        }
    }

    /// What the result's tab is called: the table it reads, when there is one.
    pub fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn copy_cells(&mut self, _: &CopyCells, _: &mut Window, cx: &mut Context<Self>) {
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
        let directory = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!(
            "{}.{}",
            source_table(&self.statement).unwrap_or_else(|| "result".into()),
            format.extension()
        );
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        self.export_task = Some(cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let write = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, text) }
                })
                .await;
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
            .when(
                matches!(grid.fetch_state(), FetchState::Fetching { .. }),
                |row| row.child(Spinner::new().xsmall()),
            )
            .when_some(
                match grid.fetch_state() {
                    FetchState::Failed(error) => Some(error.clone()),
                    _ => None,
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
                                        Self::format_menu(view.clone(), menu, Self::export_to_file)
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(&self.table).bordered(false).small()),
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
