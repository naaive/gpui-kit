use std::{
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll},
    time::Duration,
};

use anyhow::Result;
use datakit_catalog::{Role, Schema};
use datakit_driver::{
    BoxFuture, ColumnInfo, CommandSummary, Connection, DatabaseError, Row, RowStream,
    StatementOutcome, TypeCategory, Value,
};
use futures::{FutureExt as _, Stream, StreamExt as _, stream::BoxStream};
use tokio::{runtime::Handle, sync::OwnedMutexGuard};

use crate::{
    error,
    http::{self, Http, ResultSet},
    introspect, tsv, types,
};

/// How long a session lives between statements, in seconds. ClickHouse's
/// default is a minute, after which `SET` and temporary tables are gone; an
/// hour is the most the default server configuration allows.
const SESSION_TIMEOUT: &str = "3600";

/// How long an abandoned statement may take to stop before the session is
/// used again anyway.
const KILL_WAIT: Duration = Duration::from_secs(10);

/// The code ClickHouse answers a request with while another request still
/// holds the session (`SESSION_IS_LOCKED`).
const SESSION_IS_LOCKED: &str = "373";

/// Formats the result of a statement that names none is read in. A
/// statement that names another format gets that format's text.
const OWN_FORMATS: &[&str] = &[tsv::FORMAT, "TSVWithNamesAndTypes"];

/// One ClickHouse session: an HTTP endpoint and a `session_id` that keeps
/// `SET`, `USE` and temporary tables from one statement to the next.
pub(crate) struct ClickHouseConnection {
    session: Arc<Session>,
    server_version: Arc<str>,
    /// The database unqualified names are looked up in when the session
    /// started; SQL functions, which belong to no database, are listed in
    /// it.
    home: Arc<str>,
}

impl ClickHouseConnection {
    /// Start a session. Connecting is one successful round trip.
    pub(crate) async fn connect(http: Http) -> Result<Self> {
        const PROBE: &str = "SELECT version(), currentDatabase()";
        let mut session = Session::new(http, Some(SESSION_TIMEOUT));
        let probe = match session.query(PROBE).await {
            // A server configured with a shorter `max_session_timeout`
            // refuses the longer one; it then keeps its own default.
            Err(error)
                if error
                    .downcast_ref::<DatabaseError>()
                    .is_some_and(|error| error.message().contains("session_timeout")) =>
            {
                session = Session::new(session.http.clone(), None);
                session.query(PROBE).await?
            }
            probe => probe?,
        };
        let record = probe.records().next();
        let version = record.and_then(|record| record.get("version()"));
        let home = record.and_then(|record| record.get("currentDatabase()"));
        Ok(Self {
            session: Arc::new(session),
            server_version: format!("ClickHouse {}", version.unwrap_or("?")).into(),
            home: home.unwrap_or("default").into(),
        })
    }
}

impl Connection for ClickHouseConnection {
    fn server_version(&self) -> Arc<str> {
        self.server_version.clone()
    }

    /// Every statement is a request of its own, so there is no connection to
    /// lose: a server that went away is reported by the next statement, and
    /// a session that expired starts again, empty.
    fn is_closed(&self) -> bool {
        false
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        self.session.clone().execute(sql).boxed()
    }

    fn cancel(&self) -> BoxFuture<()> {
        let query_id = self
            .session
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let http = self.session.http.clone();
        async move {
            if let Some(query_id) = query_id {
                http.kill(&query_id, false).await?;
            }
            Ok(())
        }
        .boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        introspect::schemas(self.session.http.clone()).boxed()
    }

    fn introspect_roles(&self) -> BoxFuture<Vec<Role>> {
        introspect::roles(self.session.http.clone()).boxed()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        let routines = schema == self.home;
        introspect::schema(self.session.http.clone(), schema, routines).boxed()
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        let session = self.session.clone();
        async move {
            let database = session.query("SELECT currentDatabase()").await?;
            Ok(database.first_value().map(Arc::from).into_iter().collect())
        }
        .boxed()
    }
}

struct Session {
    http: Http,
    id: Arc<str>,
    timeout: Option<&'static str>,
    /// Held by the statement running in the session, from its request to its
    /// last row: ClickHouse refuses a request to a session that is busy.
    turn: Arc<tokio::sync::Mutex<()>>,
    /// The `query_id` of that statement, for [`Connection::cancel`].
    running: Arc<Mutex<Option<Arc<str>>>>,
    /// The runtime the session started on, which stops an abandoned
    /// statement wherever its rows are dropped.
    runtime: Option<Handle>,
}

impl Session {
    fn new(http: Http, timeout: Option<&'static str>) -> Self {
        Self {
            http,
            id: uuid::Uuid::new_v4().to_string().into(),
            timeout,
            turn: Arc::default(),
            running: Arc::default(),
            runtime: Handle::try_current().ok(),
        }
    }

    /// Send `sql` in this session. The caller holds the turn.
    ///
    /// The server releases a session a moment after it has sent the last
    /// byte of a result, so a request that follows at once may find it still
    /// held; it is retried briefly.
    async fn send(&self, sql: &str, parameters: &[(&str, &str)]) -> Result<reqwest::Response> {
        let mut all = vec![("session_id", &*self.id)];
        if let Some(timeout) = self.timeout {
            all.push(("session_timeout", timeout));
        }
        all.extend_from_slice(parameters);
        let mut attempt = 0;
        loop {
            match self.http.send(sql, &all).await {
                Err(error)
                    if attempt < 5
                        && error
                            .downcast_ref::<DatabaseError>()
                            .is_some_and(|error| error.code() == Some(SESSION_IS_LOCKED)) =>
                {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(50 * attempt)).await;
                }
                result => return result,
            }
        }
    }

    /// Run one of the driver's own statements in this session and read its
    /// whole result.
    async fn query(&self, sql: &str) -> Result<ResultSet> {
        let _turn = self.turn.lock().await;
        let mut parameters = vec![("default_format", tsv::FORMAT)];
        let query_id = uuid::Uuid::new_v4().to_string();
        parameters.push(("query_id", &query_id));
        let response = self.send(sql, &parameters).await?;
        http::ResultSet::from_response(response, sql).await
    }

    async fn execute(self: Arc<Self>, sql: Arc<str>) -> Result<StatementOutcome> {
        // ClickHouse refuses an empty statement; a comment alone does
        // nothing, as it does elsewhere.
        if is_blank(&sql) {
            return Ok(StatementOutcome::Command(CommandSummary::new("", None)));
        }
        let tag = command_tag(&sql);
        let turn = self.turn.clone().lock_owned().await;
        let statement = Running::start(&self, turn);
        let query_id = statement.query_id.clone();
        let mut parameters = vec![("query_id", &*query_id), ("default_format", tsv::FORMAT)];
        // An insert returns nothing, so waiting for its end costs nothing
        // and makes the summary header count every row written.
        if tag == "INSERT" {
            parameters.push(("wait_end_of_query", "1"));
        }
        let response = match self.send(&sql, &parameters).await {
            Ok(response) => response,
            Err(error) => {
                statement.settle(&error);
                return Err(error);
            }
        };
        let format = http::header(&response, "X-ClickHouse-Format");
        let written = http::header(&response, "X-ClickHouse-Summary")
            .and_then(|summary| written_rows(&summary));
        let mut body = http::body(response, sql.clone());
        match describe(&mut body, format.as_deref()).await {
            Err(error) => {
                statement.settle(&error);
                Err(error)
            }
            Ok(None) => {
                statement.finish();
                let rows = if counts_rows(&tag) { written } else { None };
                Ok(StatementOutcome::Command(CommandSummary::new(tag, rows)))
            }
            Ok(Some(Description::Columns(columns))) => {
                let categories = columns.iter().map(ColumnInfo::category).collect();
                let rows = Rows {
                    rows: typed_rows(body, categories),
                    statement: Some(statement),
                };
                Ok(StatementOutcome::Rows(RowStream::new(
                    columns,
                    rows.boxed(),
                )))
            }
            Ok(Some(Description::Text { format, first })) => {
                let rows = Rows {
                    rows: text_rows(body, first),
                    statement: Some(statement),
                };
                let columns = vec![ColumnInfo::new(format, "String", TypeCategory::Text)];
                Ok(StatementOutcome::Rows(RowStream::new(
                    columns,
                    rows.boxed(),
                )))
            }
        }
    }
}

/// What a result is made of, read from its first lines.
enum Description {
    Columns(Vec<ColumnInfo>),
    /// The result of a statement that named its own format: its text, one
    /// line per row, in a single column named after the format.
    Text {
        format: String,
        first: Vec<u8>,
    },
}

/// The columns of the result in `body`; `None` for a statement that returned
/// nothing.
async fn describe(body: &mut tsv::Body, format: Option<&str>) -> Result<Option<Description>> {
    let Some(first) = body.next_line().await? else {
        return Ok(None);
    };
    if let Some(format) = format.filter(|format| !OWN_FORMATS.contains(format)) {
        return Ok(Some(Description::Text {
            format: format.to_string(),
            first,
        }));
    }
    let names = tsv::fields(&first);
    let types = match body.next_line().await? {
        Some(line) => tsv::fields(&line),
        None => Vec::new(),
    };
    anyhow::ensure!(
        types.len() == names.len(),
        "The result has {} column names but {} column types",
        names.len(),
        types.len()
    );
    let columns = names
        .into_iter()
        .zip(types)
        .map(|(name, type_name)| {
            let type_name = type_name.unwrap_or_default();
            let category = types::category(&type_name);
            ColumnInfo::new(name.unwrap_or_default(), type_name, category)
        })
        .collect();
    Ok(Some(Description::Columns(columns)))
}

fn typed_rows(body: tsv::Body, categories: Arc<[TypeCategory]>) -> BoxStream<'static, Result<Row>> {
    futures::stream::try_unfold(body, move |mut body| {
        let categories = categories.clone();
        async move {
            let Some(line) = body.next_line().await? else {
                return Ok(None);
            };
            let values = tsv::fields(&line);
            if values.len() != categories.len() {
                return Err(malformed_row(
                    &line,
                    values.len(),
                    categories.len(),
                    body.sql(),
                ));
            }
            let row: Row = values
                .into_iter()
                .zip(categories.iter())
                .map(|(value, category)| match value {
                    Some(text) => Value::from_text(&text, *category),
                    None => Value::Null,
                })
                .collect();
            Ok(Some((row, body)))
        }
    })
    .boxed()
}

fn text_rows(body: tsv::Body, first: Vec<u8>) -> BoxStream<'static, Result<Row>> {
    let row = |line: &[u8]| -> Row {
        let text = String::from_utf8_lossy(line);
        Box::new([Value::Text(text.trim_end_matches('\r').into())])
    };
    let rest = futures::stream::try_unfold(body, move |mut body| async move {
        Ok(body.next_line().await?.map(|line| (row(&line), body)))
    });
    futures::stream::once(async move { Ok(row(&first)) })
        .chain(rest)
        .boxed()
}

/// The error for a row whose values do not match the columns: an exception
/// the server wrote in the middle of the row, or a result DataKit cannot
/// read.
fn malformed_row(line: &[u8], values: usize, columns: usize, sql: &str) -> anyhow::Error {
    let text = String::from_utf8_lossy(line);
    if text.contains("DB::Exception") {
        return error::exception(&text, None, sql).into();
    }
    anyhow::anyhow!("A row of the result has {values} values for {columns} columns")
}

/// The rows of a result, holding the session until the last one.
struct Rows {
    rows: BoxStream<'static, Result<Row>>,
    /// `None` once the rows have ended.
    statement: Option<Running>,
}

impl Stream for Rows {
    type Item = Result<Row>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.statement.is_none() {
            return Poll::Ready(None);
        }
        let item = match self.rows.poll_next_unpin(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(item) => item,
        };
        match item {
            Some(Ok(row)) => Poll::Ready(Some(Ok(row))),
            Some(Err(error)) => {
                if let Some(statement) = self.statement.take() {
                    statement.settle(&error);
                }
                Poll::Ready(Some(Err(error)))
            }
            None => {
                if let Some(statement) = self.statement.take() {
                    statement.finish();
                }
                Poll::Ready(None)
            }
        }
    }
}

/// A statement the server may still be running for this session.
///
/// Dropped before [`Self::finish`] — its rows abandoned, its future dropped,
/// its response broken off — it asks the server to stop the statement and
/// keeps the session's turn until the server has: closing the HTTP
/// connection alone leaves an `INSERT … SELECT` or a long aggregation
/// running, and the session busy, until the server next tries to write.
struct Running {
    http: Http,
    query_id: Arc<str>,
    running: Arc<Mutex<Option<Arc<str>>>>,
    runtime: Option<Handle>,
    turn: Option<OwnedMutexGuard<()>>,
    finished: bool,
}

impl Running {
    /// A new statement in `session`, which `turn` is the session's turn for.
    fn start(session: &Session, turn: OwnedMutexGuard<()>) -> Self {
        let query_id: Arc<str> = uuid::Uuid::new_v4().to_string().into();
        *session
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(query_id.clone());
        Self {
            http: session.http.clone(),
            query_id,
            running: session.running.clone(),
            runtime: session.runtime.clone(),
            turn: Some(turn),
            finished: false,
        }
    }

    /// The statement has ended; the session is free.
    fn finish(mut self) {
        self.finished = true;
    }

    /// The statement failed with `error`. An error the server reported means
    /// the statement has ended; any other — the network — may leave it
    /// running, so it is stopped.
    fn settle(self, error: &anyhow::Error) {
        if error.is::<DatabaseError>() {
            self.finish();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            if running.as_deref() == Some(&*self.query_id) {
                *running = None;
            }
        }
        if self.finished {
            return;
        }
        let turn = self.turn.take();
        let http = self.http.clone();
        let query_id = self.query_id.clone();
        match self.runtime.clone().or_else(|| Handle::try_current().ok()) {
            Some(runtime) => {
                runtime.spawn(async move {
                    match tokio::time::timeout(KILL_WAIT, http.kill(&query_id, true)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::debug!("couldn’t stop an abandoned statement: {error}")
                        }
                        Err(_) => tracing::debug!("an abandoned statement is slow to stop"),
                    }
                    drop(turn);
                });
            }
            None => tracing::debug!("an abandoned statement runs on: no runtime to stop it"),
        }
    }
}

/// How many rows the statement wrote, from the `X-ClickHouse-Summary`
/// header, whose numbers are strings.
fn written_rows(summary: &str) -> Option<u64> {
    let summary: serde_json::Value = serde_json::from_str(summary).ok()?;
    match summary.get("written_rows")? {
        serde_json::Value::String(rows) => rows.parse().ok(),
        rows => rows.as_u64(),
    }
}

/// The command a statement runs, named as PostgreSQL's command tags name
/// theirs: `INSERT`, `CREATE TABLE`, `ALTER TABLE`.
fn command_tag(sql: &str) -> String {
    let words: Vec<String> = leading_words(sql).take(5).collect();
    let Some(first) = words.first() else {
        return String::new();
    };
    match first.as_str() {
        "CREATE" | "DROP" | "ALTER" | "ATTACH" | "DETACH" => {
            let mut object = words.iter().skip(1).skip_while(|word| {
                matches!(word.as_str(), "OR" | "REPLACE" | "TEMPORARY" | "TEMP")
            });
            match (object.next(), object.next()) {
                // `CREATE MATERIALIZED VIEW`, `CREATE LIVE VIEW`.
                (Some(modifier), Some(view))
                    if matches!(modifier.as_str(), "MATERIALIZED" | "LIVE" | "WINDOW")
                        && view == "VIEW" =>
                {
                    format!("{first} {modifier} VIEW")
                }
                (Some(object), _) => format!("{first} {object}"),
                (None, _) => first.clone(),
            }
        }
        _ => first.clone(),
    }
}

/// Whether the command's row count means something. ClickHouse counts the
/// rows a statement writes, which only an insert reports truthfully: a
/// mutation is applied in the background, after the summary is sent.
fn counts_rows(tag: &str) -> bool {
    tag == "INSERT"
}

/// Whether `sql` holds nothing but comments, whitespace and semicolons.
fn is_blank(sql: &str) -> bool {
    let mut rest = sql;
    loop {
        rest = skip_comments(rest);
        match rest.strip_prefix(';') {
            Some(after) => rest = after,
            None => return rest.is_empty(),
        }
    }
}

/// `sql` after the whitespace and comments it starts with.
fn skip_comments(mut rest: &str) -> &str {
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--").or_else(|| rest.strip_prefix('#')) {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            return rest;
        }
    }
}

/// The leading words of `sql`, upper-cased, skipping comments.
fn leading_words(sql: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = sql;
    std::iter::from_fn(move || {
        rest = skip_comments(rest);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tags_name_the_command_and_its_object() {
        assert_eq!(command_tag("insert into t values (1)"), "INSERT");
        assert_eq!(
            command_tag("-- note\n# shell style\n/* x */ create or replace view v as select 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("CREATE MATERIALIZED VIEW mv TO t AS SELECT 1"),
            "CREATE MATERIALIZED VIEW"
        );
        assert_eq!(
            command_tag("CREATE TEMPORARY TABLE t (a UInt8)"),
            "CREATE TABLE"
        );
        assert_eq!(command_tag("ALTER TABLE t DELETE WHERE 1"), "ALTER TABLE");
        assert_eq!(command_tag("optimize table t final"), "OPTIMIZE");
        assert_eq!(command_tag("(SELECT 1)"), "");
    }

    #[test]
    fn only_comments_make_a_blank_statement() {
        assert!(is_blank(" -- nothing\n/* at all */ ; "));
        assert!(!is_blank("-- a query\nSELECT 1"));
        assert!(!is_blank("(SELECT 1)"));
    }

    #[test]
    fn the_summary_counts_written_rows() {
        assert_eq!(
            written_rows(r#"{"read_rows":"3","written_rows":"3","written_bytes":"24"}"#),
            Some(3)
        );
        assert_eq!(written_rows(r#"{"written_rows":5}"#), Some(5));
        assert_eq!(written_rows("not json"), None);
        assert!(counts_rows("INSERT"));
        assert!(!counts_rows("ALTER TABLE"));
    }

    #[test]
    fn a_row_broken_by_an_exception_reports_the_exception() {
        let error = malformed_row(
            b"1\tCode: 241. DB::Exception: Memory limit exceeded. (MEMORY_LIMIT_EXCEEDED)",
            2,
            3,
            "SELECT 1",
        );
        let error = error.downcast_ref::<DatabaseError>().unwrap();
        assert_eq!(error.code(), Some("241"));
        assert_eq!(error.message(), "Memory limit exceeded.");
    }
}
