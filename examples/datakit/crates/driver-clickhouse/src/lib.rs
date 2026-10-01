//! DataKit's ClickHouse driver, over ClickHouse's HTTP interface.
//!
//! Every statement is one HTTP request, and every result arrives as
//! `TabSeparatedWithNamesAndTypes`: a line of column names, a line of their
//! ClickHouse types, then one line per row, read as the server sends them.
//! Values keep the server's text; the type decides how they are parsed and
//! aligned, as for every driver.
//!
//! A [`Connection`] is a ClickHouse HTTP session: requests carry the same
//! `session_id`, so `SET`, `USE` and temporary tables last from one
//! statement to the next. The session expires after an hour without a
//! statement, or sooner when the server allows less; the next statement then
//! starts a new, empty one.
//!
//! Each statement has a `query_id` of its own. [`Connection::cancel`] and an
//! abandoned result both stop it with `KILL QUERY`, sent outside the session,
//! because ClickHouse keeps running a statement whose client has gone until
//! it next tries to write.
//!
//! A statement with its own `FORMAT` clause returns that format's text: one
//! row per line, in a single column named after the format. Binary formats
//! arrive as unreadable text.
//!
//! Every future must run on a Tokio runtime: requests are made with
//! `reqwest` on Tokio sockets.

mod connection;
mod dialect;
mod error;
mod http;
mod introspect;
mod plan;
mod text;
mod tsv;
mod types;

use std::sync::Arc;

use anyhow::Context as _;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Dialect, Driver, SslMode};
use futures::FutureExt as _;

pub use dialect::ClickHouseDialect;

use crate::{
    connection::ClickHouseConnection,
    http::{Http, Transport},
};

/// The port ClickHouse serves HTTPS on by convention.
const HTTPS_PORT: u16 = 8443;

/// The ClickHouse [`Driver`].
///
/// ClickHouse serves HTTP and HTTPS on separate ports, so TLS cannot be
/// offered and declined on one connection. The SSL modes mean:
///
/// - `Disable`: plain HTTP.
/// - `Prefer`: plain HTTP, except on port 8443, ClickHouse's HTTPS port,
///   where it is HTTPS without verifying the certificate.
/// - `Require`: HTTPS without verifying the certificate.
/// - `VerifyFull`: HTTPS, verifying the certificate against the system's
///   roots and the host name.
#[derive(Default)]
pub struct ClickHouseDriver;

impl ClickHouseDriver {
    pub const ID: &str = "clickhouse";

    pub fn new() -> Self {
        Self
    }
}

impl Driver for ClickHouseDriver {
    fn id(&self) -> &'static str {
        Self::ID
    }

    fn name(&self) -> &'static str {
        "ClickHouse"
    }

    /// The HTTP interface's port.
    fn default_port(&self) -> u16 {
        8123
    }

    fn default_user(&self) -> &'static str {
        "default"
    }

    fn dialect(&self) -> Arc<dyn Dialect> {
        Arc::new(ClickHouseDialect)
    }

    fn connect(
        &self,
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> BoxFuture<Arc<dyn Connection>> {
        let transport = match profile.ssl_mode() {
            SslMode::Disable => Transport::Http,
            SslMode::Prefer if profile.port() == HTTPS_PORT => Transport::HttpsUnverified,
            SslMode::Prefer => Transport::Http,
            SslMode::Require => Transport::HttpsUnverified,
            SslMode::VerifyFull => Transport::Https,
        };
        let http = Http::new(
            transport,
            profile.host(),
            profile.port(),
            profile.user(),
            password,
            profile.database(),
        );
        let address = profile.address();
        async move {
            let connection = ClickHouseConnection::connect(http?)
                .await
                .with_context(|| format!("Couldn’t connect to {address}"))?;
            Ok(Arc::new(connection) as Arc<dyn Connection>)
        }
        .boxed()
    }
}
