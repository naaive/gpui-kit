//! DataKit's MySQL and MariaDB driver, on `mysql_async`.
//!
//! Statements run through the text protocol, so every value arrives as the
//! server's own text: a `decimal` keeps its digits, a `datetime` its
//! fractional seconds, and a type DataKit has never heard of still displays.
//! The column types the server sends with a result decide how values are
//! parsed and aligned; bytes that are not text show as hexadecimal.
//!
//! In MySQL a database is what other servers call a schema, so a data
//! source's schemas are the server's databases, and the profile's database
//! is only the one a session starts in.
//!
//! Every future must run on a Tokio runtime: the session is a Tokio socket,
//! and each statement is served by a Tokio task.

mod connection;
mod dialect;
mod error;
mod introspect;
mod plan;
mod tls;
mod types;

use std::sync::Arc;

use anyhow::Context as _;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver};
use futures::FutureExt as _;
use mysql_async::OptsBuilder;

pub use dialect::MySqlDialect;

use crate::connection::MySqlConnection;

/// The MySQL [`Driver`], which also connects to MariaDB.
#[derive(Default)]
pub struct MySqlDriver;

impl MySqlDriver {
    pub const ID: &str = "mysql";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for MySqlDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "MySQL"
    }

    fn default_port(&self) -> u16 {
        3306
    }

    fn default_user(&self) -> &'static str {
        "root"
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(MySqlDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        let opts = OptsBuilder::default()
            .ip_or_hostname(profile.host())
            .tcp_port(profile.port())
            .user((!profile.user().is_empty()).then(|| profile.user()))
            .pass(password.filter(|password| !password.is_empty()))
            .db_name((!profile.database().is_empty()).then(|| profile.database()))
            // An `UPDATE` reports the rows it matched, as other databases
            // do, not only those whose values changed.
            .client_found_rows(true)
            .prefer_socket(false)
            .connect_attribute("program_name", "DataKit");
        let ssl_mode = profile.ssl_mode();
        let address = profile.address();
        async move {
            let connection = MySqlConnection::connect(opts, ssl_mode)
                .await
                .with_context(|| format!("Couldn’t connect to {address}"))?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}
