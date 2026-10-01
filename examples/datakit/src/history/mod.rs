//! Query history: every statement run, searchable, one double-click from
//! running again.

mod history_panel;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use datakit_store::{HistoryEntry, QueryHistory};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, SharedString, Task};

pub use history_panel::{HistoryPanel, HistoryPanelEvent};

use crate::{datasource::describe_error, services::Services};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryEvent {
    /// An entry was added or the history was cleared.
    Changed,
}

/// The query history, kept in SQLite in the data directory.
pub struct HistoryLog {
    store: Option<Arc<Mutex<QueryHistory>>>,
    open_error: Option<SharedString>,
    write_task: Option<Task<()>>,
}

struct GlobalHistory(Entity<HistoryLog>);

impl Global for GlobalHistory {}

impl EventEmitter<HistoryEvent> for HistoryLog {}

impl HistoryLog {
    pub fn init(cx: &mut App) {
        let path = Services::global(cx)
            .data_directory()
            .join("history.sqlite3");
        let (store, open_error) = match QueryHistory::open(&path) {
            Ok(store) => (Some(Arc::new(Mutex::new(store))), None),
            Err(error) => {
                tracing::error!("{error:#}");
                (None, Some(describe_error(&error)))
            }
        };
        let log = cx.new(|_| Self {
            store,
            open_error,
            write_task: None,
        });
        cx.set_global(GlobalHistory(log));
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalHistory>().0.clone()
    }

    /// Why the history could not be opened; nothing is recorded then.
    pub fn open_error(&self) -> Option<&SharedString> {
        self.open_error.as_ref()
    }

    /// Record `entry`, after any write still in progress.
    pub fn record(&mut self, entry: HistoryEntry, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let previous = self.write_task.take();
        self.write_task = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let written = cx
                .background_spawn(async move { lock(&store).record(entry) })
                .await;
            if let Err(error) = written {
                tracing::error!("couldn’t record a statement in the history: {error:#}");
                return;
            }
            let _ = this.update(cx, |_, cx| cx.emit(HistoryEvent::Changed));
        }));
    }

    /// The newest `limit` entries whose SQL contains `filter`.
    pub fn search(
        &self,
        filter: String,
        limit: usize,
        cx: &App,
    ) -> Task<Result<Vec<HistoryEntry>>> {
        let Some(store) = self.store.clone() else {
            return Task::ready(Ok(Vec::new()));
        };
        cx.background_spawn(async move { lock(&store).recent(None, &filter, limit) })
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let previous = self.write_task.take();
        self.write_task = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let cleared = cx
                .background_spawn(async move { lock(&store).clear(None) })
                .await;
            if let Err(error) = cleared {
                tracing::error!("couldn’t clear the history: {error:#}");
            }
            let _ = this.update(cx, |_, cx| cx.emit(HistoryEvent::Changed));
        }));
    }
}

fn lock(store: &Mutex<QueryHistory>) -> std::sync::MutexGuard<'_, QueryHistory> {
    store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
