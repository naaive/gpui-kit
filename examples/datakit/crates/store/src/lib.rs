//! What DataKit keeps between runs.
//!
//! - [`DataSourceFile`]: the data sources, as JSON a person can read and back
//!   up. It never contains a password.
//! - [`SecretStore`]: passwords, in the system keychain.
//! - [`QueryHistory`]: every statement run, in SQLite so it can be searched.
//! - [`CatalogCache`]: the last catalog read from each data source.
//!
//! Everything here is synchronous file IO; callers run it off the UI thread.

mod catalog_cache;
mod data_sources;
mod history;
mod secrets;

pub use catalog_cache::CatalogCache;
pub use data_sources::DataSourceFile;
pub use history::{HistoryEntry, HistoryOutcome, QueryHistory};
pub use secrets::{KeychainSecrets, MemorySecrets, SecretStore};
