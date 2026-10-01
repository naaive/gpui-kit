//! DataKit's SQLite driver, on `rusqlite` with SQLite bundled.
//!
//! A database is a file, named by the profile's [`ConnectionProfile::FILE`]
//! option; `:memory:` opens a private in-memory database. SQLite's API is
//! synchronous, so every connection owns a thread that holds the database
//! handle and runs one job at a time. The futures handed back only wait for
//! that thread, which keeps them `Send + 'static`, independent of any
//! runtime, and off the Tokio workers.
//!
//! Values arrive as SQLite's storage classes: `NULL`, integers, reals and
//! text map to the matching [`Value`](datakit_driver::Value); a blob becomes
//! text in SQLite's own literal syntax, `X'0A0B'`, which
//! [`SqliteDialect::literal`](datakit_driver::Dialect::literal) writes back
//! as a blob. A column's [`TypeCategory`](datakit_driver::TypeCategory)
//! follows the affinity of its declared type, or the storage class of the
//! first row when the column is an expression with no declared type.
//!
//! Every session enforces foreign keys (`PRAGMA foreign_keys = ON`), as the
//! catalog that DataKit shows them in implies.

mod connection;
mod dialect;
mod error;
mod introspect;
mod parse;
mod plan;
mod types;

use std::sync::Arc;

use anyhow::Context as _;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver};
use futures::FutureExt as _;

pub use dialect::SqliteDialect;

use crate::connection::SqliteConnection;

/// The SQLite [`Driver`].
#[derive(Default)]
pub struct SqliteDriver;

impl SqliteDriver {
    pub const ID: &str = "sqlite";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for SqliteDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "SQLite"
    }

    fn default_port(&self) -> u16 {
        0
    }

    fn is_file_based(&self) -> bool {
        true
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(SqliteDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        // SQLite has no users; a password means nothing to it.
        let _ = password;
        let file = profile
            .option(ConnectionProfile::FILE)
            .map(str::trim)
            .filter(|file| !file.is_empty())
            .map(String::from);
        async move {
            let file = file.context("Choose a database file to connect to")?;
            let connection = SqliteConnection::open(file.clone())
                .await
                .with_context(|| format!("Couldn’t open {file}"))?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}
