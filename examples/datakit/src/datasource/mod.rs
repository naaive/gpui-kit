//! Data sources: where DataKit connects, and what it knows about each one.
//!
//! A [`DataSource`] is one configured database. It keeps a connection of its
//! own for reading metadata — the explorer and completion share it — and the
//! [`Catalog`](datakit_catalog::Catalog) that connection has read so far.
//! Consoles open sessions of their own with [`open_connection`], so a long
//! query never blocks the explorer.
//!
//! [`DataSources`] is the list, persisted to `data-sources.json`;
//! [`DataSourceForm`] creates and edits entries.

mod data_source;
mod data_sources;
mod form;

use std::sync::Arc;

use anyhow::anyhow;
use datakit_driver::{Connection, ConnectionProfile, DataSourceId};
use datakit_runtime::RemoteTask;
use gpui_kit::{App, SharedString};

pub use data_source::{
    CatalogRequest, ConnectionStatus, DataSource, DataSourceColor, DataSourceEvent,
};
pub use data_sources::{DataSources, DataSourcesEvent};
pub use form::DataSourceForm;

use crate::services::Services;

pub(crate) use datakit_tunnel as tunnel;

/// The profile option holding a data source's [`DataSourceColor`].
pub const COLOR: &str = "color";
/// The profile option that is `true` for a data source whose data must not
/// change.
pub const READ_ONLY: &str = "read_only";

/// Open a new session to `profile`'s database, through its SSH tunnel when
/// it has one. A password of `None` is read from the keychain.
pub fn open_connection(
    profile: &ConnectionProfile,
    password: Option<String>,
    ssh_password: Option<String>,
    cx: &App,
) -> RemoteTask<Arc<dyn Connection>> {
    let services = Services::global(cx);
    let driver = services.drivers().get(profile.driver()).cloned();
    let secrets = services.secrets();
    let known_hosts = services.known_hosts();
    let profile = profile.clone();
    services.spawn(async move {
        let driver = driver
            .ok_or_else(|| anyhow!("DataKit has no driver for “{}” databases", profile.driver()))?;
        let password = match password {
            Some(password) => Some(password),
            None => secrets.read(profile.id())?,
        };
        let ssh_password = match ssh_password {
            Some(password) => Some(password),
            None if tunnel::uses_tunnel(&profile) => secrets.read(&ssh_secret_id(profile.id()))?,
            None => None,
        };
        tunnel::connect(driver, profile, password, ssh_password, known_hosts).await
    })
}

/// Where the keychain keeps a data source's SSH password or key passphrase.
pub fn ssh_secret_id(id: &DataSourceId) -> DataSourceId {
    DataSourceId::from(format!("{id}#ssh").as_str())
}

/// An error as one line a person can read: each cause after the last.
pub fn describe_error(error: &anyhow::Error) -> SharedString {
    format!("{error:#}").into()
}
