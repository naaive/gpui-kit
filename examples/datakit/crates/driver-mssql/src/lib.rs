//! DataKit's Microsoft SQL Server driver, on `tiberius`.
//!
//! Statements run as plain batches, the way SQL Server Management Studio
//! sends them, so `USE`, `SET` and temporary tables last for the session.
//! Values arrive typed: integers, floats and `bit` are parsed, and every
//! exact or structured type (`decimal`, `money`, dates and times,
//! `uniqueidentifier`, binary) keeps a text spelling that the server reads
//! back.
//!
//! Every future must run on a Tokio runtime: the connection is a Tokio
//! socket, and each statement is served by a Tokio task.

mod connection;
mod dialect;
mod error;
mod introspect;
mod plan;
mod types;

use std::{sync::Arc, time::Duration};

use anyhow::Context as _;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver, SslMode};
use futures::FutureExt as _;
use tiberius::{AuthMethod, Config, EncryptionLevel};

pub use dialect::SqlServerDialect;

use crate::connection::SqlServerConnection;

/// How long opening a session may take, TCP and login together.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// The Microsoft SQL Server [`Driver`], for SQL Server 2012 and later and
/// Azure SQL Database, with SQL Server authentication over TCP.
#[derive(Default)]
pub struct SqlServerDriver;

impl SqlServerDriver {
    pub const ID: &str = "sqlserver";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for SqlServerDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "SQL Server"
    }

    fn default_port(&self) -> u16 {
        1433
    }

    fn default_user(&self) -> &'static str {
        "sa"
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(SqlServerDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        let config = config(profile, password);
        let address = profile.address();
        async move {
            let connection =
                tokio::time::timeout(CONNECT_TIMEOUT, SqlServerConnection::connect(config))
                    .await
                    .map_err(|_| anyhow::anyhow!("The server didn’t answer in time"))
                    .and_then(|result| result)
                    .with_context(|| format!("Couldn’t connect to {address}"))?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}

/// The `tiberius` configuration for `profile`.
///
/// TLS follows the profile's mode: `Disable` sends nothing encrypted, not
/// even the login; `Prefer` and `Require` encrypt without checking the
/// certificate, which is what a server's self-signed certificate needs;
/// `VerifyFull` checks the certificate against the system's roots and the
/// host name.
fn config(profile: &ConnectionProfile, password: Option<String>) -> Config {
    let mut config = Config::new();
    config.host(profile.host());
    config.port(profile.port());
    config.application_name("DataKit");
    config.authentication(AuthMethod::sql_server(
        profile.user(),
        password.unwrap_or_default(),
    ));
    if !profile.database().is_empty() {
        config.database(profile.database());
    }
    match profile.ssl_mode() {
        SslMode::Disable => config.encryption(EncryptionLevel::NotSupported),
        SslMode::Prefer => {
            config.encryption(EncryptionLevel::On);
            config.trust_cert();
        }
        SslMode::Require => {
            config.encryption(EncryptionLevel::Required);
            config.trust_cert();
        }
        SslMode::VerifyFull => config.encryption(EncryptionLevel::Required),
    }
    config
}
