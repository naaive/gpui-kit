//! The contract between DataKit and a database.
//!
//! A [`Driver`] turns a [`ConnectionProfile`] into a [`Connection`]; a
//! connection runs statements and describes the database as
//! [`datakit_catalog`] objects. What differs between databases in *syntax* —
//! quoting, keywords, how to select the first rows of a table — is a
//! [`Dialect`], which is data the rest of DataKit reads rather than IO.
//!
//! Every future here is `Send + 'static` so it can run on the runtime the
//! driver needs (Tokio, for PostgreSQL) without the caller knowing which.

mod connection;
pub mod ddl;
mod dialect;
mod edit;
mod error;
mod plan;
mod profile;
mod value;

pub use connection::{
    BoxFuture, CommandSummary, Connection, Driver, DriverRegistry, Row, RowStream, StatementOutcome,
};
pub use dialect::Dialect;
pub use edit::RowChange;
pub use error::DatabaseError;
pub use plan::PlanNode;
pub use profile::{ConnectionProfile, DataSourceId, SslMode};
pub use value::{ColumnInfo, TypeCategory, Value};
