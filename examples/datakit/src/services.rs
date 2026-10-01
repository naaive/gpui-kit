//! The application-wide services every feature reaches for: the IO runtime,
//! the drivers, the password store and where files live.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use datakit_driver::DriverRegistry;
use datakit_driver_clickhouse::ClickHouseDriver;
use datakit_driver_mssql::SqlServerDriver;
use datakit_driver_mysql::MySqlDriver;
use datakit_driver_postgres::PostgresDriver;
use datakit_driver_sqlite::SqliteDriver;
use datakit_runtime::{IoRuntime, RemoteStream, RemoteTask};
use datakit_store::{CatalogCache, KeychainSecrets, SecretStore};
use futures::Stream;
use gpui_kit::{App, Global};

pub struct Services {
    runtime: IoRuntime,
    drivers: DriverRegistry,
    secrets: Arc<dyn SecretStore>,
    catalog_cache: Arc<CatalogCache>,
    data_directory: PathBuf,
}

impl Global for Services {}

impl Services {
    pub fn init(cx: &mut App) -> Result<()> {
        // `DATAKIT_DATA_DIR` keeps a second copy, such as a test run, apart
        // from the everyday one.
        let data_directory = std::env::var_os("DATAKIT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::data_dir()
                    .unwrap_or_else(std::env::temp_dir)
                    .join("DataKit")
            });
        let services = Self {
            runtime: IoRuntime::new()?,
            drivers: DriverRegistry::new()
                .with_driver(Arc::new(PostgresDriver::new()))
                .with_driver(Arc::new(MySqlDriver::new()))
                .with_driver(Arc::new(SqliteDriver::new()))
                .with_driver(Arc::new(SqlServerDriver::new()))
                .with_driver(Arc::new(ClickHouseDriver::new())),
            secrets: Arc::new(KeychainSecrets::new()),
            catalog_cache: Arc::new(CatalogCache::new(data_directory.join("cache"))),
            data_directory,
        };
        cx.set_global(services);
        Ok(())
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Run database IO on the runtime the drivers need. Dropping the task
    /// aborts it.
    pub fn spawn<F, T>(&self, future: F) -> RemoteTask<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        self.runtime.spawn(future)
    }

    /// Drive a stream on the IO runtime, at most `buffer` items ahead.
    pub fn forward<S>(&self, stream: S, buffer: usize) -> RemoteStream<S::Item>
    where
        S: Stream + Send + 'static,
        S::Item: Send + 'static,
    {
        self.runtime.forward(stream, buffer)
    }

    pub fn drivers(&self) -> &DriverRegistry {
        &self.drivers
    }

    pub fn secrets(&self) -> Arc<dyn SecretStore> {
        self.secrets.clone()
    }

    /// The last catalog read from each data source.
    pub fn catalog_cache(&self) -> Arc<CatalogCache> {
        self.catalog_cache.clone()
    }

    /// The SSH host keys DataKit has seen, trusted on first use.
    pub fn known_hosts(&self) -> PathBuf {
        self.data_directory.join("known_hosts")
    }

    /// Where DataKit keeps its files: data sources, history, layout and
    /// consoles.
    pub fn data_directory(&self) -> &Path {
        &self.data_directory
    }
}
