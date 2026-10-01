use std::{collections::BTreeMap, fmt, sync::Arc};

use serde::{Deserialize, Serialize};

/// The stable identity of a data source. It survives renames and is what
/// history, secrets and layouts refer to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DataSourceId(Arc<str>);

impl DataSourceId {
    /// A new, globally unique id.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DataSourceId {
    fn from(id: &str) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for DataSourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a connection uses TLS, with libpq's meaning for each mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SslMode {
    /// Never use TLS.
    Disable,
    /// Use TLS when the server offers it, without verifying the certificate.
    #[default]
    Prefer,
    /// Always use TLS, without verifying the certificate.
    Require,
    /// Always use TLS and verify the certificate and the host name.
    VerifyFull,
}

impl SslMode {
    pub const ALL: [SslMode; 4] = [
        SslMode::Disable,
        SslMode::Prefer,
        SslMode::Require,
        SslMode::VerifyFull,
    ];
}

/// Where a data source is and who to connect as. The password is not part of
/// a profile: profiles are written to a plain settings file, and the password
/// is handed to [`Driver::connect`](crate::Driver::connect) separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    id: DataSourceId,
    name: Arc<str>,
    driver: Arc<str>,
    host: Arc<str>,
    port: u16,
    database: Arc<str>,
    user: Arc<str>,
    #[serde(default)]
    ssl_mode: SslMode,
    /// Settings only some drivers read, by name: a database file, an SSH
    /// tunnel. Never a secret.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    options: BTreeMap<Arc<str>, Arc<str>>,
}

impl ConnectionProfile {
    /// The option naming the file of a database that is a file (SQLite).
    pub const FILE: &str = "file";

    /// A profile for `driver` (a [`Driver::id`](crate::Driver::id)) with a
    /// fresh id and the driver's conventional local defaults.
    pub fn new(driver: impl Into<Arc<str>>, port: u16) -> Self {
        Self {
            id: DataSourceId::generate(),
            name: "".into(),
            driver: driver.into(),
            host: "localhost".into(),
            port,
            database: "".into(),
            user: "".into(),
            ssl_mode: SslMode::default(),
            options: BTreeMap::new(),
        }
    }

    pub fn with_id(mut self, id: DataSourceId) -> Self {
        self.id = id;
        self
    }

    pub fn with_name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_host(mut self, host: impl Into<Arc<str>>) -> Self {
        self.host = host.into();
        self
    }

    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn with_database(mut self, database: impl Into<Arc<str>>) -> Self {
        self.database = database.into();
        self
    }

    pub fn with_user(mut self, user: impl Into<Arc<str>>) -> Self {
        self.user = user.into();
        self
    }

    pub fn with_ssl_mode(mut self, ssl_mode: SslMode) -> Self {
        self.ssl_mode = ssl_mode;
        self
    }

    /// Set the driver setting `name`; an empty value removes it.
    pub fn with_option(mut self, name: impl Into<Arc<str>>, value: impl Into<Arc<str>>) -> Self {
        let value = value.into();
        let name = name.into();
        if value.is_empty() {
            self.options.remove(&name);
        } else {
            self.options.insert(name, value);
        }
        self
    }

    pub fn id(&self) -> &DataSourceId {
        &self.id
    }

    /// The name the user gave the data source, or a name made from where it
    /// points when they gave none.
    pub fn name(&self) -> Arc<str> {
        if self.name.trim().is_empty() {
            self.address().into()
        } else {
            self.name.clone()
        }
    }

    /// The name exactly as the user typed it, possibly empty.
    pub fn given_name(&self) -> &str {
        &self.name
    }

    pub fn driver(&self) -> &str {
        &self.driver
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn user(&self) -> &str {
        &self.user
    }

    pub fn ssl_mode(&self) -> SslMode {
        self.ssl_mode
    }

    /// The driver setting `name`, if set.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(|value| &**value)
    }

    pub fn options(&self) -> &BTreeMap<Arc<str>, Arc<str>> {
        &self.options
    }

    /// `database@host:port`, or `host:port` without a database; a file
    /// database is its file.
    pub fn address(&self) -> String {
        if let Some(file) = self.option(Self::FILE) {
            return file.to_string();
        }
        if self.database.is_empty() {
            format!("{}:{}", self.host, self.port)
        } else {
            format!("{}@{}:{}", self.database, self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_without_a_name_is_called_by_its_address() {
        let profile = ConnectionProfile::new("postgresql", 5432).with_database("shop");
        assert_eq!(&*profile.name(), "shop@localhost:5432");

        let named = profile.with_name("Production");
        assert_eq!(&*named.name(), "Production");
    }

    #[test]
    fn profiles_round_trip_through_json_without_a_password() {
        let profile = ConnectionProfile::new("postgresql", 5432)
            .with_name("Local")
            .with_user("postgres")
            .with_ssl_mode(SslMode::VerifyFull);
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("password"));
        assert!(json.contains("\"verify-full\""));
        let restored: ConnectionProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, profile);
    }
}
