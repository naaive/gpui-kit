//! DataKit's Redis driver, on the `redis` crate over Tokio.
//!
//! Redis speaks commands, not SQL, so the console runs one command a line,
//! as `redis-cli` reads them, and replies become rows: a list of values one
//! a row, a hash or a scored set as pairs, a scalar as a single row.
//!
//! The explorer shows the server's numbered databases as schemas, `db0`
//! upward, and keys as [`RelationType::Key`](datakit_catalog::RelationType)
//! relations, at most [`KEY_LIMIT`] of each. Opening a key shows its value
//! through `DATAKIT.VALUE`, a command this driver answers itself: it looks
//! at the key's type and reads the value the way that type is read.

mod command;
mod connection;
mod dialect;
mod introspect;
mod reply;

use std::sync::Arc;

use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver};
use futures::FutureExt as _;

pub use dialect::RedisDialect;

use crate::connection::RedisConnection;

/// The most keys of a database the explorer lists.
pub const KEY_LIMIT: usize = 1000;

/// The Redis [`Driver`].
#[derive(Default)]
pub struct RedisDriver;

impl RedisDriver {
    pub const ID: &str = "redis";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for RedisDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "Redis"
    }

    fn default_port(&self) -> u16 {
        6379
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(RedisDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        let profile = profile.clone();
        async move {
            let connection = RedisConnection::open(&profile, password).await?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}
