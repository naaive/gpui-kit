//! Clipboard history: everything copied while the launcher runs, kept on disk
//! and searchable from the Clipboard History command.
//!
//! The launcher polls the clipboard instead of subscribing to it, because
//! GPUI has no clipboard notifications. Where the platform has a cheap change
//! counter (Windows), a poll that finds nothing new never reads the
//! clipboard. Content a password manager marks as private is never recorded.

mod history;
mod page;
mod platform;

pub use history::{Content, ContentType, Entry};
pub use page::clipboard_history_page;

use std::{path::PathBuf, time::Duration};

use gpui_kit::{
    App, AppContext as _, AsyncApp, ClipboardEntry, ClipboardItem, Context, Entity, Global,
    ImageFormat, Task,
};

use self::history::{History, image_path};
use crate::search::{now, write_snapshot};

/// How often the clipboard is checked.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Larger images are not kept; they are rarely wanted back and fill the disk.
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// The history and where it is saved. Pages observe this entity.
pub struct ClipboardStore {
    history: History,
    path: Option<PathBuf>,
    images: Option<PathBuf>,
    /// The platform's change counter at the last poll.
    change_count: Option<u32>,
    /// Reads of the current change that failed; after a few, the change is
    /// given up on rather than retried forever.
    failed_reads: u32,
    /// Changes before this are the launcher's own, not the user's copies.
    ignore_until: Option<std::time::Instant>,
    save_task: Option<Task<()>>,
}

struct GlobalClipboardStore(Entity<ClipboardStore>);

impl Global for GlobalClipboardStore {}

/// The history's directory under the launcher's data directory.
fn directory() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("clipboard"))
}

/// Loads the history and starts recording what is copied.
pub fn start(cx: &mut App) {
    let directory = directory();
    let path = directory.as_ref().map(|dir| dir.join("history.json"));
    let history = path
        .as_ref()
        .and_then(|path| match std::fs::read_to_string(path) {
            Ok(source) => History::from_json(&source)
                .map_err(|error| {
                    tracing::warn!("ignoring damaged clipboard history: {error}");
                })
                .ok(),
            Err(_) => None,
        })
        .unwrap_or_default();
    let store = cx.new(|_| ClipboardStore {
        history,
        path,
        images: directory.map(|dir| dir.join("images")),
        change_count: None,
        failed_reads: 0,
        ignore_until: None,
        save_task: None,
    });
    cx.set_global(GlobalClipboardStore(store.clone()));

    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(POLL_INTERVAL).await;
            cx.update(|cx| store.update(cx, |store, cx| store.poll(cx)));
        }
    })
    .detach();
}

/// The running history, if [`start`] was called.
pub fn store(cx: &App) -> Option<Entity<ClipboardStore>> {
    cx.try_global::<GlobalClipboardStore>()
        .map(|store| store.0.clone())
}

impl ClipboardStore {
    pub fn entries(&self) -> &[Entry] {
        self.history.entries()
    }

    fn poll(&mut self, cx: &mut Context<Self>) {
        let settings = crate::shell::launcher::settings(cx);
        if !settings.is_recording_clipboard() {
            return;
        }
        if let Some(days) = settings.clipboard_retention_days() {
            let cutoff = now().saturating_sub(u64::from(days) * 86_400);
            if self
                .history
                .entries()
                .iter()
                .any(|entry| !entry.is_pinned() && entry.copied_at() < cutoff)
            {
                remove_files(self.history.prune(cutoff));
                self.changed(cx);
            }
        }
        let count = platform::change_count();
        if count.is_some() && count == self.change_count {
            return;
        }
        if self
            .ignore_until
            .is_some_and(|until| std::time::Instant::now() < until)
        {
            self.change_count = count;
            return;
        }
        if platform::is_private() {
            self.change_count = count;
            return;
        }
        let source = platform::source_application();
        if source
            .as_deref()
            .is_some_and(|source| settings.is_clipboard_ignored(source))
        {
            self.change_count = count;
            return;
        }
        // The read fails while the copying application still holds the
        // clipboard open; the counter stays behind so the next poll retries.
        let Some(item) = cx.read_from_clipboard() else {
            self.failed_reads += 1;
            if self.failed_reads >= 4 {
                self.change_count = count;
                self.failed_reads = 0;
            }
            return;
        };
        self.change_count = count;
        self.failed_reads = 0;
        if let Some(content) = self.content(&item) {
            self.record(content, source, cx);
        }
    }

    /// What `item` holds, as history content: an image first, then copied
    /// files, then text. A new image is written to the image directory.
    fn content(&self, item: &ClipboardItem) -> Option<Content> {
        let image = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            ClipboardEntry::String(_) | ClipboardEntry::ExternalPaths(_) => None,
        });
        if let Some(image) = image {
            return self.save_image(&image.bytes, image.format);
        }
        let paths = item.entries().iter().find_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) if !paths.paths().is_empty() => {
                Some(paths.paths().to_vec())
            }
            ClipboardEntry::String(_)
            | ClipboardEntry::Image(_)
            | ClipboardEntry::ExternalPaths(_) => None,
        });
        if let Some(paths) = paths {
            return Some(Content::Files { paths });
        }
        let text = item.text()?;
        (!text.trim().is_empty()).then_some(Content::Text { text })
    }

    fn save_image(&self, bytes: &[u8], format: ImageFormat) -> Option<Content> {
        if bytes.len() > MAX_IMAGE_BYTES {
            return None;
        }
        let directory = self.images.as_ref()?;
        let path = image_path(directory, bytes, extension(format));
        let size = imagesize::blob_size(bytes).ok();
        let content = Content::Image {
            path: path.clone(),
            width: size.map_or(0, |size| size.width as u32),
            height: size.map_or(0, |size| size.height as u32),
            bytes: bytes.len() as u64,
        };
        if !self.history.is_latest(&content) && !path.is_file() {
            std::fs::create_dir_all(directory).ok()?;
            std::fs::write(&path, bytes)
                .map_err(|error| tracing::warn!("cannot save a copied image: {error}"))
                .ok()?;
        }
        Some(content)
    }

    fn record(&mut self, content: Content, source: Option<String>, cx: &mut Context<Self>) {
        if self.history.is_latest(&content) {
            return;
        }
        let image = match &content {
            Content::Image { path, .. } => Some(path.clone()),
            Content::Text { .. } | Content::Files { .. } => None,
        };
        let dropped = self.history.record_from(content, now(), source);
        remove_files(dropped);
        self.changed(cx);
        // The text in a new image is read in the background, for search.
        if let Some(path) = image
            && let Some(entry) = self.history.entries().first()
            && entry.recognized_text().is_none()
        {
            let id = entry.id().to_owned();
            cx.spawn(async move |this, cx| {
                let text = cx
                    .background_spawn(async move { crate::ocr::recognize(&path) })
                    .await;
                if let Some(text) = text {
                    this.update(cx, |store, cx| {
                        if store.history.set_recognized(&id, text) {
                            store.changed(cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
        }
    }

    /// Leaves clipboard changes for `duration` out of the history, while
    /// the launcher borrows the clipboard to paste something.
    pub fn ignore_changes_for(&mut self, duration: Duration) {
        self.ignore_until = Some(std::time::Instant::now() + duration);
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool, cx: &mut Context<Self>) {
        if self.history.set_pinned(id, pinned) {
            self.changed(cx);
        }
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(file) = self.history.remove(id) {
            remove_files(file);
            self.changed(cx);
        }
    }

    /// Removes every entry except the pinned ones.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        remove_files(self.history.clear());
        self.changed(cx);
    }

    /// Saves in the background, after any earlier save, and redraws pages.
    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(path) = self.path.clone() else {
            return;
        };
        let json = match self.history.to_json() {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!("cannot serialize the clipboard history: {error}");
                return;
            }
        };
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = write_snapshot(&path, &json) {
                tracing::warn!("cannot save the clipboard history: {error}");
            }
        }));
    }
}

fn remove_files(files: impl IntoIterator<Item = PathBuf>) {
    for file in files {
        std::fs::remove_file(&file).ok();
    }
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
    }
}

/// The clipboard item that puts `content` back on the clipboard.
pub fn clipboard_item(content: &Content) -> Option<ClipboardItem> {
    match content {
        Content::Text { text } => Some(ClipboardItem::new_string(text.clone())),
        Content::Image { path, .. } => {
            let bytes = std::fs::read(path).ok()?;
            let format = match path.extension().and_then(|e| e.to_str()) {
                Some("jpg") => ImageFormat::Jpeg,
                Some("webp") => ImageFormat::Webp,
                Some("gif") => ImageFormat::Gif,
                Some("svg") => ImageFormat::Svg,
                Some("bmp") => ImageFormat::Bmp,
                Some("tiff") => ImageFormat::Tiff,
                Some("ico") => ImageFormat::Ico,
                Some("pnm") => ImageFormat::Pnm,
                _ => ImageFormat::Png,
            };
            Some(ClipboardItem::new_image(&gpui_kit::Image::from_bytes(
                format, bytes,
            )))
        }
        Content::Files { paths } => Some(ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(gpui_kit::ExternalPaths(
                paths.clone().into(),
            ))],
        }),
    }
}
