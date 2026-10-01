use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow};
use datakit_catalog::{Role, Schema};
use datakit_driver::{
    BoxFuture, ColumnInfo, CommandSummary, Connection, Row, RowStream, SslMode, StatementOutcome,
    TypeCategory,
};
use futures::{FutureExt as _, Stream, StreamExt as _};
use mysql_async::{
    Conn, Opts, OptsBuilder, Pool, PoolConstraints, PoolOpts, prelude::Queryable as _,
};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc, oneshot};

use crate::{
    error::{convert, is_empty_query},
    introspect, tls, types,
};

/// How long connecting may take before it is given up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Rows read ahead of the reader. The server stops sending when they are not
/// taken, so a result larger than memory streams instead of piling up.
const ROW_BUFFER: usize = 256;

/// One MySQL session.
///
/// Statements run one at a time on the session, each served by a Tokio task
/// that holds the session until its result has been read to the end.
/// Reading the catalog uses a small pool of other sessions instead, so the
/// explorer can load while a long statement runs; MySQL sessions cannot
/// interleave statements, and the catalog does not depend on session state.
pub(crate) struct MySqlConnection {
    session: Arc<Mutex<Conn>>,
    killer: Killer,
    metadata: Pool,
    closed: Arc<AtomicBool>,
    server_version: Arc<str>,
    is_mariadb: bool,
}

impl MySqlConnection {
    pub(crate) async fn connect(opts: OptsBuilder, ssl_mode: SslMode) -> Result<Self> {
        let plain = Opts::from(opts.clone());
        let (mut conn, opts) = match tls::ssl_opts(ssl_mode) {
            None => (connect_within_timeout(plain.clone()).await?, plain),
            Some(ssl) => {
                let secure = Opts::from(opts.ssl_opts(ssl));
                match connect_within_timeout(secure.clone()).await {
                    Ok(conn) => (conn, secure),
                    // `Prefer` settles for a plain session when the server
                    // has no TLS.
                    Err(error) if ssl_mode == SslMode::Prefer && lacks_tls(&error) => {
                        (connect_within_timeout(plain.clone()).await?, plain)
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        let version: Option<String> = conn.query_first("SELECT VERSION()").await?;
        let version = version.unwrap_or_default();
        let is_mariadb = version.contains("MariaDB");
        let server_version = if is_mariadb {
            // `10.11.6-MariaDB-1:10.11.6+maria~ubu2204` is version 10.11.6.
            let number = version.split('-').next().unwrap_or(&version);
            format!("MariaDB {number}")
        } else {
            format!("MySQL {version}")
        };

        // Side sessions (`KILL QUERY` and the catalog) start in no database:
        // the session's own may be dropped while it is connected.
        let side = Opts::from(OptsBuilder::from_opts(opts).db_name(None::<String>));
        let metadata = Pool::new(
            OptsBuilder::from_opts(side.clone()).pool_opts(
                PoolOpts::new()
                    .with_constraints(PoolConstraints::new(0, 4).expect("0 ≤ 4"))
                    .with_inactive_connection_ttl(Duration::from_secs(60)),
            ),
        );
        Ok(Self {
            killer: Killer {
                opts: side,
                connection_id: conn.id(),
            },
            session: Arc::new(Mutex::new(conn)),
            metadata,
            closed: Arc::new(AtomicBool::new(false)),
            server_version: server_version.into(),
            is_mariadb,
        })
    }
}

async fn connect_within_timeout(opts: Opts) -> Result<Conn> {
    match tokio::time::timeout(CONNECT_TIMEOUT, Conn::new(opts)).await {
        Ok(conn) => conn.map_err(convert),
        Err(_) => Err(anyhow!("The server didn’t answer within 15 seconds")),
    }
}

/// Whether connecting failed because the server does not offer TLS.
fn lacks_tls(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<mysql_async::Error>(),
        Some(mysql_async::Error::Driver(
            mysql_async::DriverError::NoClientSslFlagFromServer
        ))
    )
}

impl Drop for MySqlConnection {
    fn drop(&mut self) {
        let metadata = self.metadata.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = metadata.disconnect().await {
                    tracing::debug!("couldn’t close the catalog sessions: {error}");
                }
            });
        }
    }
}

impl Connection for MySqlConnection {
    fn server_version(&self) -> Arc<str> {
        self.server_version.clone()
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        let session = self.session.clone();
        let killer = self.killer.clone();
        let closed = self.closed.clone();
        async move {
            let conn = session.lock_owned().await;
            let (outcome, receiver) = oneshot::channel();
            tokio::spawn(run(conn, sql, outcome, killer, closed));
            receiver
                .await
                .unwrap_or_else(|_| Err(anyhow!("The statement stopped unexpectedly")))
        }
        .boxed()
    }

    fn cancel(&self) -> BoxFuture<()> {
        let killer = self.killer.clone();
        async move { killer.kill().await }.boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        introspect::schemas(self.metadata.clone()).boxed()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        introspect::schema(self.metadata.clone(), schema, self.is_mariadb).boxed()
    }

    fn introspect_roles(&self) -> BoxFuture<Vec<Role>> {
        introspect::roles(self.metadata.clone()).boxed()
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        let session = self.session.clone();
        async move {
            let mut conn = session.lock().await;
            let database: Option<Option<String>> = conn
                .query_first("SELECT DATABASE()")
                .await
                .context("Couldn’t read the current database")?;
            Ok(database.flatten().map(Arc::from).into_iter().collect())
        }
        .boxed()
    }
}

/// Stops the statement a session is running, from another session: MySQL
/// has no out-of-band cancel request, only `KILL QUERY`.
#[derive(Clone)]
struct Killer {
    opts: Opts,
    /// The server's id for the session, which `KILL QUERY` names.
    connection_id: u32,
}

impl Killer {
    async fn kill(&self) -> Result<()> {
        let mut conn = connect_within_timeout(self.opts.clone()).await?;
        conn.query_drop(format!("KILL QUERY {}", self.connection_id))
            .await
            .map_err(convert)?;
        conn.disconnect().await?;
        Ok(())
    }
}

/// Where a statement whose rows are being read is: still running, read to
/// the end, or abandoned by its reader and being killed. The task reading
/// rows and the watcher for abandonment agree through it, so the statement
/// is killed once, and a kill never lands on the next statement.
const RUNNING: u8 = 0;
const FINISHED: u8 = 1;
const KILLING: u8 = 2;

/// Run `sql` on the session `conn` and report its outcome; when it returns
/// rows, go on reading them into the outcome's stream.
///
/// A multi-statement text reports its first result; the results after it
/// are read and discarded, and only an error among them is reported, as the
/// last item of the stream or instead of the command's summary.
async fn run(
    mut conn: OwnedMutexGuard<Conn>,
    sql: Arc<str>,
    outcome: oneshot::Sender<Result<StatementOutcome>>,
    killer: Killer,
    closed: Arc<AtomicBool>,
) {
    let failed = |error: mysql_async::Error| {
        if error.is_fatal() {
            closed.store(true, Ordering::SeqCst);
        }
        convert(error)
    };

    let mut result = match conn.query_iter(&*sql).await {
        Ok(result) => result,
        Err(error) if is_empty_query(&error) => {
            let _ = outcome.send(Ok(StatementOutcome::Command(CommandSummary::new("", None))));
            return;
        }
        Err(error) => {
            let _ = outcome.send(Err(failed(error)));
            return;
        }
    };

    let Some(columns) = result.columns().filter(|columns| !columns.is_empty()) else {
        let affected = result.affected_rows();
        let tag = command_tag(&sql);
        let summary = CommandSummary::new(tag.clone(), counts_rows(&tag).then_some(affected));
        let reported = match result.drop_result().await {
            Ok(()) => Ok(StatementOutcome::Command(summary)),
            Err(error) => Err(failed(error)),
        };
        let _ = outcome.send(reported);
        return;
    };

    let infos: Vec<ColumnInfo> = columns.iter().map(types::column_info).collect();
    let categories: Vec<TypeCategory> = infos.iter().map(ColumnInfo::category).collect();
    let (rows, receiver) = mpsc::channel(ROW_BUFFER);
    let (dropped, abandoned) = oneshot::channel::<()>();
    let stream = Rows {
        receiver,
        _dropped: dropped,
    };
    if outcome
        .send(Ok(StatementOutcome::Rows(RowStream::new(
            infos,
            stream.boxed(),
        ))))
        .is_err()
    {
        // Nobody is waiting for the result any more.
        kill_abandoned(&killer).await;
        let _ = result.drop_result().await;
        return;
    }

    // The reader may leave while the task waits for the server, which only
    // a watcher notices, or while the task hands it a row, which the task
    // notices itself. Whichever notices first kills the statement.
    let state = Arc::new(AtomicU8::new(RUNNING));
    let watcher = tokio::spawn({
        let state = state.clone();
        let killer = killer.clone();
        async move {
            // Resolves only when the stream is dropped: nothing is sent.
            let _ = abandoned.await;
            if state
                .compare_exchange(RUNNING, KILLING, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                kill_abandoned(&killer).await;
            }
        }
    });

    let mut failure = None;
    let mut left = false;
    loop {
        match result.next().await {
            Ok(Some(row)) => {
                let values: Row = categories
                    .iter()
                    .enumerate()
                    .map(|(ix, category)| types::value(row.as_ref(ix), *category))
                    .collect();
                if rows.send(Ok(values)).await.is_err() {
                    left = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }

    let next = if left { KILLING } else { FINISHED };
    if state
        .compare_exchange(RUNNING, next, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        watcher.abort();
        if left {
            kill_abandoned(&killer).await;
        }
    } else {
        // The watcher is killing: let the kill land before the session is
        // drained and handed to the next statement.
        let _ = watcher.await;
    }

    let rest = result.drop_result().await;
    if let Some(error) = failure.or(rest.err()) {
        let _ = rows.send(Err(failed(error))).await;
    }
}

async fn kill_abandoned(killer: &Killer) {
    if let Err(error) = killer.kill().await {
        tracing::debug!("couldn’t stop an abandoned statement: {error}");
    }
}

/// The rows of a result as the statement's task reads them.
///
/// Dropping it before the end tells the task, which kills the statement
/// with `KILL QUERY`: otherwise the session would go on receiving, and
/// discarding, every remaining row before it could run anything else.
struct Rows {
    receiver: mpsc::Receiver<Result<Row>>,
    _dropped: oneshot::Sender<()>,
}

impl Stream for Rows {
    type Item = Result<Row>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

/// The command a statement runs, named as PostgreSQL names it in its
/// command tag so every driver reports the same: `INSERT`, `CREATE TABLE`,
/// `SELECT`.
fn command_tag(sql: &str) -> String {
    let words: Vec<String> = leading_words(sql).take(6).collect();
    let Some(first) = words.first() else {
        return String::new();
    };
    match first.as_str() {
        "CREATE" | "DROP" | "ALTER" => {
            // `CREATE OR REPLACE VIEW`, `CREATE UNIQUE INDEX` and
            // `CREATE DEFINER = x PROCEDURE` name the object after the
            // modifiers.
            let object = words.iter().skip(1).find(|word| {
                !matches!(
                    word.as_str(),
                    "OR" | "REPLACE"
                        | "UNIQUE"
                        | "FULLTEXT"
                        | "SPATIAL"
                        | "TEMPORARY"
                        | "ONLINE"
                        | "OFFLINE"
                        | "IGNORE"
                        | "ALGORITHM"
                        | "DEFINER"
                        | "SQL"
                        | "SECURITY"
                        | "UNDEFINED"
                        | "MERGE"
                        | "TEMPTABLE"
                        | "INVOKER"
                        | "CURRENT_USER"
                        | "IF"
                        | "NOT"
                        | "EXISTS"
                )
            });
            match object {
                Some(object) => format!("{first} {object}"),
                None => first.clone(),
            }
        }
        _ => first.clone(),
    }
}

/// The leading words of `sql`, upper-cased, skipping comments.
fn leading_words(sql: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = sql;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
            if let Some(after) = rest.strip_prefix("--").or_else(|| rest.strip_prefix('#')) {
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

/// Whether the command's affected-row count means something.
fn counts_rows(tag: &str) -> bool {
    matches!(tag, "INSERT" | "UPDATE" | "DELETE" | "REPLACE" | "LOAD")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tags_name_the_command_and_its_object() {
        assert_eq!(command_tag("insert into t values (1)"), "INSERT");
        assert_eq!(
            command_tag("-- note\n# another\n/* x */ create or replace view v as select 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("CREATE UNIQUE INDEX i ON t (a)"),
            "CREATE INDEX"
        );
        assert_eq!(
            command_tag("CREATE ALGORITHM = MERGE VIEW v AS SELECT 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("create table if not exists t (a int)"),
            "CREATE TABLE"
        );
        assert_eq!(command_tag("drop table t"), "DROP TABLE");
        assert_eq!(command_tag("  "), "");
    }

    #[test]
    fn only_data_commands_count_rows() {
        assert!(counts_rows("REPLACE"));
        assert!(!counts_rows("CREATE TABLE"));
    }
}
