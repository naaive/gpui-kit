use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use datakit_catalog::Schema;
use futures::{Stream, stream::BoxStream};

use crate::{ColumnInfo, ConnectionProfile, Dialect, Value};

/// A future a driver returns: `Send` and `'static`, so it can be handed to
/// whichever runtime the driver needs.
pub type BoxFuture<T> = futures::future::BoxFuture<'static, anyhow::Result<T>>;

/// One row of a result, one value per column.
pub type Row = Box<[Value]>;

/// A kind of database DataKit can connect to.
pub trait Driver: Send + Sync {
    /// The stable id stored in profiles, such as `postgresql`.
    fn id(&self) -> &'static str;

    /// The product name shown to people.
    fn name(&self) -> &'static str;

    /// The port a server listens on unless configured otherwise; unused by a
    /// driver whose database is a file.
    fn default_port(&self) -> u16;

    /// The user a new data source starts with, such as `postgres`.
    fn default_user(&self) -> &'static str {
        ""
    }

    /// Whether a database is a file, named by the profile's
    /// [`ConnectionProfile::FILE`] option, rather than a server.
    fn is_file_based(&self) -> bool {
        false
    }

    fn dialect(&self) -> Arc<dyn Dialect>;

    /// Open a session to the database `profile` describes.
    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>>;
}

/// One session with a database.
///
/// A session runs one statement at a time; a statement sent while another is
/// running waits for it. Session state — the transaction, `SET` variables,
/// temporary tables — belongs to the session, which is why each console owns
/// its own.
pub trait Connection: Send + Sync {
    /// The server's product and version, as the server reports it.
    fn server_version(&self) -> Arc<str>;

    /// Whether the session has ended, by the server or the network.
    fn is_closed(&self) -> bool;

    /// Run one statement.
    ///
    /// The future resolves as soon as the server has described the result,
    /// before any rows arrive; rows then stream through
    /// [`StatementOutcome::Rows`]. Dropping that stream before it ends
    /// cancels the statement.
    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome>;

    /// Ask the server to stop the statement this session is running. A
    /// session that is idle ignores the request.
    fn cancel(&self) -> BoxFuture<()>;

    /// Every schema of the database, without their relations.
    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>>;

    /// Everything in `schema`: relations with their columns, indexes,
    /// constraints and triggers; routines; sequences.
    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema>;

    /// The schemas an unqualified name is looked up in for this session.
    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>>;
}

/// What running a statement produced.
pub enum StatementOutcome {
    /// The statement returns rows; they stream as the server sends them.
    Rows(RowStream),
    /// The statement changed something or returned nothing.
    Command(CommandSummary),
}

/// The server's report on a statement that returned no rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSummary {
    tag: Arc<str>,
    rows: Option<u64>,
}

impl CommandSummary {
    /// `tag` is the command the server ran (`INSERT`, `CREATE TABLE`); `rows`
    /// is how many rows it touched, for commands that count them.
    pub fn new(tag: impl Into<Arc<str>>, rows: Option<u64>) -> Self {
        Self {
            tag: tag.into(),
            rows,
        }
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    pub fn rows(&self) -> Option<u64> {
        self.rows
    }
}

/// The rows of a result, as they arrive.
pub struct RowStream {
    columns: Arc<[ColumnInfo]>,
    rows: BoxStream<'static, anyhow::Result<Row>>,
}

impl RowStream {
    pub fn new(
        columns: impl Into<Arc<[ColumnInfo]>>,
        rows: BoxStream<'static, anyhow::Result<Row>>,
    ) -> Self {
        Self {
            columns: columns.into(),
            rows,
        }
    }

    pub fn columns(&self) -> &Arc<[ColumnInfo]> {
        &self.columns
    }
}

impl Stream for RowStream {
    type Item = anyhow::Result<Row>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rows.as_mut().poll_next(cx)
    }
}

/// The drivers an application can connect with, found by [`Driver::id`].
#[derive(Clone, Default)]
pub struct DriverRegistry {
    drivers: Vec<Arc<dyn Driver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_driver(mut self, driver: Arc<dyn Driver>) -> Self {
        self.drivers.push(driver);
        self
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Driver>> {
        self.drivers.iter().find(|driver| driver.id() == id)
    }

    pub fn drivers(&self) -> &[Arc<dyn Driver>] {
        &self.drivers
    }
}
