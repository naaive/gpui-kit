//! DataKit's PostgreSQL driver, on `tokio-postgres`.
//!
//! Statements run through the simple-query protocol, so every value arrives
//! as the server's own text: a `numeric` keeps its digits, a `timestamptz`
//! its zone, and a type DataKit has never heard of still displays. Before
//! running, the statement is prepared once to learn its column types, which
//! decide how values are parsed and aligned.
//!
//! Every future must run on a Tokio runtime: the connection is a Tokio
//! socket served by a Tokio task.

mod connection;
mod dialect;
mod error;
mod introspect;
mod plan;
mod tls;
mod types;

use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver, SslMode};
use futures::FutureExt as _;

pub use dialect::PostgresDialect;

use crate::{connection::PostgresConnection, tls::Tls};

/// The PostgreSQL [`Driver`].
#[derive(Default)]
pub struct PostgresDriver;

impl PostgresDriver {
    pub const ID: &str = "postgresql";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for PostgresDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "PostgreSQL"
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn default_user(&self) -> &'static str {
        "postgres"
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(PostgresDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        let mut config = tokio_postgres::Config::new();
        config
            .host(profile.host())
            .port(profile.port())
            .user(profile.user())
            .application_name("DataKit")
            .connect_timeout(Duration::from_secs(15))
            .ssl_mode(match profile.ssl_mode() {
                SslMode::Disable => tokio_postgres::config::SslMode::Disable,
                SslMode::Prefer => tokio_postgres::config::SslMode::Prefer,
                SslMode::Require | SslMode::VerifyFull => tokio_postgres::config::SslMode::Require,
            });
        if !profile.database().is_empty() {
            config.dbname(profile.database());
        }
        if let Some(password) = password.filter(|password| !password.is_empty()) {
            config.password(password);
        }
        let ssl_mode = profile.ssl_mode();
        let address = profile.address();
        async move {
            let tls = Tls::new(ssl_mode)?;
            let connection = PostgresConnection::connect(config, tls)
                .await
                .with_context(|| format!("Couldn’t connect to {address}"))?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}
