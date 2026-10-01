use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use datakit_driver::{Connection, DataSourceId, DatabaseError, StatementOutcome};
use datakit_sql::{
    FormatStyle, Inspection, Severity, Target, format_sql, inspect, parameters, resolve,
    split_statements, statement_at, substitute, templates::CARET,
};
use datakit_store::{HistoryEntry, HistoryOutcome};
use futures::StreamExt as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dock::{BasePanel, Panel, PanelEvent, PanelInfo, PanelState},
    h_flex,
    highlighter::{Diagnostic, DiagnosticSeverity},
    input::{Editor, EditorState, InputEvent, RopeExt as _},
    menu::{DropdownMenu as _, PopupMenuItem},
    notification::Notification,
    resizable::{resizable_panel, v_resizable},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, prelude::FluentBuilder as _, rems,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};

use super::{
    CONTEXT, CancelExecution, Commit, ExecuteStatement, ExplainAnalyze, ExplainPlan, FormatSql,
    QuickDocumentation, RenameAlias, Rollback, SaveConsoleAs,
    completion::SqlCompletion,
    intelligence::{SqlIntelligence, problem_message},
    parameters_dialog::ParametersDialog,
};
use crate::{
    datasource::{ConnectionStatus, DataSource, DataSourceEvent, DataSources, describe_error},
    format,
    history::HistoryLog,
    navigation::{Navigation, NavigationEvent},
    objects::{ObjectPath, ObjectRef},
    results::{Page, PlanView, ResultView, result_pages},
    services::Services,
};

/// What a console's session is doing, for the Services window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Disconnected,
    Idle,
    InTransaction,
    Running {
        started: Instant,
        /// The 1-based statement running, of `total`; 0 while connecting.
        statement: usize,
        total: usize,
    },
}

/// Whether each statement commits by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransactionMode {
    #[default]
    Auto,
    /// Statements run in a transaction the person commits or rolls back.
    Manual,
}

/// A console's identity in a saved layout, and the file its text lives in.
#[derive(Serialize, Deserialize)]
struct SavedConsole {
    id: String,
    name: String,
    data_source: DataSourceId,
    #[serde(default)]
    transaction_mode: TransactionMode,
    /// The SQL file the console edits, when it is not a scratch console.
    #[serde(default)]
    file: Option<PathBuf>,
}

/// What running a set of statements does with them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunMode {
    Execute,
    /// Show the plan of the first statement, running it when `analyze`.
    Explain {
        analyze: bool,
    },
}

/// A result tab.
enum ResultEntry {
    Rows(Entity<ResultView>),
    Plan {
        view: Entity<PlanView>,
        title: SharedString,
    },
}

impl ResultEntry {
    fn title(&self, cx: &App) -> SharedString {
        match self {
            ResultEntry::Rows(view) => view.read(cx).title(),
            ResultEntry::Plan { title, .. } => title.clone(),
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            ResultEntry::Rows(view) => view.clone().into_any_element(),
            ResultEntry::Plan { view, .. } => view.clone().into_any_element(),
        }
    }
}

/// A SQL editor bound to one data source, with a session of its own.
pub struct ConsolePanel {
    id: SharedString,
    name: SharedString,
    data_source_id: DataSourceId,
    /// `None` when the data source was removed after the console was saved,
    /// or a file was opened without one.
    data_source: Option<Entity<DataSource>>,
    /// The SQL file the text is saved to; a scratch console's text lives in
    /// the data directory.
    file: Option<PathBuf>,
    editor: Entity<EditorState>,
    inspections: Rc<RefCell<Vec<Inspection>>>,
    session: Option<Arc<dyn Connection>>,
    transaction_mode: TransactionMode,
    in_transaction: bool,
    run: Option<Run>,
    results: Vec<ResultEntry>,
    output: Vec<OutputLine>,
    active_tab: ResultTab,
    parameter_values: HashMap<String, String>,
    /// Where the server said the last failure was, kept until the next edit.
    server_error: Option<Diagnostic>,
    save_task: Option<Task<()>>,
    inspect_task: Option<Task<()>>,
    data_source_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

/// Statements being run.
struct Run {
    started: Instant,
    /// The 1-based statement running, of `total`.
    statement: usize,
    total: usize,
    cancelling: bool,
    _task: Task<()>,
    /// Redraws the elapsed time while the run lasts.
    _ticker: Task<()>,
}

#[derive(Clone)]
struct OutputLine {
    time_ms: i64,
    text: SharedString,
    failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResultTab {
    Output,
    Result(usize),
}

/// The most output lines a console keeps.
const OUTPUT_LIMIT: usize = 1000;

/// Marks a statement whose text is not the editor's, so a server error
/// position cannot be underlined in it.
const DETACHED: usize = usize::MAX;

impl EventEmitter<PanelEvent> for ConsolePanel {}

impl ConsolePanel {
    pub const NAME: &str = "Console";

    /// A new console for `data_source`, called `name`, holding `text`.
    pub fn new(
        data_source: &Entity<DataSource>,
        name: SharedString,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let data_source_id = data_source.read(cx).profile().id().clone();
        let mut console = Self::build(
            new_console_id(),
            name,
            data_source_id,
            Some(data_source.clone()),
            None,
            text,
            TransactionMode::default(),
            window,
            cx,
        );
        if !text.is_empty() {
            console.schedule_save(cx);
        }
        console
    }

    /// A console editing the SQL file at `path`, which holds `text`, run
    /// against `data_source` when there is one.
    pub fn open_file(
        path: PathBuf,
        text: &str,
        data_source: Option<&Entity<DataSource>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let data_source_id = data_source
            .map(|data_source| data_source.read(cx).profile().id().clone())
            .unwrap_or_else(|| DataSourceId::from(""));
        Self::build(
            new_console_id(),
            file_name(&path),
            data_source_id,
            data_source.cloned(),
            Some(path),
            text,
            TransactionMode::default(),
            window,
            cx,
        )
    }

    /// The SQL file the console edits.
    pub fn file(&self) -> Option<&PathBuf> {
        self.file.as_ref()
    }

    /// Run the console's statements against `data_source` from now on. The
    /// session of the previous one is closed.
    pub fn set_data_source(
        &mut self,
        data_source: Entity<DataSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.run.is_some() {
            return;
        }
        if self.session.is_some() {
            self.disconnect(cx);
        }
        self.data_source_id = data_source.read(cx).profile().id().clone();
        self.data_source = Some(data_source.clone());
        self.server_error = None;
        let inspections = self.inspections.clone();
        self.editor.update(cx, |editor, _| {
            attach_intelligence(editor, &data_source, inspections)
        });
        self.data_source_subscriptions = watch_data_source(&data_source, window, cx);
        self.schedule_inspection(cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Save the console's text to a SQL file the person picks, and edit that
    /// file from then on.
    fn save_as(&mut self, _: &SaveConsoleAs, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .file
            .as_ref()
            .and_then(|file| file.parent().map(PathBuf::from))
            .or_else(dirs::document_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let suggested = format!("{}.sql", self.name.trim_end_matches(".sql"));
        let path = cx.prompt_for_new_path(&directory, Some(&suggested));
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            let Ok(text) = this.update(cx, |this, cx| this.editor.read(cx).value()) else {
                return;
            };
            let written = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::write(&path, text.as_bytes()) }
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| match written {
                Ok(()) => {
                    // The scratch text has moved into the file.
                    if this.file.is_none() {
                        let scratch = console_path(&this.id, cx);
                        cx.background_spawn(async move {
                            let _ = std::fs::remove_file(scratch);
                        })
                        .detach();
                    }
                    this.name = file_name(&path);
                    this.file = Some(path);
                    cx.emit(PanelEvent::LayoutChanged);
                    cx.notify();
                }
                Err(error) => window.push_notification(
                    Notification::error(
                        t!(
                            "console.save_failed",
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

    pub fn name(&self) -> SharedString {
        self.name.clone()
    }

    pub fn data_source(&self) -> Option<&Entity<DataSource>> {
        self.data_source.as_ref()
    }

    /// What the console's session is doing.
    pub fn session_state(&self) -> SessionState {
        match &self.run {
            Some(run) => SessionState::Running {
                started: run.started,
                statement: run.statement,
                total: run.total,
            },
            None if self.session.as_ref().is_some_and(|s| !s.is_closed()) => {
                if self.in_transaction {
                    SessionState::InTransaction
                } else {
                    SessionState::Idle
                }
            }
            None => SessionState::Disconnected,
        }
    }

    /// Close the console's session; the next run opens another. An open
    /// transaction is rolled back by the server.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.run = None;
        self.session = None;
        self.in_transaction = false;
        self.log(t!("console.disconnected").into(), false);
        cx.notify();
    }

    /// The data source of the console `state` describes, if it is one.
    pub fn saved_data_source(state: &PanelState) -> Option<DataSourceId> {
        if state.panel_name != Self::NAME {
            return None;
        }
        match &state.info {
            PanelInfo::Panel(value) => serde_json::from_value::<SavedConsole>(value.clone())
                .ok()
                .map(|saved| saved.data_source),
            _ => None,
        }
    }

    /// The console a saved layout describes, with the text it had.
    pub fn restore(state: &PanelState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let saved = match &state.info {
            PanelInfo::Panel(value) => serde_json::from_value::<SavedConsole>(value.clone()).ok(),
            _ => None,
        };
        let saved = saved.unwrap_or_else(|| SavedConsole {
            id: new_console_id().to_string(),
            name: t!("console.default_name").to_string(),
            data_source: DataSourceId::from(""),
            transaction_mode: TransactionMode::default(),
            file: None,
        });
        let path = saved
            .file
            .clone()
            .unwrap_or_else(|| console_path(&saved.id, cx));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let data_source = DataSources::global(cx).read(cx).get(&saved.data_source, cx);
        Self::build(
            saved.id.into(),
            saved.name.into(),
            saved.data_source,
            data_source,
            saved.file,
            &text,
            saved.transaction_mode,
            window,
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        id: SharedString,
        name: SharedString,
        data_source_id: DataSourceId,
        data_source: Option<Entity<DataSource>>,
        file: Option<PathBuf>,
        text: &str,
        transaction_mode: TransactionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text = SharedString::from(text.to_string());
        let inspections: Rc<RefCell<Vec<Inspection>>> = Rc::default();
        let editor = cx.new(|cx| {
            let mut state = EditorState::new(window, cx)
                .language("sql")
                .line_number(true)
                .searchable(true)
                .default_value(text);
            if let Some(data_source) = &data_source {
                attach_intelligence(&mut state, data_source, inspections.clone());
            }
            state
        });
        let subscriptions = vec![cx.subscribe_in(
            &editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.place_template_caret(window, cx);
                    this.server_error = None;
                    this.schedule_save(cx);
                    this.schedule_inspection(cx);
                }
            },
        )];
        let data_source_subscriptions = match &data_source {
            Some(data_source) => watch_data_source(data_source, window, cx),
            None => Vec::new(),
        };
        let mut console = Self {
            id,
            name,
            data_source_id,
            data_source,
            file,
            editor,
            inspections,
            session: None,
            transaction_mode,
            in_transaction: false,
            run: None,
            results: Vec::new(),
            output: Vec::new(),
            active_tab: ResultTab::Output,
            parameter_values: HashMap::new(),
            server_error: None,
            save_task: None,
            inspect_task: None,
            data_source_subscriptions,
            _subscriptions: subscriptions,
        };
        console.schedule_inspection(cx);
        super::Sessions::register(&cx.entity(), cx);
        console
    }

    /// Replace the console's text with `sql` and run every statement in it.
    pub fn run_sql(&mut self, sql: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text: Arc<str> = sql.into();
        self.editor.update(cx, |editor, cx| {
            editor.set_value(sql.to_string(), window, cx);
        });
        // Setting the value is not an edit, so nothing else saves it.
        self.schedule_save(cx);
        self.schedule_inspection(cx);
        let statements = split_statements(&text);
        self.prepare_run(text, statements, RunMode::Execute, window, cx);
    }

    /// Where the console's text is saved.
    fn text_path(&self, cx: &App) -> PathBuf {
        self.file
            .clone()
            .unwrap_or_else(|| console_path(&self.id, cx))
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let Ok((path, text)) = this.update(cx, |this, cx| {
                (this.text_path(cx), this.editor.read(cx).value())
            }) else {
                return;
            };
            let written = cx
                .background_spawn(async move {
                    if let Some(directory) = path.parent() {
                        std::fs::create_dir_all(directory)?;
                    }
                    std::fs::write(&path, text.as_bytes())
                })
                .await;
            if let Err(error) = written {
                tracing::error!("couldn’t save the console: {error}");
            }
        }));
    }

    /// Inspect the text a moment after typing stops, and show what was
    /// found as diagnostics.
    fn schedule_inspection(&mut self, cx: &mut Context<Self>) {
        let Some(data_source) = self.data_source.clone() else {
            return;
        };
        self.inspect_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let Ok((text, catalog, dialect)) = this.update(cx, |this, cx| {
                let source = data_source.read(cx);
                (
                    this.editor.read(cx).value().to_string(),
                    source.catalog().clone(),
                    source.dialect(),
                )
            }) else {
                return;
            };
            let found = cx
                .background_spawn(async move { inspect(&text, &catalog, &*dialect) })
                .await;
            let _ = this.update(cx, |this, cx| {
                *this.inspections.borrow_mut() = found;
                this.show_diagnostics(cx);
            });
        }));
    }

    /// Put the inspections and the server's last error into the editor.
    fn show_diagnostics(&mut self, cx: &mut Context<Self>) {
        let inspections = self.inspections.clone();
        let server_error = self.server_error.clone();
        self.editor.update(cx, |editor, cx| {
            let text = editor.text().clone();
            let Some(diagnostics) = editor.diagnostics_mut() else {
                return;
            };
            diagnostics.reset(&text);
            for inspection in inspections.borrow().iter() {
                let range = inspection.range();
                let start = range.start.min(text.len());
                let end = range.end.max(start + 1).min(text.len());
                diagnostics.push(
                    Diagnostic::new(
                        text.offset_to_position(start)..text.offset_to_position(end),
                        problem_message(inspection.problem()),
                    )
                    .with_severity(match inspection.severity() {
                        Severity::Error => DiagnosticSeverity::Error,
                        Severity::Warning => DiagnosticSeverity::Warning,
                    }),
                );
            }
            if let Some(error) = server_error {
                diagnostics.push(error);
            }
            // A popover may still describe a problem that is gone.
            editor.clear_diagnostic_popover(cx);
            cx.notify();
        });
    }

    /// After a live template is inserted, move the caret to where it marks.
    fn place_template_caret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.editor.read(cx).value();
        let Some(offset) = value.find(CARET) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(offset..offset + CARET.len_utf8(), cx);
            editor.replace("", window, cx);
        });
    }

    /// The statements to run: those in the selection, or the one at the
    /// caret, as byte ranges into `text`.
    fn statements_to_run(&self, cx: &App) -> (Arc<str>, Vec<Range<usize>>) {
        let editor = self.editor.read(cx);
        let text: Arc<str> = editor.value().to_string().into();
        let selection = editor.selected_range();
        let statements = if selection.is_empty() {
            statement_at(&text, editor.cursor()).into_iter().collect()
        } else {
            split_statements(&text[selection.clone()])
                .into_iter()
                .map(|range| range.start + selection.start..range.end + selection.start)
                .collect()
        };
        (text, statements)
    }

    fn execute(&mut self, _: &ExecuteStatement, window: &mut Window, cx: &mut Context<Self>) {
        let (text, statements) = self.statements_to_run(cx);
        self.prepare_run(text, statements, RunMode::Execute, window, cx);
    }

    fn explain(&mut self, _: &ExplainPlan, window: &mut Window, cx: &mut Context<Self>) {
        let (text, statements) = self.statements_to_run(cx);
        let mode = RunMode::Explain { analyze: false };
        self.prepare_run(text, statements, mode, window, cx);
    }

    fn explain_analyze(&mut self, _: &ExplainAnalyze, window: &mut Window, cx: &mut Context<Self>) {
        let (text, statements) = self.statements_to_run(cx);
        let mode = RunMode::Explain { analyze: true };
        self.prepare_run(text, statements, mode, window, cx);
    }

    fn format(&mut self, _: &FormatSql, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let selection = editor.selected_range();
        let value = editor.value().to_string();
        let style = FormatStyle::default()
            .uppercase_keywords(crate::settings::Settings::global(cx).uppercase_keywords());
        if selection.is_empty() {
            let formatted = format_sql(&value, style);
            self.editor
                .update(cx, |editor, cx| editor.replace_all(formatted, window, cx));
        } else {
            let formatted = format_sql(&value[selection], style);
            self.editor
                .update(cx, |editor, cx| editor.replace(formatted, window, cx));
        }
    }

    /// The catalog object the name at the caret refers to.
    fn object_at_caret(&self, cx: &App) -> Option<ObjectRef> {
        let data_source = self.data_source.clone()?;
        let editor = self.editor.read(cx);
        let text = editor.value().to_string();
        let source = data_source.read(cx);
        let catalog = source.catalog();
        let dialect = source.dialect();
        let mut target = resolve(&text, editor.cursor(), catalog, &*dialect)?
            .target()
            .clone();
        // An alias stands for the relation it names.
        if let Target::Declaration(range) = &target {
            target = resolve(&text, range.start, catalog, &*dialect)?
                .target()
                .clone();
        }
        let path = match target {
            Target::Schema { schema } => ObjectPath::Schema { schema },
            Target::Relation { schema, relation } => ObjectPath::relation(schema, relation),
            Target::Column {
                schema,
                relation,
                column,
            } => ObjectPath::Column {
                schema,
                relation,
                column,
            },
            Target::Routine { schema, name } => {
                let routine = catalog.schema(&schema)?.routines_named(&name).next()?;
                ObjectPath::Routine {
                    schema,
                    signature: routine.signature().into(),
                }
            }
            Target::Declaration(_) => return None,
        };
        Some(ObjectRef::new(data_source, path))
    }

    fn quick_documentation(
        &mut self,
        _: &QuickDocumentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(object) = self.object_at_caret(cx) {
            crate::explorer::show_documentation(&object, window, cx);
        }
    }

    fn rename_alias(&mut self, _: &RenameAlias, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data_source) = &self.data_source else {
            return;
        };
        let dialect = data_source.read(cx).dialect();
        let editor = self.editor.read(cx);
        let text = editor.value().to_string();
        let Some(occurrences) = datakit_sql::alias_occurrences(&text, editor.cursor(), &*dialect)
        else {
            return;
        };
        let editor = self.editor.downgrade();
        let name = occurrences.name().to_string();
        crate::prompt::prompt_text(
            t!("console.rename_title", name = name).into(),
            t!("console.rename_label").into(),
            &name,
            t!("console.rename").into(),
            move |new_name, window, cx| {
                let new_name = dialect.quote_identifier(&new_name);
                let _ = editor.update(cx, |editor, cx| {
                    let rope = editor.text().clone();
                    let edits: Vec<lsp_types::TextEdit> = occurrences
                        .ranges()
                        .iter()
                        .map(|range| lsp_types::TextEdit {
                            range: lsp_types::Range::new(
                                rope.offset_to_position(range.start),
                                rope.offset_to_position(range.end),
                            ),
                            new_text: new_name.clone(),
                        })
                        .collect();
                    editor.apply_lsp_edits(&edits, window, cx);
                });
            },
            window,
            cx,
        );
    }

    /// Ask for placeholder values when the statements have any, then run.
    fn prepare_run(
        &mut self,
        text: Arc<str>,
        statements: Vec<Range<usize>>,
        mode: RunMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.run.is_some() || statements.is_empty() {
            return;
        }
        let mut names: Vec<String> = Vec::new();
        for range in &statements {
            for parameter in parameters(&text[range.clone()]) {
                if !names.iter().any(|name| name == parameter.name()) {
                    names.push(parameter.name().to_string());
                }
            }
        }
        if names.is_empty() {
            let statements = statements
                .into_iter()
                .map(|range| (range.start, text[range].to_string()))
                .collect();
            self.run_statements(statements, mode, window, cx);
            return;
        }
        let console = cx.entity().downgrade();
        ParametersDialog::open(
            names,
            &self.parameter_values,
            move |values, window, cx| {
                let _ = console.update(cx, |console, cx| {
                    console.parameter_values.extend(values.clone());
                    // Error positions refer to the substituted text, so they
                    // are not underlined.
                    let statements = statements
                        .iter()
                        .map(|range| (DETACHED, substitute(&text[range.clone()], &values)))
                        .collect();
                    console.run_statements(statements, mode, window, cx);
                });
            },
            window,
            cx,
        );
    }

    /// Run `statements` — each with the offset of its text in the editor,
    /// or [`DETACHED`] — one after another, stopping at the first that
    /// fails.
    fn run_statements(
        &mut self,
        statements: Vec<(usize, String)>,
        mode: RunMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.run.is_some() || statements.is_empty() {
            return;
        }
        let Some(data_source) = self.data_source.clone() else {
            return;
        };
        self.server_error = None;
        self.show_diagnostics(cx);
        self.results.clear();
        self.active_tab = ResultTab::Output;

        let dialect = data_source.read(cx).dialect();
        let session = self.session.clone().filter(|session| !session.is_closed());
        let begin = (self.transaction_mode == TransactionMode::Manual
            && mode == RunMode::Execute
            && (!self.in_transaction || session.is_none()))
        .then(|| dialect.begin_transaction().to_string());
        let statements: Vec<(usize, String)> = match mode {
            RunMode::Execute => statements,
            RunMode::Explain { analyze } => {
                let (offset, sql) = statements.into_iter().next().unwrap_or_default();
                match dialect.explain(&sql, analyze) {
                    Some(explain) => vec![(offset, explain)],
                    None => {
                        self.log(t!("console.cannot_explain").into(), true);
                        cx.notify();
                        return;
                    }
                }
            }
        };
        let total = statements.len();
        let task = cx.spawn_in(window, async move |this, cx| {
            let connection = match session {
                Some(connection) => connection,
                None => {
                    let Ok(connecting) = cx.update(|_, cx| data_source.read(cx).open_session(cx))
                    else {
                        return;
                    };
                    match connecting.await {
                        Ok(connection) => {
                            let _ = this.update(cx, |this, cx| {
                                this.log(
                                    t!("console.connected", server = connection.server_version())
                                        .into(),
                                    false,
                                );
                                this.session = Some(connection.clone());
                                this.in_transaction = false;
                                cx.notify();
                            });
                            connection
                        }
                        Err(error) => {
                            let _ = this.update(cx, |this, cx| {
                                this.log(describe_error(&error), true);
                                this.run = None;
                                cx.notify();
                            });
                            return;
                        }
                    }
                }
            };

            if let Some(begin) = begin {
                let Ok(started) =
                    cx.update(|_, cx| Services::global(cx).spawn(connection.execute(begin.into())))
                else {
                    return;
                };
                let started = started.await;
                let _ = this.update(cx, |this, cx| {
                    match &started {
                        Ok(_) => {
                            this.in_transaction = true;
                            this.log(t!("console.transaction_started").into(), false);
                        }
                        Err(error) => {
                            this.log(describe_error(error), true);
                            this.run = None;
                        }
                    }
                    cx.notify();
                });
                if started.is_err() {
                    return;
                }
            }

            for (ix, (statement_start, sql)) in statements.into_iter().enumerate() {
                let sql: Arc<str> = sql.into();
                let _ = this.update(cx, |this, cx| {
                    if let Some(run) = &mut this.run {
                        run.statement = ix + 1;
                    }
                    cx.notify();
                });
                let started = Instant::now();
                let started_at = format::now_ms();
                let Ok(execution) =
                    cx.update(|_, cx| Services::global(cx).spawn(connection.execute(sql.clone())))
                else {
                    return;
                };
                let succeeded = match (mode, execution.await) {
                    (RunMode::Explain { analyze }, Ok(StatementOutcome::Rows(mut stream))) => {
                        let mut rows = Vec::new();
                        let mut failure = None;
                        while let Some(row) = stream.next().await {
                            match row {
                                Ok(row) => rows.push(row.into_vec()),
                                Err(error) => {
                                    failure = Some(error);
                                    break;
                                }
                            }
                        }
                        let elapsed = started.elapsed();
                        let dialect = dialect.clone();
                        let _ = this.update_in(cx, |this, window, cx| {
                            let plan = match failure {
                                Some(error) => Err(error),
                                None => dialect.parse_plan(&rows),
                            };
                            match plan {
                                Ok(plan) => {
                                    let view = cx.new(|cx| PlanView::new(&plan, window, cx));
                                    let title: SharedString = if analyze {
                                        t!("console.plan_analyzed").into()
                                    } else {
                                        t!("console.plan").into()
                                    };
                                    this.results.push(ResultEntry::Plan { view, title });
                                    this.active_tab = ResultTab::Result(this.results.len() - 1);
                                    this.log(
                                        t!(
                                            "console.explained",
                                            duration = format::duration(elapsed)
                                        )
                                        .into(),
                                        false,
                                    );
                                }
                                Err(error) => this.log(describe_error(&error), true),
                            }
                            cx.notify();
                        });
                        true
                    }
                    (_, Ok(StatementOutcome::Rows(stream))) => {
                        let columns = stream.columns().clone();
                        let Ok(mut pages) = cx.update(|_, cx| result_pages(stream, cx)) else {
                            return;
                        };
                        let page = Page::fetch(&mut pages).await;
                        let elapsed = started.elapsed();
                        if page.is_empty()
                            && let Some(message) = page.error().cloned()
                        {
                            let database_error = page.database_error().cloned();
                            let _ = this.update(cx, |this, cx| {
                                this.fail_statement(
                                    sql.clone(),
                                    statement_start,
                                    started_at,
                                    elapsed,
                                    message,
                                    database_error.as_ref(),
                                    cx,
                                );
                            });
                            break;
                        }
                        let fetched = page.len();
                        let failure = page.error().cloned();
                        let dialect = dialect.clone();
                        let _ = this.update_in(cx, |this, window, cx| {
                            let result = cx.new(|cx| {
                                ResultView::new(
                                    sql.clone(),
                                    columns,
                                    page,
                                    pages,
                                    elapsed,
                                    dialect,
                                    window,
                                    cx,
                                )
                            });
                            if !this
                                .results
                                .iter()
                                .any(|entry| matches!(entry, ResultEntry::Rows(_)))
                            {
                                this.active_tab = ResultTab::Result(this.results.len());
                            }
                            this.results.push(ResultEntry::Rows(result));
                            let outcome = match &failure {
                                Some(error) => {
                                    this.log(error.clone(), true);
                                    HistoryOutcome::Failed(error.to_string().into())
                                }
                                None => {
                                    this.log(
                                        t!(
                                            "console.rows_fetched",
                                            count = format::count(fetched),
                                            duration = format::duration(elapsed)
                                        )
                                        .into(),
                                        false,
                                    );
                                    HistoryOutcome::Rows(fetched as u64)
                                }
                            };
                            this.record(sql.clone(), started_at, elapsed, outcome, cx);
                            cx.notify();
                        });
                        failure.is_none()
                    }
                    (_, Ok(StatementOutcome::Command(summary))) => {
                        let elapsed = started.elapsed();
                        let _ = this.update(cx, |this, cx| {
                            this.complete_command(&sql, &summary, started_at, elapsed, cx)
                        });
                        true
                    }
                    (_, Err(error)) => {
                        let elapsed = started.elapsed();
                        let _ = this.update(cx, |this, cx| {
                            this.fail_statement(
                                sql.clone(),
                                statement_start,
                                started_at,
                                elapsed,
                                describe_error(&error),
                                error.downcast_ref::<DatabaseError>(),
                                cx,
                            );
                        });
                        false
                    }
                };
                if !succeeded {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.run = None;
                cx.notify();
            });
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.run.is_some()
                });
                if !running.unwrap_or(false) {
                    break;
                }
            }
        });
        self.run = Some(Run {
            started: Instant::now(),
            statement: 0,
            total,
            cancelling: false,
            _task: task,
            _ticker: ticker,
        });
        cx.notify();
    }

    /// Report a statement that returned no rows.
    fn complete_command(
        &mut self,
        sql: &Arc<str>,
        summary: &datakit_driver::CommandSummary,
        started_at: i64,
        elapsed: Duration,
        cx: &mut Context<Self>,
    ) {
        let tag = if summary.tag().is_empty() {
            t!("console.statement").to_string()
        } else {
            summary.tag().to_string()
        };
        match summary.tag() {
            "BEGIN" | "START" => self.in_transaction = true,
            "COMMIT" | "ROLLBACK" | "END" | "ABORT" => self.in_transaction = false,
            _ => {}
        }
        let line = match summary.rows() {
            Some(rows) => t!(
                "console.command_rows",
                command = tag,
                count = format::count(rows as usize),
                duration = format::duration(elapsed)
            ),
            None => t!(
                "console.command",
                command = tag,
                duration = format::duration(elapsed)
            ),
        };
        self.log(line.into(), false);
        self.record(
            sql.clone(),
            started_at,
            elapsed,
            HistoryOutcome::Command(summary.rows()),
            cx,
        );
        // A statement that changed the schema makes the catalog stale.
        let changed_schema = summary
            .tag()
            .split_whitespace()
            .next()
            .is_some_and(|word| matches!(word, "CREATE" | "ALTER" | "DROP" | "COMMENT"));
        if changed_schema && let Some(data_source) = &self.data_source {
            data_source.update(cx, |data_source, cx| data_source.refresh(cx));
        }
        cx.notify();
    }

    /// Commit or roll back the console's transaction.
    fn end_transaction(&mut self, commit: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(data_source) = &self.data_source else {
            return;
        };
        if !self.in_transaction {
            return;
        }
        let dialect = data_source.read(cx).dialect();
        let statement = if commit {
            dialect.commit()
        } else {
            dialect.rollback()
        };
        // Ending the transaction must not start another.
        let mode = self.transaction_mode;
        self.transaction_mode = TransactionMode::Auto;
        self.run_statements(
            vec![(DETACHED, statement.to_string())],
            RunMode::Execute,
            window,
            cx,
        );
        self.transaction_mode = mode;
    }

    fn commit(&mut self, _: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        self.end_transaction(true, window, cx);
    }

    fn rollback(&mut self, _: &Rollback, window: &mut Window, cx: &mut Context<Self>) {
        self.end_transaction(false, window, cx);
    }

    fn set_transaction_mode(
        &mut self,
        mode: TransactionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.transaction_mode == mode {
            return;
        }
        // Leaving manual mode commits what was done, as DataGrip does.
        if mode == TransactionMode::Auto && self.in_transaction {
            self.end_transaction(true, window, cx);
        }
        self.transaction_mode = mode;
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Report a statement that failed, or that the person cancelled.
    #[allow(clippy::too_many_arguments)]
    fn fail_statement(
        &mut self,
        sql: Arc<str>,
        statement_start: usize,
        started_at: i64,
        elapsed: Duration,
        message: SharedString,
        database_error: Option<&DatabaseError>,
        cx: &mut Context<Self>,
    ) {
        let cancelled = self.run.as_ref().is_some_and(|run| run.cancelling);
        let outcome = if cancelled {
            self.log(t!("console.cancelled").into(), true);
            HistoryOutcome::Cancelled
        } else {
            self.log(message.clone(), true);
            if let Some(error) = database_error
                && statement_start != DETACHED
            {
                self.mark_error(statement_start, error, cx);
            }
            HistoryOutcome::Failed(message.to_string().into())
        };
        self.record(sql, started_at, elapsed, outcome, cx);
        cx.notify();
    }

    fn cancel(&mut self, _: &CancelExecution, _: &mut Window, cx: &mut Context<Self>) {
        self.cancel_run(cx);
    }

    /// Ask the server to stop what the console is running.
    pub fn cancel_run(&mut self, cx: &mut Context<Self>) {
        let Some(run) = &mut self.run else {
            return;
        };
        if run.cancelling {
            return;
        }
        match self.session.clone() {
            Some(connection) => {
                run.cancelling = true;
                Services::global(cx)
                    .spawn(async move { connection.cancel().await })
                    .detach();
            }
            None => {
                // Still connecting: there is no statement on the server yet.
                self.run = None;
                self.log(t!("console.cancelled").into(), true);
            }
        }
        cx.notify();
    }

    fn record(
        &mut self,
        sql: Arc<str>,
        started_at: i64,
        elapsed: Duration,
        outcome: HistoryOutcome,
        cx: &mut Context<Self>,
    ) {
        let entry = HistoryEntry::new(
            self.data_source_id.clone(),
            sql,
            started_at,
            elapsed.as_millis() as u64,
            outcome,
        );
        HistoryLog::global(cx).update(cx, |log, cx| log.record(entry, cx));
    }

    fn log(&mut self, text: SharedString, failed: bool) {
        self.output.push(OutputLine {
            time_ms: format::now_ms(),
            text,
            failed,
        });
        if self.output.len() > OUTPUT_LIMIT {
            self.output.drain(..self.output.len() - OUTPUT_LIMIT);
        }
    }

    /// Underline where the server says `error` is, in the statement that
    /// starts at `statement_start`.
    fn mark_error(
        &mut self,
        statement_start: usize,
        error: &DatabaseError,
        cx: &mut Context<Self>,
    ) {
        let Some(position) = error.position() else {
            return;
        };
        let message = SharedString::from(error.message().to_string());
        let text = self.editor.read(cx).text().clone();
        let start = (statement_start + position).min(text.len());
        // Underline the word the error points at.
        let end = text
            .slice(start..text.len())
            .to_string()
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '"' || c == '.'))
            .map_or(text.len(), |len| start + len.max(1))
            .min(text.len());
        let range = text.offset_to_position(start)..text.offset_to_position(end);
        self.server_error =
            Some(Diagnostic::new(range, message).with_severity(DiagnosticSeverity::Error));
        self.show_diagnostics(cx);
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.run.is_some();
        let theme = cx.theme();
        let (status_icon, status_color) = match self
            .data_source
            .as_ref()
            .map(|data_source| data_source.read(cx).status().clone())
        {
            _ if self.session.as_ref().is_some_and(|s| !s.is_closed()) => {
                (IconName::Plug, theme.success)
            }
            Some(ConnectionStatus::Failed(_)) => (IconName::Unplug, theme.danger),
            _ => (IconName::Unplug, theme.muted_foreground),
        };
        let data_source_name: SharedString = match &self.data_source {
            Some(data_source) => data_source.read(cx).name(),
            None => t!("console.data_source_missing").into(),
        };
        let disabled = running || self.data_source.is_none();
        let console = cx.entity().downgrade();
        let mode = self.transaction_mode;
        let mode_label: SharedString = match mode {
            TransactionMode::Auto => t!("console.tx_auto").into(),
            TransactionMode::Manual => t!("console.tx_manual").into(),
        };
        h_flex()
            .flex_none()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.border)
            .child(
                Button::new("execute")
                    .ghost()
                    .small()
                    .icon(IconName::Play)
                    .tooltip_with_action(
                        t!("console.execute").to_string(),
                        &ExecuteStatement,
                        Some(CONTEXT),
                    )
                    .disabled(disabled)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.execute(&ExecuteStatement, window, cx)
                    })),
            )
            .child(
                Button::new("cancel")
                    .ghost()
                    .small()
                    .icon(IconName::CircleStop)
                    .tooltip_with_action(
                        t!("console.cancel").to_string(),
                        &CancelExecution,
                        Some(CONTEXT),
                    )
                    .disabled(!running)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.cancel(&CancelExecution, window, cx)
                        }),
                    ),
            )
            .child(
                Button::new("explain")
                    .ghost()
                    .small()
                    .icon(IconName::Workflow)
                    .tooltip(t!("console.explain_menu").to_string())
                    .disabled(disabled)
                    .dropdown_menu(move |menu, _, _| {
                        menu.menu(t!("console.explain").to_string(), Box::new(ExplainPlan))
                            .menu(
                                t!("console.explain_analyze").to_string(),
                                Box::new(ExplainAnalyze),
                            )
                    }),
            )
            .child(
                Button::new("format")
                    .ghost()
                    .small()
                    .icon(IconName::WandSparkles)
                    .tooltip_with_action(
                        t!("console.format").to_string(),
                        &FormatSql,
                        Some(CONTEXT),
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.format(&FormatSql, window, cx)),
                    ),
            )
            .child(div().w_px().h_4().mx_1().bg(theme.border))
            .child(
                Button::new("transaction-mode")
                    .ghost()
                    .small()
                    .label(mode_label)
                    .tooltip(t!("console.tx_mode").to_string())
                    .dropdown_menu(move |menu, _, _| {
                        let auto = console.clone();
                        let manual = console.clone();
                        menu.item(
                            PopupMenuItem::new(t!("console.tx_auto").to_string())
                                .checked(mode == TransactionMode::Auto)
                                .on_click(move |_, window, cx| {
                                    let _ = auto.update(cx, |this, cx| {
                                        this.set_transaction_mode(TransactionMode::Auto, window, cx)
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new(t!("console.tx_manual").to_string())
                                .checked(mode == TransactionMode::Manual)
                                .on_click(move |_, window, cx| {
                                    let _ = manual.update(cx, |this, cx| {
                                        this.set_transaction_mode(
                                            TransactionMode::Manual,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                    }),
            )
            .when(mode == TransactionMode::Manual, |toolbar| {
                toolbar
                    .child(
                        Button::new("commit")
                            .ghost()
                            .small()
                            .icon(IconName::Check)
                            .tooltip(t!("console.commit").to_string())
                            .disabled(running || !self.in_transaction)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.commit(&Commit, window, cx)),
                            ),
                    )
                    .child(
                        Button::new("rollback")
                            .ghost()
                            .small()
                            .icon(IconName::Undo2)
                            .tooltip(t!("console.rollback").to_string())
                            .disabled(running || !self.in_transaction)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.rollback(&Rollback, window, cx)
                            })),
                    )
            })
            .child(
                Button::new("data-source")
                    .ghost()
                    .small()
                    .ml_1()
                    .icon(Icon::new(status_icon).text_color(status_color))
                    .label(data_source_name)
                    .tooltip(t!("console.choose_data_source").to_string())
                    .disabled(running)
                    .dropdown_menu({
                        let console = cx.entity().downgrade();
                        let current = self.data_source_id.clone();
                        move |menu, _, cx| {
                            let items = DataSources::global(cx).read(cx).items().to_vec();
                            items.into_iter().fold(menu, |menu, data_source| {
                                let source = data_source.read(cx);
                                let checked = source.profile().id() == &current;
                                let console = console.clone();
                                menu.item(
                                    PopupMenuItem::new(source.name().to_string())
                                        .checked(checked)
                                        .on_click(move |_, window, cx| {
                                            let data_source = data_source.clone();
                                            let _ = console.update(cx, |console, cx| {
                                                console.set_data_source(data_source, window, cx)
                                            });
                                        }),
                                )
                            })
                        }
                    }),
            )
            .child(div().flex_1())
            .when_some(self.run.as_ref(), |toolbar, run| {
                let label: SharedString = if run.cancelling {
                    t!("console.cancelling").into()
                } else if run.statement == 0 {
                    t!("console.connecting").into()
                } else if run.total > 1 {
                    t!(
                        "console.running_of",
                        current = run.statement,
                        total = run.total
                    )
                    .into()
                } else {
                    t!("console.running").into()
                };
                toolbar.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(Spinner::new().xsmall())
                        .child(label)
                        .child(format::duration(run.started.elapsed())),
                )
            })
    }

    fn render_output(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        div()
            .id("console-output")
            .size_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .text_sm()
                    .font_family(theme.mono_font_family.clone())
                    .children(self.output.iter().map(|line| {
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(format::time_of_day(line.time_ms)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .when(line.failed, |text| text.text_color(theme.danger))
                                    .child(line.text.clone()),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_results(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = match self.active_tab {
            ResultTab::Output => 0,
            ResultTab::Result(ix) => ix + 1,
        };
        let tabs = std::iter::once(Tab::new().label(t!("console.output").to_string())).chain(
            self.results
                .iter()
                .map(|result| Tab::new().label(result.title(cx))),
        );
        let content = match self.active_tab {
            ResultTab::Result(ix) if ix < self.results.len() => self.results[ix].element(),
            _ => self.render_output(cx),
        };
        let view = cx.entity().downgrade();
        v_flex()
            .size_full()
            .child(
                TabBar::new("console-results")
                    .small()
                    .children(tabs)
                    .selected_index(selected)
                    .on_click(move |ix: &usize, _, cx| {
                        let ix = *ix;
                        let _ = view.update(cx, |this, cx| {
                            this.active_tab = if ix == 0 {
                                ResultTab::Output
                            } else {
                                ResultTab::Result(ix - 1)
                            };
                            cx.notify();
                        });
                    }),
            )
            .child(div().flex_1().min_h_0().child(content))
    }
}

impl Focusable for ConsolePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl BasePanel for ConsolePanel {
    fn panel_name(&self) -> &'static str {
        Self::NAME
    }

    fn dump(&self, _: &App) -> PanelState {
        let mut state = PanelState::new(Self::NAME);
        let saved = SavedConsole {
            id: self.id.to_string(),
            name: self.name.to_string(),
            data_source: self.data_source_id.clone(),
            transaction_mode: self.transaction_mode,
            file: self.file.clone(),
        };
        state.info = PanelInfo::panel(serde_json::to_value(saved).unwrap_or_default());
        state
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // A closed console keeps its text on disk unless it had none. A
        // file is the person's and stays.
        if self.file.is_none() && self.editor.read(cx).value().trim().is_empty() {
            let path = console_path(&self.id, cx);
            cx.background_spawn(async move {
                let _ = std::fs::remove_file(path);
            })
            .detach();
        }
    }
}

impl Panel for ConsolePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Icon::new(if self.file.is_some() {
                    IconName::FileCode
                } else {
                    IconName::SquareTerminal
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
            .child(self.name.clone())
            // An open transaction is easy to forget; the tab says so.
            .when(self.in_transaction, |title| {
                title.child(div().size_1p5().rounded_full().bg(cx.theme().warning))
            })
    }

    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.name.clone())
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ConsolePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let results_height = rems(18.).to_pixels(window.rem_size());
        let has_results = !self.output.is_empty() || !self.results.is_empty();
        let editor = Editor::new(&self.editor)
            .bordered(false)
            .h_full()
            .font_family(cx.theme().mono_font_family.clone())
            .text_size(cx.theme().mono_font_size);
        v_flex()
            .size_full()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::execute))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::explain))
            .on_action(cx.listener(Self::explain_analyze))
            .on_action(cx.listener(Self::format))
            .on_action(cx.listener(Self::commit))
            .on_action(cx.listener(Self::rollback))
            .on_action(cx.listener(Self::quick_documentation))
            .on_action(cx.listener(Self::rename_alias))
            .on_action(cx.listener(Self::save_as))
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().map(|body| {
                if has_results {
                    body.child(
                        v_resizable(SharedString::from(format!("console-{}", self.id)))
                            .child(resizable_panel().child(editor))
                            .child(
                                resizable_panel()
                                    .size(results_height)
                                    .child(self.render_results(cx)),
                            ),
                    )
                } else {
                    body.child(editor)
                }
            }))
    }
}

fn new_console_id() -> SharedString {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{n}", format::now_ms()).into()
}

/// Give the editor completion, documentation, navigation and fixes that
/// know `data_source`'s catalog.
fn attach_intelligence(
    editor: &mut EditorState,
    data_source: &Entity<DataSource>,
    inspections: Rc<RefCell<Vec<Inspection>>>,
) {
    let intelligence = Rc::new(SqlIntelligence::new(data_source.downgrade(), inspections));
    let lsp = editor.lsp_mut();
    lsp.completion_provider = Some(Rc::new(SqlCompletion::new(data_source.downgrade())));
    lsp.hover_provider = Some(intelligence.clone());
    lsp.definition_provider = Some(intelligence.clone());
    lsp.code_action_providers = vec![intelligence.clone()];
    lsp.show_document = Some(Rc::new(move |params, _, cx| {
        match intelligence.object_for(&params.uri) {
            Some(object) => {
                Navigation::request(NavigationEvent::Reveal(object), cx);
                true
            }
            // A declaration in the console's own text.
            None => false,
        }
    }));
}

fn watch_data_source(
    data_source: &Entity<DataSource>,
    _: &mut Window,
    cx: &mut Context<ConsolePanel>,
) -> Vec<Subscription> {
    vec![
        cx.observe(data_source, |_, _, cx| cx.notify()),
        // What the catalog knows decides what is a problem.
        cx.subscribe(data_source, |this, _, event: &DataSourceEvent, cx| {
            if matches!(event, DataSourceEvent::CatalogChanged) {
                this.schedule_inspection(cx);
            }
        }),
    ]
}

fn file_name(path: &std::path::Path) -> SharedString {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
        .into()
}

fn console_path(id: &str, cx: &App) -> PathBuf {
    Services::global(cx)
        .data_directory()
        .join("consoles")
        .join(format!("{id}.sql"))
}
