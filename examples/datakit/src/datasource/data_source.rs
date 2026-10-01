use std::{collections::HashMap, sync::Arc, time::Duration};

use datakit_catalog::{Catalog, Role, Schema};
use datakit_driver::{Connection, ConnectionProfile, Dialect, StatementOutcome};
use datakit_driver_postgres::PostgresDialect;
use datakit_runtime::RemoteTask;
use futures::StreamExt as _;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, AppContext as _, Context, EventEmitter, Hsla, SharedString, Task};
use rust_i18n::t;

use super::{COLOR, READ_ONLY, describe_error, open_connection};
use crate::services::Services;

/// Something the catalog can be asked to read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CatalogRequest {
    /// The list of schemas and the search path.
    Schemas,
    /// Everything in one schema: relations, routines and sequences.
    Objects(Arc<str>),
}

/// The state of a data source's metadata connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed(SharedString),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataSourceEvent {
    /// The profile or the connection status changed.
    StatusChanged,
    /// The catalog, or what is being loaded into it, changed.
    CatalogChanged,
}

/// The color a data source is marked with, as DataGrip marks them, so a
/// production database looks different wherever its data appears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataSourceColor {
    Red,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
}

impl DataSourceColor {
    pub const ALL: [DataSourceColor; 6] = [
        DataSourceColor::Red,
        DataSourceColor::Yellow,
        DataSourceColor::Green,
        DataSourceColor::Cyan,
        DataSourceColor::Blue,
        DataSourceColor::Magenta,
    ];

    /// The name the profile keeps it under.
    pub fn as_str(self) -> &'static str {
        match self {
            DataSourceColor::Red => "red",
            DataSourceColor::Yellow => "yellow",
            DataSourceColor::Green => "green",
            DataSourceColor::Cyan => "cyan",
            DataSourceColor::Blue => "blue",
            DataSourceColor::Magenta => "magenta",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.as_str() == name)
    }

    pub fn title(self) -> SharedString {
        match self {
            DataSourceColor::Red => t!("datasource.color.red"),
            DataSourceColor::Yellow => t!("datasource.color.yellow"),
            DataSourceColor::Green => t!("datasource.color.green"),
            DataSourceColor::Cyan => t!("datasource.color.cyan"),
            DataSourceColor::Blue => t!("datasource.color.blue"),
            DataSourceColor::Magenta => t!("datasource.color.magenta"),
        }
        .into()
    }

    /// The theme's color of this name.
    pub fn hsla(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            DataSourceColor::Red => theme.red,
            DataSourceColor::Yellow => theme.yellow,
            DataSourceColor::Green => theme.green,
            DataSourceColor::Cyan => theme.cyan,
            DataSourceColor::Blue => theme.blue,
            DataSourceColor::Magenta => theme.magenta,
        }
    }
}

/// One configured database and what DataKit knows about it.
pub struct DataSource {
    profile: ConnectionProfile,
    dialect: Arc<dyn Dialect>,
    status: ConnectionStatus,
    connection: Option<Arc<dyn Connection>>,
    catalog: Catalog,
    /// Whether the catalog came from the cache and has not been read from
    /// the server since; connecting brings it up to date.
    stale: bool,
    /// Passwords given moments ago, used until the keychain has them, so a
    /// connection right after saving never reads the old ones.
    recent_password: Option<String>,
    recent_ssh_password: Option<String>,
    /// Requests waiting for the connection to open.
    pending: Vec<CatalogRequest>,
    in_flight: HashMap<CatalogRequest, Task<()>>,
    failures: HashMap<CatalogRequest, SharedString>,
    connect_task: Option<Task<()>>,
    cache_task: Option<Task<()>>,
    /// Bumped whenever the connection is replaced, so results from an old
    /// one are dropped instead of applied.
    generation: u64,
}

impl EventEmitter<DataSourceEvent> for DataSource {}

/// The options that decide how a profile connects: all but DataKit's own.
fn connection_options(profile: &ConnectionProfile) -> Vec<(&str, &str)> {
    profile
        .options()
        .iter()
        .map(|(name, value)| (&**name, &**value))
        .filter(|(name, _)| ![COLOR, READ_ONLY].contains(name))
        .collect()
}

impl DataSource {
    pub fn new(profile: ConnectionProfile, cx: &mut Context<Self>) -> Self {
        let mut data_source = Self {
            dialect: dialect_for(&profile, cx),
            catalog: Catalog::new(profile.database()),
            profile,
            status: ConnectionStatus::Disconnected,
            connection: None,
            stale: false,
            recent_password: None,
            recent_ssh_password: None,
            pending: Vec::new(),
            in_flight: HashMap::new(),
            failures: HashMap::new(),
            connect_task: None,
            cache_task: None,
            generation: 0,
        };
        data_source.load_cache(cx);
        data_source
    }

    pub fn profile(&self) -> &ConnectionProfile {
        &self.profile
    }

    pub fn name(&self) -> SharedString {
        self.profile.name().to_string().into()
    }

    pub fn dialect(&self) -> Arc<dyn Dialect> {
        self.dialect.clone()
    }

    /// The color the data source is marked with, if any.
    pub fn color(&self) -> Option<DataSourceColor> {
        self.profile.option(COLOR).and_then(DataSourceColor::parse)
    }

    /// The background of a toolbar or tab showing the data source's data:
    /// a wash of its color.
    pub fn tint(&self, cx: &App) -> Option<Hsla> {
        self.color().map(|color| color.hsla(cx).opacity(0.14))
    }

    /// Whether DataKit refuses to change the data source's data: consoles
    /// run only statements that read, and tables cannot be edited.
    pub fn is_read_only(&self) -> bool {
        self.profile.option(READ_ONLY) == Some("true")
    }

    pub fn status(&self) -> &ConnectionStatus {
        &self.status
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    #[cfg(test)]
    pub fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog = catalog;
    }

    /// The loaded schema `name`, if it is loaded.
    pub fn loaded_schema(&self, name: &str) -> Option<&Schema> {
        self.catalog
            .schema(name)
            .filter(|schema| schema.is_loaded())
    }

    pub fn is_loading(&self, request: &CatalogRequest) -> bool {
        self.in_flight.contains_key(request)
            || (self.pending.contains(request) && self.status == ConnectionStatus::Connecting)
    }

    pub fn failure(&self, request: &CatalogRequest) -> Option<&SharedString> {
        self.failures.get(request)
    }

    /// Open a new session of its own to this data source's database: what a
    /// console runs its statements in.
    pub fn open_session(&self, cx: &App) -> RemoteTask<Arc<dyn Connection>> {
        open_connection(
            &self.profile,
            self.recent_password.clone(),
            self.recent_ssh_password.clone(),
            cx,
        )
    }

    /// Run `statements` one after another in a session of their own,
    /// stopping at the first that fails. Rows a statement returns are read
    /// and discarded. A read-only data source refuses them.
    pub fn run_statements(&self, statements: Vec<String>, cx: &App) -> Task<anyhow::Result<()>> {
        if self.is_read_only() {
            return Task::ready(Err(anyhow::anyhow!(
                t!("datasource.read_only_refused", name = self.name()).to_string()
            )));
        }
        self.run_reading_statements(statements, cx)
    }

    /// Run `statements`, which leave the data as it is, as
    /// [`Self::run_statements`] does, even when the data source is read-only.
    pub fn run_reading_statements(
        &self,
        statements: Vec<String>,
        cx: &App,
    ) -> Task<anyhow::Result<()>> {
        let session = self.open_session(cx);
        let services = Services::global(cx);
        let work = services.spawn(async move {
            let connection = session.await?;
            for statement in statements {
                match connection.execute(statement.into()).await? {
                    StatementOutcome::Rows(mut rows) => {
                        while let Some(row) = rows.next().await {
                            row?;
                        }
                    }
                    StatementOutcome::Command(_) => {}
                }
            }
            Ok(())
        });
        cx.background_spawn(work)
    }

    /// Use these passwords for new connections until the keychain writes
    /// `stored` finish.
    pub fn hold_passwords(
        &mut self,
        password: Option<String>,
        ssh_password: Option<String>,
        stored: Task<()>,
        cx: &mut Context<Self>,
    ) {
        self.recent_password = password;
        self.recent_ssh_password = ssh_password;
        cx.spawn(async move |this, cx| {
            stored.await;
            let _ = this.update(cx, |this, _| {
                this.recent_password = None;
                this.recent_ssh_password = None;
            });
        })
        .detach();
    }

    /// Replace the profile. A change to where or how it connects closes the
    /// connection and forgets the catalog, which may describe another
    /// database now.
    pub fn set_profile(&mut self, profile: ConnectionProfile, cx: &mut Context<Self>) {
        let reconnect = self.profile.host() != profile.host()
            || self.profile.port() != profile.port()
            || self.profile.database() != profile.database()
            || self.profile.user() != profile.user()
            || self.profile.ssl_mode() != profile.ssl_mode()
            || self.profile.driver() != profile.driver()
            || connection_options(&self.profile) != connection_options(&profile);
        self.dialect = dialect_for(&profile, cx);
        self.profile = profile;
        if reconnect {
            self.disconnect(cx);
            self.catalog = Catalog::new(self.profile.database());
            self.failures.clear();
            self.save_cache(cx);
            cx.emit(DataSourceEvent::CatalogChanged);
        }
        cx.emit(DataSourceEvent::StatusChanged);
        cx.notify();
    }

    /// Open the metadata connection, then run whatever was waiting for it.
    pub fn connect(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.status,
            ConnectionStatus::Connecting | ConnectionStatus::Connected
        ) {
            return;
        }
        self.status = ConnectionStatus::Connecting;
        let generation = self.generation;
        let task = self.open_session(cx);
        self.connect_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.connect_task = None;
                match result {
                    Ok(connection) => {
                        this.connection = Some(connection);
                        this.status = ConnectionStatus::Connected;
                        if this.stale {
                            // What the cache showed is brought up to date.
                            this.stale = false;
                            for request in this.loaded_requests() {
                                if !this.pending.contains(&request) {
                                    this.pending.push(request);
                                }
                            }
                        }
                        for request in std::mem::take(&mut this.pending) {
                            this.start(request, cx);
                        }
                    }
                    Err(error) => {
                        this.status = ConnectionStatus::Failed(describe_error(&error));
                        this.pending.clear();
                        cx.emit(DataSourceEvent::CatalogChanged);
                    }
                }
                cx.emit(DataSourceEvent::StatusChanged);
                cx.notify();
            });
        }));
        cx.emit(DataSourceEvent::StatusChanged);
        cx.notify();
    }

    /// Close the metadata connection. The catalog stays, so the explorer and
    /// completion keep working from what was read.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.connection = None;
        self.connect_task = None;
        self.in_flight.clear();
        self.pending.clear();
        self.status = ConnectionStatus::Disconnected;
        cx.emit(DataSourceEvent::StatusChanged);
        cx.emit(DataSourceEvent::CatalogChanged);
        cx.notify();
    }

    /// Read `request` into the catalog, connecting first if needed. A request
    /// already running is not repeated.
    pub fn request(&mut self, request: CatalogRequest, cx: &mut Context<Self>) {
        if self.in_flight.contains_key(&request) {
            return;
        }
        let connected = self.status == ConnectionStatus::Connected
            && self.connection.as_ref().is_some_and(|c| !c.is_closed());
        if connected {
            self.start(request, cx);
            return;
        }
        if self.status == ConnectionStatus::Connected {
            // The server or the network ended the session; open a new one.
            self.status = ConnectionStatus::Disconnected;
            self.connection = None;
        }
        if !self.pending.contains(&request) {
            self.pending.push(request);
        }
        self.connect(cx);
        cx.emit(DataSourceEvent::CatalogChanged);
    }

    /// Like [`Self::request`], but does nothing for what is already loaded
    /// or already failed, so a caller that asks on every keystroke does not
    /// retry a failure forever.
    pub fn ensure(&mut self, request: CatalogRequest, cx: &mut Context<Self>) {
        let loaded = match &request {
            CatalogRequest::Schemas => self.catalog.has_schemas(),
            CatalogRequest::Objects(schema) => self.loaded_schema(schema).is_some(),
        };
        if loaded || self.failures.contains_key(&request) || self.is_loading(&request) {
            return;
        }
        if matches!(self.status, ConnectionStatus::Failed(_)) {
            return;
        }
        self.request(request, cx);
    }

    /// Read the schema list again, and every schema that was loaded.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.failures.clear();
        for request in self.loaded_requests() {
            self.request(request, cx);
        }
    }

    /// The requests that read again what the catalog holds.
    fn loaded_requests(&self) -> Vec<CatalogRequest> {
        std::iter::once(CatalogRequest::Schemas)
            .chain(
                self.catalog
                    .schemas()
                    .iter()
                    .filter(|schema| schema.is_loaded())
                    .map(|schema| CatalogRequest::Objects(schema.name())),
            )
            .collect()
    }

    fn start(&mut self, request: CatalogRequest, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let generation = self.generation;
        self.failures.remove(&request);
        let services = Services::global(cx);
        let task = match &request {
            CatalogRequest::Schemas => services.spawn(async move {
                let (schemas, search_path) =
                    futures::try_join!(connection.introspect_schemas(), connection.search_path())?;
                // Seeing the users and roles takes privileges many logins
                // lack; without them there are none to show.
                let roles = connection.introspect_roles().await.unwrap_or_default();
                Ok(CatalogUpdate::Schemas(schemas, search_path, roles))
            }),
            CatalogRequest::Objects(schema) => {
                let schema = schema.clone();
                services.spawn(async move {
                    let schema = connection.introspect_schema(schema).await?;
                    Ok(CatalogUpdate::Objects(schema))
                })
            }
        };
        let key = request.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.in_flight.remove(&key);
                match result {
                    Ok(CatalogUpdate::Schemas(schemas, search_path, roles)) => {
                        this.catalog = this
                            .catalog
                            .with_schemas(schemas)
                            .with_search_path(search_path)
                            .with_roles(roles);
                        this.save_cache(cx);
                    }
                    Ok(CatalogUpdate::Objects(schema)) => {
                        this.catalog = this.catalog.with_schema(schema);
                        this.save_cache(cx);
                    }
                    Err(error) => {
                        this.failures.insert(key, describe_error(&error));
                    }
                }
                cx.emit(DataSourceEvent::CatalogChanged);
                cx.notify();
            });
        });
        self.in_flight.insert(request, task);
        cx.emit(DataSourceEvent::CatalogChanged);
        cx.notify();
    }

    /// Show the catalog cached from the last run until the server is read.
    fn load_cache(&mut self, cx: &mut Context<Self>) {
        let cache = Services::global(cx).catalog_cache();
        let id = self.profile.id().clone();
        cx.spawn(async move |this, cx| {
            let cached = cx.background_spawn(async move { cache.load(&id) }).await;
            let Some(cached) = cached else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // Whatever the server said meanwhile is newer.
                if this.catalog.has_schemas() || cached.database() != this.profile.database() {
                    return;
                }
                this.catalog = cached;
                this.stale = true;
                cx.emit(DataSourceEvent::CatalogChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Write the catalog to the cache a moment after it stops changing.
    fn save_cache(&mut self, cx: &mut Context<Self>) {
        self.cache_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let Ok((cache, id, catalog)) = this.update(cx, |this, cx| {
                (
                    Services::global(cx).catalog_cache(),
                    this.profile.id().clone(),
                    this.catalog.clone(),
                )
            }) else {
                return;
            };
            let saved = cx
                .background_spawn(async move { cache.save(&id, &catalog) })
                .await;
            if let Err(error) = saved {
                tracing::warn!("{error:#}");
            }
        }));
    }
}

enum CatalogUpdate {
    Schemas(Vec<Schema>, Vec<Arc<str>>, Vec<Role>),
    Objects(Schema),
}

fn dialect_for(profile: &ConnectionProfile, cx: &App) -> Arc<dyn Dialect> {
    Services::global(cx)
        .drivers()
        .get(profile.driver())
        .map(|driver| driver.dialect())
        .unwrap_or_else(|| Arc::new(PostgresDialect))
}
