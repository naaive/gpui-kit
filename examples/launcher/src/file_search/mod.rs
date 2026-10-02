//! Search Files: finds files and folders in the home folder, or the folders
//! chosen in settings, by name, with a preview beside the results.
//!
//! The launcher walks those folders itself (skipping hidden folders and the
//! ones excluded in settings) instead of asking a platform index, so it works
//! the same everywhere. The walk runs in the background when the command
//! opens and the index is older than a few minutes or was built for other
//! settings; results show while it runs.

mod index;
mod page;

pub use page::{quick_look_path, search_files_page};

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};

use self::index::FileEntry;

/// An index older than this is walked again when the command opens.
const STALE_AFTER: Duration = Duration::from_secs(180);

pub struct FileIndex {
    files: Arc<Vec<FileEntry>>,
    indexed_at: Option<Instant>,
    /// What the files, or the walk under way, were gathered from.
    scope: Option<index::Scope>,
    task: Option<Task<()>>,
}

struct GlobalFileIndex(Entity<FileIndex>);

impl Global for GlobalFileIndex {}

fn file_index(cx: &mut App) -> Entity<FileIndex> {
    if let Some(index) = cx.try_global::<GlobalFileIndex>() {
        return index.0.clone();
    }
    let index = cx.new(|_| FileIndex {
        files: Arc::default(),
        indexed_at: None,
        scope: None,
        task: None,
    });
    cx.set_global(GlobalFileIndex(index.clone()));
    index
}

impl FileIndex {
    pub fn files(&self) -> &Arc<Vec<FileEntry>> {
        &self.files
    }

    pub fn is_indexing(&self) -> bool {
        self.task.is_some()
    }

    /// Walks the chosen folders again unless the index is fresh or a walk is
    /// already running. Folders or exclusions changed in settings make the
    /// index stale at once, and drop what was found under the old ones.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let settings = crate::shell::launcher::settings(cx);
        let scope = index::Scope::new(
            settings.file_search_roots(),
            settings.file_search_excluded(),
        );
        if self.scope.as_ref() != Some(&scope) {
            self.files = Arc::default();
            self.indexed_at = None;
            self.task = None;
        }
        let fresh = self
            .indexed_at
            .is_some_and(|indexed_at| indexed_at.elapsed() < STALE_AFTER);
        if fresh || self.task.is_some() {
            return;
        }
        self.scope = Some(scope.clone());
        let walk = cx.background_spawn(async move { index::walk(&scope) });
        self.task = Some(cx.spawn(async move |this, cx| {
            let files = walk.await;
            this.update(cx, |this, cx| {
                this.files = Arc::new(files);
                this.indexed_at = Some(Instant::now());
                this.task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}
