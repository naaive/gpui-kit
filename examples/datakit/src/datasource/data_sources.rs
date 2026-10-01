use std::sync::Arc;

use datakit_driver::{ConnectionProfile, DataSourceId};
use datakit_store::DataSourceFile;
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};

use super::{DataSource, describe_error, ssh_secret_id};
use crate::services::Services;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataSourcesEvent {
    /// A data source was added, removed or renamed.
    ListChanged,
}

/// Every configured data source, in the order they were added.
pub struct DataSources {
    items: Vec<Entity<DataSource>>,
    file: Arc<DataSourceFile>,
    /// Why the file could not be read; the list is empty and saving is
    /// refused, so the file is not overwritten.
    load_error: Option<SharedString>,
    save_task: Option<Task<()>>,
}

struct GlobalDataSources(Entity<DataSources>);

impl Global for GlobalDataSources {}

impl EventEmitter<DataSourcesEvent> for DataSources {}

impl DataSources {
    pub fn init(cx: &mut App) {
        let file = DataSourceFile::new(
            Services::global(cx)
                .data_directory()
                .join("data-sources.json"),
        );
        let (profiles, load_error) = match file.load() {
            Ok(profiles) => (profiles, None),
            Err(error) => {
                tracing::error!("{error:#}");
                (Vec::new(), Some(describe_error(&error)))
            }
        };
        let data_sources = cx.new(|cx| Self {
            items: profiles
                .into_iter()
                .map(|profile| cx.new(|cx| DataSource::new(profile, cx)))
                .collect(),
            file: Arc::new(file),
            load_error,
            save_task: None,
        });
        cx.set_global(GlobalDataSources(data_sources));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalDataSources>().0.clone()
    }

    pub fn items(&self) -> &[Entity<DataSource>] {
        &self.items
    }

    pub fn load_error(&self) -> Option<&SharedString> {
        self.load_error.as_ref()
    }

    pub fn get(&self, id: &DataSourceId, cx: &App) -> Option<Entity<DataSource>> {
        self.items
            .iter()
            .find(|item| item.read(cx).profile().id() == id)
            .cloned()
    }

    /// Add a data source, storing the passwords given in the keychain.
    pub fn add(
        &mut self,
        profile: ConnectionProfile,
        password: Option<String>,
        ssh_password: Option<String>,
        cx: &mut Context<Self>,
    ) -> Entity<DataSource> {
        let stored = store_passwords(&profile, password.clone(), ssh_password.clone(), cx);
        let item = cx.new(|cx| DataSource::new(profile, cx));
        item.update(cx, |item, cx| {
            item.hold_passwords(password, ssh_password, stored, cx)
        });
        self.items.push(item.clone());
        self.save(cx);
        cx.emit(DataSourcesEvent::ListChanged);
        cx.notify();
        item
    }

    /// Replace the profile of the data source with the same id. A password
    /// of `None` keeps the stored one.
    pub fn update(
        &mut self,
        profile: ConnectionProfile,
        password: Option<String>,
        ssh_password: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.get(profile.id(), cx) else {
            return;
        };
        let password_changed = password.is_some() || ssh_password.is_some();
        let stored = store_passwords(&profile, password.clone(), ssh_password.clone(), cx);
        item.update(cx, |item, cx| {
            if password_changed {
                item.hold_passwords(password, ssh_password, stored, cx);
            }
            item.set_profile(profile, cx);
            if password_changed && item.status() != &super::ConnectionStatus::Disconnected {
                item.disconnect(cx);
            }
        });
        self.save(cx);
        cx.emit(DataSourcesEvent::ListChanged);
        cx.notify();
    }

    /// Remove a data source and forget its password.
    pub fn remove(&mut self, id: &DataSourceId, cx: &mut Context<Self>) {
        self.items.retain(|item| item.read(cx).profile().id() != id);
        let secrets = Services::global(cx).secrets();
        let cache = Services::global(cx).catalog_cache();
        let id = id.clone();
        cx.background_spawn(async move {
            for secret in [id.clone(), ssh_secret_id(&id)] {
                if let Err(error) = secrets.delete(&secret) {
                    tracing::warn!("{error:#}");
                }
            }
            if let Err(error) = cache.remove(&id) {
                tracing::warn!("{error:#}");
            }
        })
        .detach();
        self.save(cx);
        cx.emit(DataSourcesEvent::ListChanged);
        cx.notify();
    }

    /// Write the list, after any write still in progress.
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.load_error.is_some() {
            tracing::warn!("not saving data sources: the file could not be read");
            return;
        }
        let profiles: Vec<ConnectionProfile> = self
            .items
            .iter()
            .map(|item| item.read(cx).profile().clone())
            .collect();
        let file = self.file.clone();
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = file.save(&profiles) {
                tracing::error!("{error:#}");
            }
        }));
    }
}

/// Write the passwords given to the keychain, removing a stored one when
/// its new value is empty; the task finishes when the keychain has them.
fn store_passwords(
    profile: &ConnectionProfile,
    password: Option<String>,
    ssh_password: Option<String>,
    cx: &App,
) -> Task<()> {
    let secrets = Services::global(cx).secrets();
    let id = profile.id().clone();
    cx.background_spawn(async move {
        for (id, password) in [(ssh_secret_id(&id), ssh_password), (id, password)] {
            let Some(password) = password else {
                continue;
            };
            let result = if password.is_empty() {
                secrets.delete(&id)
            } else {
                secrets.write(&id, &password)
            };
            if let Err(error) = result {
                tracing::error!("{error:#}");
            }
        }
    })
}
