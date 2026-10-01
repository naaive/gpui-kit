//! One SQL Server session, and how a statement runs on it.
//!
//! A `tiberius` client needs `&mut` access for as long as a result is being
//! read, so each statement runs in a Tokio task of its own that holds the
//! session's lock, sends the batch and reads the response. The task answers
//! [`Connection::execute`] as soon as the first result set is described,
//! then forwards rows through a bounded channel; a reader that stops reading
//! stops the task, which stops reading from the server.
//!
//! TDS has no out-of-band cancel: the client sends an *attention* on the
//! session's own socket and reads until the server acknowledges it. So both
//! [`Connection::cancel`] and dropping a result early end with the task
//! sending an attention, after which the session is ready for the next
//! statement. A result dropped early is first read to its end for a short
//! while, because most results are already on the way in full and an
//! attention for a response that has ended would be acknowledged in a
//! message of its own. If the attention fails or is not acknowledged in
//! time, the conversation has lost its place: the session is closed and
//! [`Connection::is_closed`] says so, so the application opens a new one.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};

use anyhow::{Result, anyhow};
use datakit_catalog::{Role, Schema};
use datakit_driver::{
    BoxFuture, ColumnInfo, CommandSummary, Connection, DatabaseError, Row, RowStream,
    StatementOutcome, TypeCategory,
};
use futures::{FutureExt as _, StreamExt as _};
use tiberius::{Config, QueryItem, QueryStream};
use tokio::{
    net::TcpStream,
    sync::{Mutex, OwnedMutexGuard, mpsc, oneshot},
    time::Instant,
};
use tokio_util::{
    compat::{Compat, TokioAsyncWriteCompatExt as _},
    sync::CancellationToken,
};

use crate::{error, introspect, types};

pub(crate) type Client = tiberius::Client<Compat<TcpStream>>;

/// Rows a result may run ahead of its reader.
const ROW_BUFFER: usize = 64;

/// How long a result dropped before its end is read on, in the hope that it
/// ends, before the statement is stopped with an attention.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

/// How long the server may take to acknowledge an attention before the
/// session is given up.
const ATTENTION_TIMEOUT: Duration = Duration::from_secs(10);

/// The column of the result the driver adds to a data change to learn how
/// many rows it touched.
const ROW_COUNT_COLUMN: &str = "datakit:rows";

/// The column SQL Server names a showplan result.
const SHOWPLAN_COLUMN: &str = "Microsoft SQL Server 2005 XML Showplan";

/// What SQL Server Management Studio sets for every session; without them
/// a session runs with the old DB-Library defaults, under which filtered
/// indexes, indexed views and computed column indexes refuse changes.
const SESSION_SETTINGS: &str = "SET ANSI_NULLS ON; SET ANSI_PADDING ON; SET ANSI_WARNINGS ON; \
     SET ARITHABORT ON; SET CONCAT_NULL_YIELDS_NULL ON; SET QUOTED_IDENTIFIER ON; \
     SET NUMERIC_ROUNDABORT OFF; SET TEXTSIZE 2147483647";

const SERVER_VERSION: &str = "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), \
     CAST(SERVERPROPERTY('Edition') AS nvarchar(128))";

/// One SQL Server session.
pub(crate) struct SqlServerConnection {
    /// `None` once the session has been given up.
    client: Arc<Mutex<Option<Client>>>,
    session: Arc<Session>,
    server_version: Arc<str>,
}

/// What [`Connection::cancel`] and [`Connection::is_closed`] share with the
/// statement that is running.
#[derive(Default)]
pub(crate) struct Session {
    closed: AtomicBool,
    /// Cancels the running statement; `None` while the session is idle.
    running: std::sync::Mutex<Option<CancellationToken>>,
}

impl Session {
    fn start(&self) -> CancellationToken {
        let token = CancellationToken::new();
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = Some(token.clone());
        token
    }

    fn finish(&self) {
        *self.running.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn cancel(&self) {
        if let Some(token) = &*self.running.lock().unwrap_or_else(|e| e.into_inner()) {
            token.cancel();
        }
    }

    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

impl SqlServerConnection {
    pub(crate) async fn connect(config: Config) -> Result<Self> {
        let mut client = open(config).await?;
        client
            .simple_query(SESSION_SETTINGS)
            .await?
            .into_results()
            .await?;
        let row = client
            .simple_query(SERVER_VERSION)
            .await?
            .into_row()
            .await?;
        let server_version = match &row {
            Some(row) => {
                let version: Option<&str> = row.get(0);
                let edition: Option<&str> = row.get(1);
                match (version, edition) {
                    (Some(version), Some(edition)) => format!("SQL Server {version} {edition}"),
                    (Some(version), None) => format!("SQL Server {version}"),
                    _ => "SQL Server".to_string(),
                }
            }
            None => "SQL Server".to_string(),
        };
        Ok(Self {
            client: Arc::new(Mutex::new(Some(client))),
            session: Arc::default(),
            server_version: server_version.into(),
        })
    }
}

/// A logged-in client for `config`, following the redirection Azure SQL
/// Database answers some logins with.
async fn open(config: Config) -> tiberius::Result<Client> {
    match login(config.clone()).await {
        Err(tiberius::error::Error::Routing { host, port }) => {
            let mut config = config;
            config.host(host);
            config.port(port);
            login(config).await
        }
        result => result,
    }
}

async fn login(config: Config) -> tiberius::Result<Client> {
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    Client::connect(config, tcp.compat_write()).await
}

impl Connection for SqlServerConnection {
    fn server_version(&self) -> Arc<str> {
        self.server_version.clone()
    }

    fn is_closed(&self) -> bool {
        self.session.is_closed()
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        let client = self.client.clone();
        let session = self.session.clone();
        async move {
            let (outcome, answer) = oneshot::channel();
            tokio::spawn(run(client, session, sql, outcome));
            answer
                .await
                .unwrap_or_else(|_| Err(anyhow!("The statement stopped unexpectedly")))
        }
        .boxed()
    }

    /// Asks the task running the statement to send an attention; resolves
    /// at once. The statement's result, or its reader, then fails with
    /// "The statement was cancelled".
    fn cancel(&self) -> BoxFuture<()> {
        self.session.cancel();
        async { Ok(()) }.boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        let client = self.client.clone();
        let session = self.session.clone();
        async move {
            let mut guard = client.lock_owned().await;
            let result = introspect::schemas(open_client(&mut guard)?).await;
            settle(result, &mut guard, &session)
        }
        .boxed()
    }

    fn introspect_roles(&self) -> BoxFuture<Vec<Role>> {
        let client = self.client.clone();
        let session = self.session.clone();
        async move {
            let mut guard = client.lock_owned().await;
            let result = introspect::roles(open_client(&mut guard)?).await;
            settle(result, &mut guard, &session)
        }
        .boxed()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        let client = self.client.clone();
        let session = self.session.clone();
        async move {
            let mut guard = client.lock_owned().await;
            let result = introspect::schema(open_client(&mut guard)?, schema).await;
            settle(result, &mut guard, &session)
        }
        .boxed()
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        let client = self.client.clone();
        let session = self.session.clone();
        async move {
            let mut guard = client.lock_owned().await;
            let result = introspect::search_path(open_client(&mut guard)?).await;
            settle(result, &mut guard, &session)
        }
        .boxed()
    }
}

fn open_client(guard: &mut OwnedMutexGuard<Option<Client>>) -> Result<&mut Client> {
    guard.as_mut().ok_or_else(closed)
}

/// `result`, closing the session first when its error left it unusable.
fn settle<T>(
    result: Result<T>,
    guard: &mut OwnedMutexGuard<Option<Client>>,
    session: &Session,
) -> Result<T> {
    if let Err(error) = &result
        && error
            .downcast_ref::<tiberius::error::Error>()
            .is_some_and(error::is_fatal)
    {
        session.close();
        **guard = None;
    }
    result
}

fn closed() -> anyhow::Error {
    anyhow!("The connection is closed")
}

fn cancelled() -> anyhow::Error {
    anyhow::Error::new(DatabaseError::new("The statement was cancelled"))
}

/// How a batch ended, which decides what the session needs before it can
/// run the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The server's response was read to its end.
    Complete,
    /// The response was left unread; the server must be told to stop.
    Interrupted,
    /// The conversation with the server is lost.
    Broken,
}

impl Ending {
    fn after(error: &tiberius::error::Error) -> Self {
        if error::is_fatal(error) {
            Self::Broken
        } else {
            // A server error is reported once its response has been read
            // in full.
            Self::Complete
        }
    }
}

/// The statement as it is sent: the batch, and where in the statement the
/// batch begins.
struct Batch {
    sql: Arc<str>,
    text: String,
    /// Where the text the server runs begins in `sql`.
    start: usize,
    tag: String,
    /// Whether the batch ends with a query of the rows the statement
    /// touched.
    counted: bool,
}

impl Batch {
    fn new(sql: Arc<str>, start: usize) -> Self {
        let body = &sql[start..];
        let tag = command_tag(body);
        let counted = counts_rows(&tag);
        let text = if counted {
            // After a line break, so a trailing `--` comment cannot swallow
            // it.
            format!("{body}\n;SELECT ROWCOUNT_BIG() AS [{ROW_COUNT_COLUMN}]")
        } else {
            body.to_string()
        };
        Self {
            sql,
            text,
            start,
            tag,
            counted,
        }
    }

    fn error(&self, error: tiberius::error::Error) -> anyhow::Error {
        error::convert(error, &self.sql, self.start)
    }
}

/// A plan request: the setting that turns plans on, which SQL Server wants
/// in a batch of its own, and its opposite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanSetting {
    /// `SHOWPLAN_XML`: the estimated plan, without running the statement.
    Estimated,
    /// `STATISTICS XML`: the actual plan, after running it.
    Actual,
}

impl PlanSetting {
    pub(crate) fn prefix(self) -> &'static str {
        match self {
            Self::Estimated => "SET SHOWPLAN_XML ON;\n",
            Self::Actual => "SET STATISTICS XML ON;\n",
        }
    }

    fn on(self) -> &'static str {
        self.prefix().trim_end().trim_end_matches(';')
    }

    fn off(self) -> &'static str {
        match self {
            Self::Estimated => "SET SHOWPLAN_XML OFF",
            Self::Actual => "SET STATISTICS XML OFF",
        }
    }

    /// The setting `sql` starts with, as the dialect's `explain` writes it,
    /// with where the statement after it begins.
    fn find(sql: &str) -> Option<(Self, usize)> {
        let trimmed = sql.trim_start();
        let offset = sql.len() - trimmed.len();
        [Self::Estimated, Self::Actual]
            .into_iter()
            .find_map(|setting| {
                let prefix = setting.prefix();
                let head = trimmed.get(..prefix.len())?;
                head.eq_ignore_ascii_case(prefix)
                    .then_some((setting, offset + prefix.len()))
            })
            .filter(|(_, start)| !sql[*start..].trim().is_empty())
    }
}

/// Run `sql` on the session, answering through `outcome`.
async fn run(
    client: Arc<Mutex<Option<Client>>>,
    session: Arc<Session>,
    sql: Arc<str>,
    mut outcome: oneshot::Sender<Result<StatementOutcome>>,
) {
    // A statement sent while another runs waits for it; one whose caller
    // has gone by then never runs.
    let mut guard = tokio::select! {
        guard = client.lock_owned() => guard,
        () = outcome.closed() => return,
    };
    let Some(client) = guard.as_mut() else {
        let _ = outcome.send(Err(closed()));
        return;
    };
    let cancel = session.start();
    let plan = PlanSetting::find(&sql);
    let mut ending = match plan {
        None => run_batch(client, &Batch::new(sql, 0), &cancel, outcome).await,
        Some((setting, start)) => {
            run_plan(client, setting, &Batch::new(sql, start), &cancel, outcome).await
        }
    };
    session.finish();

    if ending == Ending::Interrupted {
        ending = match tokio::time::timeout(ATTENTION_TIMEOUT, client.cancel_query()).await {
            Ok(Ok(())) => Ending::Complete,
            Ok(Err(error)) => {
                tracing::debug!("SQL Server didn’t acknowledge a cancel: {error}");
                Ending::Broken
            }
            Err(_) => {
                tracing::debug!("SQL Server didn’t acknowledge a cancel in time");
                Ending::Broken
            }
        };
    }
    if ending == Ending::Complete
        && let Some((setting, _)) = plan
        && let Err(error) = drain(client.simple_query(setting.off()).await).await
        && error::is_fatal(&error)
    {
        ending = Ending::Broken;
    }
    if ending == Ending::Broken {
        session.close();
        *guard = None;
    }
}

/// Read every result of a batch, discarding them.
async fn drain(stream: tiberius::Result<QueryStream<'_>>) -> tiberius::Result<()> {
    let mut stream = stream?;
    while stream.next().await.transpose()?.is_some() {}
    Ok(())
}

/// What stopped a wait for the server.
enum Interruption {
    Cancelled,
    /// Whoever was to receive the result has gone.
    Abandoned,
}

/// The next item of `stream`, unless the statement is cancelled or
/// `abandoned` resolves first.
async fn next_item(
    stream: &mut QueryStream<'_>,
    cancel: &CancellationToken,
    abandoned: impl Future<Output = ()>,
) -> Result<Option<tiberius::Result<QueryItem>>, Interruption> {
    tokio::select! {
        item = stream.next() => Ok(item),
        () = cancel.cancelled() => Err(Interruption::Cancelled),
        () = abandoned => Err(Interruption::Abandoned),
    }
}

/// Send `batch` and wait for its first result set or its end.
async fn send<'a>(
    client: &'a mut Client,
    batch: &Batch,
    cancel: &CancellationToken,
    outcome: &mut oneshot::Sender<Result<StatementOutcome>>,
) -> Result<tiberius::Result<QueryStream<'a>>, Interruption> {
    tokio::select! {
        stream = client.simple_query(batch.text.as_str()) => Ok(stream),
        () = cancel.cancelled() => Err(Interruption::Cancelled),
        () = outcome.closed() => Err(Interruption::Abandoned),
    }
}

/// Run an ordinary batch: the first result set streams as rows, a batch
/// without one is a command.
async fn run_batch(
    client: &mut Client,
    batch: &Batch,
    cancel: &CancellationToken,
    mut outcome: oneshot::Sender<Result<StatementOutcome>>,
) -> Ending {
    let mut stream = match send(client, batch, cancel, &mut outcome).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            let ending = Ending::after(&error);
            let _ = outcome.send(Err(batch.error(error)));
            return ending;
        }
        Err(interruption) => return interrupted(interruption, outcome),
    };
    let first = match next_item(&mut stream, cancel, outcome.closed()).await {
        Ok(item) => item,
        Err(interruption) => return interrupted(interruption, outcome),
    };
    let columns = match first {
        None => {
            let summary = CommandSummary::new(batch.tag.clone(), None);
            let _ = outcome.send(Ok(StatementOutcome::Command(summary)));
            return Ending::Complete;
        }
        Some(Err(error)) => {
            let ending = Ending::after(&error);
            let _ = outcome.send(Err(batch.error(error)));
            return ending;
        }
        Some(Ok(QueryItem::Metadata(metadata))) => metadata.columns().to_vec(),
        Some(Ok(QueryItem::Row(_))) => {
            let _ = outcome.send(Err(anyhow!("SQL Server sent a row before its columns")));
            return Ending::Broken;
        }
    };

    if batch.counted && columns.len() == 1 && columns[0].name() == ROW_COUNT_COLUMN {
        return count_rows(stream, batch, cancel, outcome).await;
    }

    let columns: Vec<ColumnInfo> = columns
        .iter()
        .map(|column| {
            ColumnInfo::new(
                column.name(),
                types::display_name(column.column_type()),
                types::category(column.column_type()),
            )
        })
        .collect();
    let (rows, receiver) = mpsc::channel(ROW_BUFFER);
    let interrupted = Arc::new(AtomicBool::new(false));
    let row_stream = row_stream(receiver, interrupted.clone());
    if outcome
        .send(Ok(StatementOutcome::Rows(RowStream::new(
            columns, row_stream,
        ))))
        .is_err()
    {
        return abandon(stream, cancel).await;
    }
    forward_rows(stream, batch, cancel, Rows { rows, interrupted }).await
}

/// Where the rows of a result go, and how a cancelled result says so.
struct Rows {
    rows: mpsc::Sender<Result<Row>>,
    /// Set when the statement was cancelled; the reader learns it once it
    /// has read every row sent before, even with the channel full.
    interrupted: Arc<AtomicBool>,
}

/// The reader's side of a result: the rows, then "The statement was
/// cancelled" when it was.
fn row_stream(
    mut receiver: mpsc::Receiver<Result<Row>>,
    interrupted: Arc<AtomicBool>,
) -> futures::stream::BoxStream<'static, Result<Row>> {
    let mut reported = false;
    futures::stream::poll_fn(move |cx| match receiver.poll_recv(cx) {
        Poll::Ready(None) if !reported && interrupted.load(Ordering::SeqCst) => {
            reported = true;
            Poll::Ready(Some(Err(cancelled())))
        }
        poll => poll,
    })
    .boxed()
}

/// Answer `outcome` after `interruption`, before anything was sent to it.
fn interrupted(
    interruption: Interruption,
    outcome: oneshot::Sender<Result<StatementOutcome>>,
) -> Ending {
    if let Interruption::Cancelled = interruption {
        let _ = outcome.send(Err(cancelled()));
    }
    Ending::Interrupted
}

/// The rows of the first result set, through `rows`. Later result sets of
/// the batch are read and discarded, so the batch runs to its end and an
/// error in a later statement still reaches the reader.
async fn forward_rows(
    mut stream: QueryStream<'_>,
    batch: &Batch,
    cancel: &CancellationToken,
    rows: Rows,
) -> Ending {
    let Rows { rows, interrupted } = &rows;
    let cancelled = || {
        interrupted.store(true, Ordering::SeqCst);
        Ending::Interrupted
    };
    let mut first_result = true;
    loop {
        let item = match next_item(&mut stream, cancel, rows.closed()).await {
            Ok(item) => item,
            Err(Interruption::Cancelled) => return cancelled(),
            Err(Interruption::Abandoned) => return abandon(stream, cancel).await,
        };
        match item {
            None => return Ending::Complete,
            Some(Err(error)) => {
                let ending = Ending::after(&error);
                let _ = rows.send(Err(batch.error(error))).await;
                return ending;
            }
            Some(Ok(QueryItem::Metadata(_))) => first_result = false,
            Some(Ok(QueryItem::Row(row))) if first_result => {
                let values = types::row_values(&row);
                tokio::select! {
                    sent = rows.send(Ok(values)) => {
                        if sent.is_err() {
                            return abandon(stream, cancel).await;
                        }
                    }
                    () = cancel.cancelled() => return cancelled(),
                }
            }
            Some(Ok(QueryItem::Row(_))) => {}
        }
    }
}

/// The rest of a response nobody will read: read on for a moment, since
/// most results are already on the way in full, and stop the statement if
/// it goes on.
async fn abandon(mut stream: QueryStream<'_>, cancel: &CancellationToken) -> Ending {
    let deadline = Instant::now() + DRAIN_GRACE;
    loop {
        let item = tokio::select! {
            item = stream.next() => item,
            () = tokio::time::sleep_until(deadline) => return Ending::Interrupted,
            () = cancel.cancelled() => return Ending::Interrupted,
        };
        match item {
            None => return Ending::Complete,
            Some(Err(error)) => return Ending::after(&error),
            Some(Ok(_)) => {}
        }
    }
}

/// The command of a counted batch: the driver's row-count query is its
/// only result.
async fn count_rows(
    mut stream: QueryStream<'_>,
    batch: &Batch,
    cancel: &CancellationToken,
    mut outcome: oneshot::Sender<Result<StatementOutcome>>,
) -> Ending {
    let mut count = None;
    loop {
        let item = match next_item(&mut stream, cancel, outcome.closed()).await {
            Ok(item) => item,
            Err(interruption) => return interrupted(interruption, outcome),
        };
        match item {
            None => break,
            Some(Err(error)) => {
                let ending = Ending::after(&error);
                let _ = outcome.send(Err(batch.error(error)));
                return ending;
            }
            Some(Ok(QueryItem::Row(row))) if count.is_none() => count = row.get::<i64, _>(0),
            Some(Ok(_)) => {}
        }
    }
    let rows = count.and_then(|count| u64::try_from(count).ok());
    let summary = CommandSummary::new(batch.tag.clone(), rows);
    let _ = outcome.send(Ok(StatementOutcome::Command(summary)));
    Ending::Complete
}

/// Run a plan request: turn the setting on in a batch of its own, run the
/// statement, and answer with the showplan documents it produced, one row
/// each. The caller turns the setting off again.
async fn run_plan(
    client: &mut Client,
    setting: PlanSetting,
    batch: &Batch,
    cancel: &CancellationToken,
    mut outcome: oneshot::Sender<Result<StatementOutcome>>,
) -> Ending {
    if let Err(error) = drain(client.simple_query(setting.on()).await).await {
        let ending = Ending::after(&error);
        let _ = outcome.send(Err(anyhow::Error::new(error)));
        return ending;
    }
    let mut stream = match send(client, batch, cancel, &mut outcome).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            let ending = Ending::after(&error);
            let _ = outcome.send(Err(batch.error(error)));
            return ending;
        }
        Err(interruption) => return interrupted(interruption, outcome),
    };
    let mut plans: Vec<Result<Row>> = Vec::new();
    let mut in_plan = false;
    loop {
        let item = match next_item(&mut stream, cancel, outcome.closed()).await {
            Ok(item) => item,
            Err(interruption) => return interrupted(interruption, outcome),
        };
        match item {
            None => break,
            Some(Err(error)) => {
                let ending = Ending::after(&error);
                let _ = outcome.send(Err(batch.error(error)));
                return ending;
            }
            Some(Ok(QueryItem::Metadata(metadata))) => {
                let columns = metadata.columns();
                in_plan = columns.len() == 1 && columns[0].name() == SHOWPLAN_COLUMN;
            }
            Some(Ok(QueryItem::Row(row))) if in_plan => plans.push(Ok(types::row_values(&row))),
            Some(Ok(QueryItem::Row(_))) => {}
        }
    }
    let columns = [ColumnInfo::new("plan", "xml", TypeCategory::Other)];
    let rows = RowStream::new(columns, futures::stream::iter(plans).boxed());
    let _ = outcome.send(Ok(StatementOutcome::Rows(rows)));
    Ending::Complete
}

/// The command a statement runs, named the way the PostgreSQL driver
/// names it: `INSERT`, `CREATE TABLE`, `BEGIN TRANSACTION`.
pub(crate) fn command_tag(sql: &str) -> String {
    let words: Vec<String> = leading_words(sql).take(6).collect();
    let Some(first) = words.first() else {
        return String::new();
    };
    let first = match first.as_str() {
        "EXECUTE" => "EXEC",
        word => word,
    };
    match first {
        "CREATE" | "DROP" | "ALTER" => {
            // `CREATE OR ALTER VIEW` and `CREATE UNIQUE CLUSTERED INDEX`
            // name the object after the modifiers.
            let object = words.iter().skip(1).find(|word| {
                !matches!(
                    word.as_str(),
                    "OR" | "ALTER" | "UNIQUE" | "CLUSTERED" | "NONCLUSTERED" | "COLUMNSTORE"
                )
            });
            match object.map(String::as_str) {
                Some("PROC") => format!("{first} PROCEDURE"),
                Some(object) => format!("{first} {object}"),
                None => first.to_string(),
            }
        }
        "BEGIN" if words.get(1).is_some_and(|word| word.starts_with("TRAN")) => {
            "BEGIN TRANSACTION".into()
        }
        _ => first.to_string(),
    }
}

/// The leading words of `sql`, upper-cased, skipping comments.
fn leading_words(sql: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = sql;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("--") {
                rest = after.split_once('\n').map_or("", |(_, tail)| tail);
            } else if let Some(after) = rest.strip_prefix("/*") {
                rest = after.split_once("*/").map_or("", |(_, tail)| tail);
            } else {
                break;
            }
        }
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let word = rest[..end].to_uppercase();
        rest = &rest[end..];
        Some(word)
    })
}

/// Whether the command changes rows, so that how many is worth asking.
fn counts_rows(tag: &str) -> bool {
    matches!(tag, "INSERT" | "UPDATE" | "DELETE" | "MERGE")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tags_name_the_command_and_its_object() {
        assert_eq!(command_tag("insert into t values (1)"), "INSERT");
        assert_eq!(
            command_tag("-- note\n/* x */ create or alter view v as select 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("CREATE UNIQUE NONCLUSTERED INDEX i ON t (a)"),
            "CREATE INDEX"
        );
        assert_eq!(command_tag("create proc p as select 1"), "CREATE PROCEDURE");
        assert_eq!(command_tag("begin tran"), "BEGIN TRANSACTION");
        assert_eq!(command_tag("execute sp_who"), "EXEC");
        assert_eq!(command_tag("  "), "");
    }

    #[test]
    fn only_data_changes_are_counted() {
        let batch = Batch::new("UPDATE t SET a = 1 -- all of them".into(), 0);
        assert!(batch.counted);
        assert!(
            batch
                .text
                .ends_with("-- all of them\n;SELECT ROWCOUNT_BIG() AS [datakit:rows]")
        );
        let batch = Batch::new("CREATE TABLE t (a int)".into(), 0);
        assert!(!batch.counted);
        assert_eq!(batch.text, "CREATE TABLE t (a int)");
    }

    #[test]
    fn a_cancelled_result_says_so_after_its_rows() {
        let (rows, receiver) = mpsc::channel(1);
        let interrupted = Arc::new(AtomicBool::new(false));
        let stream = row_stream(receiver, interrupted.clone());
        rows.try_send(Ok(vec![datakit_driver::Value::Int(1)].into()))
            .unwrap();
        // The buffer is full; the cancellation still reaches the reader.
        interrupted.store(true, Ordering::SeqCst);
        drop(rows);
        let items: Vec<Result<Row>> = futures::executor::block_on(stream.collect());
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok());
        let error = items[1].as_ref().unwrap_err();
        assert_eq!(
            error.downcast_ref::<DatabaseError>().unwrap().message(),
            "The statement was cancelled"
        );
    }

    #[test]
    fn a_plan_request_is_found_at_the_start() {
        let sql = "SET SHOWPLAN_XML ON;\nSELECT 1";
        let (setting, start) = PlanSetting::find(sql).unwrap();
        assert_eq!(setting, PlanSetting::Estimated);
        assert_eq!(&sql[start..], "SELECT 1");
        assert_eq!(setting.on(), "SET SHOWPLAN_XML ON");
        assert!(PlanSetting::find("set statistics xml on;\nSELECT 1").is_some());
        // The setting alone is an ordinary statement.
        assert!(PlanSetting::find("SET SHOWPLAN_XML ON;\n").is_none());
        assert!(PlanSetting::find("SELECT 1").is_none());
    }
}
