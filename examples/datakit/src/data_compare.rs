//! Data comparison: how the rows of one table differ from another's, and
//! the statements that make the second hold what the first does, as
//! DataGrip's Compare Content shows them.
//!
//! The tables can be in different data sources. Rows are matched on the
//! source table's key and compared on the columns both tables have.

use std::sync::Arc;

use datakit_driver::{Row, RowsDiff, StatementOutcome};
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent},
    h_flex,
    input::{Editor, EditorState},
    select::{Select, SelectItem, SelectState},
    spinner::Spinner,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Task, Window, div,
    prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;

use crate::{
    datasource::{DataSources, describe_error},
    format,
    navigation::{Navigation, NavigationEvent},
    objects::{self, CatalogObject, ObjectPath, ObjectRef},
    services::Services,
};

/// The most rows read from each table.
const ROW_LIMIT: usize = 100_000;

/// One table of one data source.
#[derive(Clone)]
struct TableChoice {
    object: ObjectRef,
    title: SharedString,
    key: SharedString,
}

impl SelectItem for TableChoice {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &SharedString {
        &self.key
    }
}

enum Comparison {
    Idle,
    Running { _task: Task<()> },
    Done(RowsDiff),
    Failed(SharedString),
}

/// The data comparison window.
pub struct DataComparePanel {
    focus_handle: FocusHandle,
    source: Entity<SelectState<Vec<TableChoice>>>,
    target: Entity<SelectState<Vec<TableChoice>>>,
    choices: Vec<TableChoice>,
    script: Entity<EditorState>,
    comparison: Comparison,
    /// Whether a table had more rows than were read.
    truncated: bool,
}

impl EventEmitter<PanelEvent> for DataComparePanel {}

impl DataComparePanel {
    pub const NAME: &str = "DataCompare";

    /// A comparison with `source` already chosen, and as the target the
    /// table of the same name in another data source, when there is one.
    pub fn new(source: Option<&ObjectRef>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let choices = table_choices(cx);
        let source_ix =
            source.and_then(|source| choices.iter().position(|choice| &choice.object == source));
        let target_ix = source.and_then(|source| {
            choices.iter().position(|choice| {
                choice.object.data_source() != source.data_source()
                    && choice.object.path().name() == source.path().name()
            })
        });
        let select = |ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>| {
            let choices = choices.clone();
            cx.new(|cx| SelectState::new(choices, ix.map(IndexPath::new), window, cx))
        };
        let source_state = select(source_ix, window, cx);
        let target_state = select(target_ix, window, cx);
        let script = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
        });
        Self {
            focus_handle: cx.focus_handle(),
            source: source_state,
            target: target_state,
            choices,
            script,
            comparison: Comparison::Idle,
            truncated: false,
        }
    }

    fn chosen(&self, state: &Entity<SelectState<Vec<TableChoice>>>, cx: &App) -> Option<ObjectRef> {
        let key = state.read(cx).selected_value()?.clone();
        self.choices
            .iter()
            .find(|choice| choice.key == key)
            .map(|choice| choice.object.clone())
    }

    /// Read both tables and compare them.
    pub fn compare(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(source), Some(target)) =
            (self.chosen(&self.source, cx), self.chosen(&self.target, cx))
        else {
            self.comparison = Comparison::Failed(t!("data_compare.choose").into());
            cx.notify();
            return;
        };
        let Some(key) = key_columns(&source, cx) else {
            self.comparison = Comparison::Failed(t!("data_compare.no_columns").into());
            cx.notify();
            return;
        };
        let source_rows = read_rows(&source, cx);
        let target_rows = read_rows(&target, cx);
        self.comparison = Comparison::Running {
            _task: cx.spawn_in(window, async move |this, cx| {
                let result = futures::try_join!(source_rows, target_rows);
                let _ = this.update_in(cx, |this, window, cx| {
                    match result {
                        Ok((source_rows, target_rows)) => {
                            this.show(&target, &key, source_rows, target_rows, window, cx)
                        }
                        Err(error) => {
                            this.comparison = Comparison::Failed(describe_error(&error));
                        }
                    }
                    cx.notify();
                });
            }),
        };
        cx.notify();
    }

    fn show(
        &mut self,
        target: &ObjectRef,
        key: &[Arc<str>],
        (source_columns, source_rows): (Vec<Arc<str>>, Vec<Row>),
        (target_columns, target_rows): (Vec<Arc<str>>, Vec<Row>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.truncated = source_rows.len() > ROW_LIMIT || target_rows.len() > ROW_LIMIT;
        // The columns both tables have, in the source's order.
        let shared: Vec<(usize, usize)> = source_columns
            .iter()
            .enumerate()
            .filter_map(|(ix, name)| {
                let target_ix = target_columns.iter().position(|other| other == name)?;
                Some((ix, target_ix))
            })
            .collect();
        let columns: Vec<Arc<str>> = shared
            .iter()
            .map(|(ix, _)| source_columns[*ix].clone())
            .collect();
        let key: Vec<usize> = key
            .iter()
            .filter_map(|name| columns.iter().position(|column| column == name))
            .collect();
        let key = if key.is_empty() {
            (0..columns.len()).collect()
        } else {
            key
        };
        let project = |rows: &[Row], pick: &dyn Fn(&(usize, usize)) -> usize| -> Vec<Row> {
            rows.iter()
                .take(ROW_LIMIT)
                .map(|row| shared.iter().map(|pair| row[pick(pair)].clone()).collect())
                .collect()
        };
        let source_rows = project(&source_rows, &|(ix, _)| *ix);
        let target_rows = project(&target_rows, &|(_, ix)| *ix);
        let diff = RowsDiff::new(columns, key, &source_rows, &target_rows);
        let target_source = target.data_source().read(cx);
        let dialect = target_source.dialect();
        let schema = target.path().schema().clone();
        let relation = target.path().name();
        let statements: Vec<String> = diff
            .changes_to_target()
            .iter()
            .map(|change| dialect.row_change(&schema, &relation, change))
            .collect();
        self.script.update(cx, |editor, cx| {
            editor.set_value(objects::script(&statements), window, cx);
        });
        self.comparison = Comparison::Done(diff);
    }

    #[cfg(test)]
    pub fn result(&self, cx: &App) -> Option<(RowsDiff, String)> {
        match &self.comparison {
            Comparison::Done(diff) => {
                Some((diff.clone(), self.script.read(cx).value().to_string()))
            }
            _ => None,
        }
    }

    fn open_in_console(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.chosen(&self.target, cx) else {
            return;
        };
        let sql = self.script.read(cx).value().to_string();
        if sql.trim().is_empty() {
            return;
        }
        Navigation::request(
            NavigationEvent::OpenConsole {
                object: target,
                sql,
            },
            cx,
        );
    }

    fn summary(&self) -> Option<SharedString> {
        let Comparison::Done(diff) = &self.comparison else {
            return None;
        };
        let mut summary = if diff.is_empty() {
            t!(
                "data_compare.identical",
                rows = format::count(diff.identical())
            )
            .to_string()
        } else {
            t!(
                "data_compare.summary",
                only_source = format::count(diff.only_in_source().len()),
                only_target = format::count(diff.only_in_target().len()),
                changed = format::count(diff.changed().len()),
                identical = format::count(diff.identical())
            )
            .to_string()
        };
        if self.truncated {
            summary.push(' ');
            summary.push_str(&t!(
                "data_compare.truncated",
                limit = format::count(ROW_LIMIT)
            ));
        }
        Some(summary.into())
    }
}

/// Every table of the loaded schemas of every data source.
fn table_choices(cx: &App) -> Vec<TableChoice> {
    let mut choices = Vec::new();
    for data_source in DataSources::global(cx).read(cx).items() {
        let source = data_source.read(cx);
        for schema in source.catalog().schemas() {
            for relation in schema.relations().unwrap_or_default() {
                if !relation.relation_type().is_table() {
                    continue;
                }
                let path = ObjectPath::relation(schema.name(), relation.name());
                choices.push(TableChoice {
                    title: format!("{}.{} · {}", schema.name(), relation.name(), source.name())
                        .into(),
                    key: format!("{}/{}", source.profile().id(), path.key()).into(),
                    object: ObjectRef::new(data_source.clone(), path),
                });
            }
        }
    }
    choices
}

/// The columns that identify a row of `table`: its key, or every column.
fn key_columns(table: &ObjectRef, cx: &App) -> Option<Vec<Arc<str>>> {
    match table
        .path()
        .resolve(table.data_source().read(cx).catalog())?
    {
        CatalogObject::Relation(relation) => Some(
            relation
                .row_identity()
                .iter()
                .map(|column| column.name())
                .collect(),
        ),
        _ => None,
    }
}

/// Up to one more than [`ROW_LIMIT`] rows of `table` in its key's order,
/// with its column names, so a caller can tell there were more.
fn read_rows(table: &ObjectRef, cx: &App) -> Task<anyhow::Result<(Vec<Arc<str>>, Vec<Row>)>> {
    let source = table.data_source().read(cx);
    let dialect = source.dialect();
    let order: String = key_columns(table, cx)
        .unwrap_or_default()
        .iter()
        .map(|column| dialect.quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql: Arc<str> = dialect
        .select_page(
            table.path().schema(),
            &table.path().name(),
            "",
            &order,
            ROW_LIMIT as u64 + 1,
            0,
        )
        .into();
    let session = source.open_session(cx);
    let work = Services::global(cx).spawn(async move {
        let connection = session.await?;
        match connection.execute(sql).await? {
            StatementOutcome::Rows(mut stream) => {
                let columns = stream
                    .columns()
                    .iter()
                    .map(|column| column.name())
                    .collect();
                let mut rows = Vec::new();
                while let Some(row) = stream.next().await {
                    rows.push(row?);
                }
                Ok((columns, rows))
            }
            StatementOutcome::Command(_) => Ok((Vec::new(), Vec::new())),
        }
    });
    cx.background_spawn(work)
}

impl Focusable for DataComparePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for DataComparePanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }
}

impl Panel for DataComparePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Icon::new(IconName::Diff)
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(t!("data_compare.title").to_string())
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for DataComparePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let has_script = !self.script.read(cx).value().trim().is_empty();
        let running = matches!(self.comparison, Comparison::Running { .. });
        let status: Option<(SharedString, bool)> = match &self.comparison {
            Comparison::Idle => Some((t!("data_compare.choose").into(), false)),
            Comparison::Running { .. } => None,
            Comparison::Done(_) => self.summary().map(|summary| (summary, false)),
            Comparison::Failed(error) => Some((error.clone(), true)),
        };
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_sm().child(t!("compare.source").to_string()))
                    .child(div().w(rems(16.)).child(Select::new(&self.source).small()))
                    .child(Icon::new(IconName::ArrowRight).xsmall())
                    .child(div().text_sm().child(t!("compare.target").to_string()))
                    .child(div().w(rems(16.)).child(Select::new(&self.target).small()))
                    .child(
                        Button::new("compare")
                            .small()
                            .primary()
                            .label(t!("data_compare.compare").to_string())
                            .loading(running)
                            .disabled(running)
                            .on_click(cx.listener(|this, _, window, cx| this.compare(window, cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("copy-script")
                            .ghost()
                            .small()
                            .icon(IconName::Copy)
                            .tooltip(t!("compare.copy").to_string())
                            .disabled(!has_script)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let script = this.script.read(cx).value().to_string();
                                cx.write_to_clipboard(ClipboardItem::new_string(script));
                            })),
                    )
                    .child(
                        Button::new("script-to-console")
                            .small()
                            .outline()
                            .icon(IconName::SquareTerminal)
                            .label(t!("compare.to_console").to_string())
                            .disabled(!has_script)
                            .on_click(cx.listener(|this, _, _, cx| this.open_in_console(cx))),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .border_b_1()
                    .border_color(theme.border)
                    .when(running, |row| {
                        row.child(Spinner::new().xsmall())
                            .child(t!("data_compare.reading").to_string())
                    })
                    .when_some(status, |row, (text, failed)| {
                        row.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(if failed {
                                    theme.danger
                                } else {
                                    theme.muted_foreground
                                })
                                .child(text),
                        )
                    }),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(&self.script)
                        .bordered(false)
                        .h_full()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(theme.mono_font_size),
                ),
            )
    }
}
