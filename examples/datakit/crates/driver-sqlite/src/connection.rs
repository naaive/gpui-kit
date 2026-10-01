use std::{
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
    task::{Context, Poll},
    time::Duration,
};

use anyhow::{Result, anyhow};
use datakit_catalog::Schema;
use datakit_driver::{
    BoxFuture, ColumnInfo, CommandSummary, Connection, Row, RowStream, StatementOutcome,
    TypeCategory,
};
use futures::{
    FutureExt as _, SinkExt as _, Stream, StreamExt as _,
    channel::{mpsc, oneshot},
};
use rusqlite::{InterruptHandle, fallible_iterator::FallibleIterator as _};

use crate::{error::convert, introspect, parse, types};

/// How many rows the worker reads ahead of the reader before it waits.
const ROW_BUFFER: usize = 256;

/// How long a statement waits for another connection's lock on the file
/// before it fails with `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Work for the connection's thread, which owns the database handle.
type Job = Box<dyn FnOnce(&rusqlite::Connection) + Send>;

/// One SQLite session: a thread that owns the database handle and runs one
/// job at a time, in the order they were sent.
pub(crate) struct SqliteConnection {
    jobs: std_mpsc::Sender<Job>,
    shared: Arc<Shared>,
    next_statement: AtomicU64,
    server_version: Arc<str>,
}

/// What the connection and its thread both read.
struct Shared {
    /// The `execute` call whose statement the thread is running, by its id.
    /// Interrupting on behalf of a call checks this under the lock, so an
    /// abandoned call never interrupts the statement that runs after it.
    running: Mutex<Option<u64>>,
    interrupt: InterruptHandle,
    closed: AtomicBool,
}

impl Shared {
    fn set_running(&self, statement: Option<u64>) {
        *self
            .running
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = statement;
    }

    /// Interrupt the statement of the `execute` call `statement`, if it is
    /// still the one running.
    fn interrupt_statement(&self, statement: u64) {
        let running = self
            .running
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if *running == Some(statement) {
            self.interrupt.interrupt();
        }
    }
}

impl SqliteConnection {
    /// Open `file` on a thread of its own; `:memory:` is a private
    /// in-memory database.
    pub(crate) async fn open(file: String) -> Result<Self> {
        let (jobs, queue) = std_mpsc::channel::<Job>();
        let (opened, opening) = oneshot::channel::<Result<Arc<Shared>>>();
        std::thread::Builder::new()
            .name("datakit-sqlite".into())
            .spawn(move || {
                let connection = match open(&file) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                let shared = Arc::new(Shared {
                    running: Mutex::new(None),
                    interrupt: connection.get_interrupt_handle(),
                    closed: AtomicBool::new(false),
                });
                if opened.send(Ok(shared.clone())).is_err() {
                    return;
                }
                while let Ok(job) = queue.recv() {
                    // A job that panics loses only its own result: its reply
                    // channel drops, and the session goes on.
                    if std::panic::catch_unwind(AssertUnwindSafe(|| job(&connection))).is_err() {
                        tracing::error!("a SQLite job panicked");
                        shared.set_running(None);
                    }
                }
                shared.closed.store(true, Ordering::SeqCst);
            })?;
        let shared = opening.await.map_err(|_| stopped())??;
        Ok(Self {
            jobs,
            shared,
            next_statement: AtomicU64::new(0),
            server_version: format!("SQLite {}", rusqlite::version()).into(),
        })
    }

    /// Run `work` on the connection's thread, after the jobs sent before it.
    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
    ) -> BoxFuture<T> {
        let jobs = self.jobs.clone();
        async move {
            let (reply, result) = oneshot::channel();
            jobs.send(Box::new(move |connection| {
                if !reply.is_canceled() {
                    let _ = reply.send(work(connection));
                }
            }))
            .map_err(|_| stopped())?;
            result.await.map_err(|_| stopped())?
        }
        .boxed()
    }
}

fn open(file: &str) -> Result<rusqlite::Connection> {
    let connection = if file == ":memory:" {
        rusqlite::Connection::open_in_memory()
    } else {
        rusqlite::Connection::open(file)
    }
    .map_err(|error| convert(error, ""))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .and_then(|_| connection.pragma_update(None, "foreign_keys", true))
        // SQLite reads the file lazily; reading the schema now makes a file
        // that is not a database fail here rather than at the first query.
        .and_then(|_| connection.query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(())))
        .map_err(|error| convert(error, ""))?;
    Ok(connection)
}

fn stopped() -> anyhow::Error {
    anyhow!("The SQLite connection has closed")
}

impl Connection for SqliteConnection {
    fn server_version(&self) -> Arc<str> {
        self.server_version.clone()
    }

    fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        let statement = self.next_statement.fetch_add(1, Ordering::Relaxed);
        let shared = self.shared.clone();
        let jobs = self.jobs.clone();
        async move {
            // Dropping this future before the statement has described its
            // result interrupts the statement; once rows stream, the guard
            // moves into the stream and does the same when it is dropped.
            let guard = StatementGuard {
                statement,
                shared: shared.clone(),
            };
            let (reply, described) = oneshot::channel();
            jobs.send(Box::new(move |connection| {
                if reply.is_canceled() {
                    return;
                }
                shared.set_running(Some(statement));
                run_statements(connection, &sql, reply);
                shared.set_running(None);
            }))
            .map_err(|_| stopped())?;
            match described.await.map_err(|_| stopped())?? {
                Described::Command(summary) => Ok(StatementOutcome::Command(summary)),
                Described::Rows { columns, rows } => Ok(StatementOutcome::Rows(RowStream::new(
                    columns,
                    Rows {
                        rows,
                        _guard: guard,
                    }
                    .boxed(),
                ))),
            }
        }
        .boxed()
    }

    fn cancel(&self) -> BoxFuture<()> {
        // SQLite ignores an interrupt when no statement is running.
        self.shared.interrupt.interrupt();
        futures::future::ready(Ok(())).boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        self.run(introspect::schemas)
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        self.run(move |connection| introspect::schema(connection, &schema))
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        futures::future::ready(Ok(vec!["main".into()])).boxed()
    }
}

/// What a statement produces, as the connection's thread first reports it.
enum Described {
    Command(CommandSummary),
    Rows {
        columns: Vec<ColumnInfo>,
        rows: mpsc::Receiver<Result<Row>>,
    },
}

/// Interrupts the statement of one `execute` call when dropped, unless that
/// statement has already finished.
struct StatementGuard {
    statement: u64,
    shared: Arc<Shared>,
}

impl Drop for StatementGuard {
    fn drop(&mut self) {
        self.shared.interrupt_statement(self.statement);
    }
}

/// The rows of a result as the connection's thread reads them.
///
/// Dropped before the end, the thread's next send fails and it stops
/// stepping the statement; the guard also interrupts a step that is still
/// computing the next row, so the session is free again promptly.
struct Rows {
    rows: mpsc::Receiver<Result<Row>>,
    _guard: StatementGuard,
}

impl Stream for Rows {
    type Item = Result<Row>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rows.poll_next_unpin(cx)
    }
}

/// Run the statements of `sql` in order, on the connection's thread.
///
/// The text is normally one statement. When it holds several, each runs in
/// turn and the last one's summary is reported, until one returns rows: that
/// one becomes the result, and the statements after it do not run.
fn run_statements(
    connection: &rusqlite::Connection,
    sql: &str,
    reply: oneshot::Sender<Result<Described>>,
) {
    let mut batch = rusqlite::Batch::new(connection, sql);
    let mut summary = CommandSummary::new("", None);
    loop {
        let mut statement = match batch.next() {
            Ok(Some(statement)) => statement,
            Ok(None) => break,
            Err(error) => {
                let _ = reply.send(Err(convert(error, sql)));
                return;
            }
        };
        if statement.column_count() > 0 {
            stream_rows(&mut statement, sql, reply);
            return;
        }
        let text = statement.expanded_sql().unwrap_or_else(|| sql.to_string());
        if let Err(error) = statement.raw_execute() {
            let _ = reply.send(Err(convert(error, sql)));
            return;
        }
        let tag = command_tag(&text);
        let rows = counts_rows(&tag).then(|| connection.changes());
        summary = CommandSummary::new(tag, rows);
    }
    let _ = reply.send(Ok(Described::Command(summary)));
}

/// Describe the result of `statement`, then send its rows until they end or
/// nobody reads them.
///
/// The description waits for the first row: a column that is an expression
/// has no declared type, and the first value's storage class stands in.
fn stream_rows(
    statement: &mut rusqlite::Statement<'_>,
    sql: &str,
    reply: oneshot::Sender<Result<Described>>,
) {
    let names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(String::from)
        .collect();
    let declared: Vec<Option<String>> = statement
        .columns()
        .iter()
        .map(|column| {
            column
                .decl_type()
                .filter(|declared| !declared.trim().is_empty())
                .map(String::from)
        })
        .collect();

    let mut rows = statement.raw_query();
    let (columns, categories, first) = match rows.next() {
        Ok(first) => {
            let mut columns = Vec::with_capacity(names.len());
            let mut categories = Vec::with_capacity(names.len());
            for (ix, name) in names.iter().enumerate() {
                let (type_name, category) = match (&declared[ix], first) {
                    (Some(declared), _) => (declared.clone(), types::category(declared)),
                    (None, Some(row)) => {
                        let value = row.get_ref_unwrap(ix);
                        (
                            types::storage_name(value).to_string(),
                            types::storage_category(value),
                        )
                    }
                    (None, None) => (String::new(), TypeCategory::Text),
                };
                columns.push(ColumnInfo::new(name.as_str(), type_name, category));
                categories.push(category);
            }
            let first = first.map(|row| read_row(row, &categories));
            (columns, categories, first)
        }
        Err(error) => {
            let _ = reply.send(Err(convert(error, sql)));
            return;
        }
    };

    let (mut sender, receiver) = mpsc::channel(ROW_BUFFER);
    if let Some(first) = first {
        // The channel is empty, so this cannot be full.
        let _ = sender.try_send(Ok(first));
    }
    if reply
        .send(Ok(Described::Rows {
            columns,
            rows: receiver,
        }))
        .is_err()
    {
        return;
    }
    loop {
        let item = match rows.next() {
            Ok(Some(row)) => Ok(read_row(row, &categories)),
            Ok(None) => return,
            Err(error) => Err(convert(error, sql)),
        };
        let failed = item.is_err();
        if !deliver(&mut sender, item) || failed {
            return;
        }
    }
}

fn read_row(row: &rusqlite::Row<'_>, categories: &[TypeCategory]) -> Row {
    categories
        .iter()
        .enumerate()
        .map(|(ix, category)| types::value(row.get_ref_unwrap(ix), *category))
        .collect()
}

/// Send `item` to the reader, waiting while the buffer is full. `false`
/// when the reader has gone.
fn deliver(sender: &mut mpsc::Sender<Result<Row>>, item: Result<Row>) -> bool {
    match sender.try_send(item) {
        Ok(()) => true,
        Err(error) if error.is_full() => {
            futures::executor::block_on(sender.send(error.into_inner())).is_ok()
        }
        Err(_) => false,
    }
}

/// The command a statement runs, named as PostgreSQL names its command tags
/// so the console reads the same for every database: `INSERT`,
/// `CREATE TABLE`, `PRAGMA`.
fn command_tag(sql: &str) -> String {
    let words = parse::top_level_words(sql);
    let Some(first) = words.first() else {
        return String::new();
    };
    match first.as_str() {
        "CREATE" | "DROP" | "ALTER" => {
            // `CREATE UNIQUE INDEX` and `CREATE VIRTUAL TABLE` name the
            // object after the modifier.
            let object = words
                .iter()
                .skip(1)
                .find(|word| !matches!(word.as_str(), "UNIQUE" | "TEMP" | "TEMPORARY" | "VIRTUAL"));
            match object {
                Some(object) => format!("{first} {object}"),
                None => first.clone(),
            }
        }
        // A common table expression introduces the statement that follows
        // it; one that returns no rows changes them.
        "WITH" => words
            .iter()
            .find(|word| matches!(word.as_str(), "INSERT" | "REPLACE" | "UPDATE" | "DELETE"))
            .cloned()
            .unwrap_or_else(|| first.clone()),
        _ => first.clone(),
    }
}

/// Whether the command's row count means something.
fn counts_rows(tag: &str) -> bool {
    matches!(tag, "INSERT" | "REPLACE" | "UPDATE" | "DELETE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tags_name_the_command_and_its_object() {
        assert_eq!(command_tag("insert or replace into t values (1)"), "INSERT");
        assert_eq!(
            command_tag("-- note\n/* x */ create temp view v as select 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("CREATE UNIQUE INDEX i ON t (a)"),
            "CREATE INDEX"
        );
        assert_eq!(
            command_tag("CREATE VIRTUAL TABLE f USING fts5(body)"),
            "CREATE TABLE"
        );
        assert_eq!(
            command_tag("WITH old AS (SELECT 1 AS id) DELETE FROM t WHERE id IN old"),
            "DELETE"
        );
        assert_eq!(command_tag("pragma foreign_keys = on"), "PRAGMA");
        assert_eq!(command_tag("  "), "");
    }

    #[test]
    fn only_data_commands_count_rows() {
        assert!(counts_rows("DELETE"));
        assert!(counts_rows("REPLACE"));
        assert!(!counts_rows("CREATE TABLE"));
    }
}
