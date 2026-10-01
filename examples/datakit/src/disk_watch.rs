//! Hearing about changes other programs make to files DataKit shows.

use std::{path::PathBuf, time::Duration};

use futures::{StreamExt as _, channel::mpsc};
use gpui_kit::{Context, Task};
pub use notify::RecursiveMode;
use notify::Watcher as _;

/// Watches paths for as long as it is kept.
pub struct DiskWatch {
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
}

/// How long changes are gathered before they are reported: saving a file
/// is often several events.
const SETTLE: Duration = Duration::from_millis(200);

/// Call `on_change` with the paths that changed whenever something under
/// `paths` is created, modified or removed. Returns `None` when nothing
/// can be watched.
pub fn watch<T: 'static>(
    paths: &[(PathBuf, RecursiveMode)],
    cx: &mut Context<T>,
    on_change: impl Fn(&mut T, Vec<PathBuf>, &mut Context<T>) + 'static,
) -> Option<DiskWatch> {
    let (sender, mut receiver) = mpsc::unbounded::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event
            && !event.kind.is_access()
        {
            let _ = sender.unbounded_send(event.paths);
        }
    })
    .map_err(|error| tracing::warn!("cannot watch files: {error}"))
    .ok()?;
    let mut watching = false;
    for (path, mode) in paths {
        match watcher.watch(path, *mode) {
            Ok(()) => watching = true,
            Err(error) => tracing::warn!("cannot watch {}: {error}", path.display()),
        }
    }
    if !watching {
        return None;
    }
    let task = cx.spawn(async move |this, cx| {
        while let Some(mut changed) = receiver.next().await {
            cx.background_executor().timer(SETTLE).await;
            while let Ok(more) = receiver.try_recv() {
                changed.extend(more);
            }
            changed.sort();
            changed.dedup();
            if this
                .update(cx, |this, cx| on_change(this, changed, cx))
                .is_err()
            {
                return;
            }
        }
    });
    Some(DiskWatch {
        _watcher: watcher,
        _task: task,
    })
}
