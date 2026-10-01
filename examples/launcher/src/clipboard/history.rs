//! What was copied, newest first, as plain data that is saved as JSON.

use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// How many entries are kept; pinned entries do not count and are never
/// dropped.
pub const MAX_ENTRIES: usize = 1000;
const VERSION: u32 = 1;

/// One thing that was on the clipboard.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Content {
    Text {
        text: String,
    },
    /// An image saved in the history's image directory.
    Image {
        path: PathBuf,
        width: u32,
        height: u32,
        bytes: u64,
    },
    /// Files copied in a file manager.
    Files {
        paths: Vec<PathBuf>,
    },
}

impl Content {
    /// A stable identity: the same content copied again is the same entry.
    fn fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        match self {
            Self::Text { text } => ("text", text).hash(&mut hasher),
            // Image files are named after their bytes' hash already.
            Self::Image { path, .. } => ("image", path).hash(&mut hasher),
            Self::Files { paths } => ("files", paths).hash(&mut hasher),
        }
        format!("{:016x}", hasher.finish())
    }

    pub fn kind(&self) -> ContentType {
        match self {
            Self::Text { text } if is_link(text) => ContentType::Link,
            Self::Text { text } if is_color(text) => ContentType::Color,
            Self::Text { .. } => ContentType::Text,
            Self::Image { .. } => ContentType::Image,
            Self::Files { .. } => ContentType::File,
        }
    }

    /// What a search matches.
    pub fn searchable_text(&self) -> String {
        match self {
            Self::Text { text } => text.clone(),
            Self::Image { width, height, .. } => format!("Image {width}×{height}"),
            Self::Files { paths } => paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// What kind of content an entry holds, for its icon and the type filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentType {
    Text,
    Link,
    Color,
    Image,
    File,
}

impl ContentType {
    pub const ALL: [Self; 5] = [Self::Text, Self::Link, Self::Color, Self::Image, Self::File];

    pub fn value(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Link => "link",
            Self::Color => "color",
            Self::Image => "image",
            Self::File => "file",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.value() == value)
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Link => "Link",
            Self::Color => "Color",
            Self::Image => "Image",
            Self::File => "File",
        }
    }

    pub fn plural_title(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Link => "Links",
            Self::Color => "Colors",
            Self::Image => "Images",
            Self::File => "Files",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Text => "type",
            Self::Link => "link",
            Self::Color => "palette",
            Self::Image => "image",
            Self::File => "file",
        }
    }
}

fn is_link(text: &str) -> bool {
    let text = text.trim();
    !text.contains(char::is_whitespace)
        && url::Url::parse(text)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "mailto" | "ftp"))
}

/// `#rgb`, `#rrggbb` or `#rrggbbaa`.
fn is_color(text: &str) -> bool {
    let Some(hex) = text.trim().strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Entry {
    id: String,
    content: Content,
    /// Unix seconds of the latest copy.
    copied_at: u64,
    /// How many times it was copied.
    #[serde(default = "one")]
    copies: u32,
    #[serde(default)]
    pinned: bool,
    /// The application it was copied from, by its executable's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// The text recognized in an image, for search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recognized: Option<String>,
}

fn one() -> u32 {
    1
}

impl Entry {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn content(&self) -> &Content {
        &self.content
    }

    pub fn copied_at(&self) -> u64 {
        self.copied_at
    }

    pub fn copies(&self) -> u32 {
        self.copies
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn recognized_text(&self) -> Option<&str> {
        self.recognized.as_deref()
    }

    /// What a search matches: the content, the text in an image and the
    /// application it came from.
    pub fn searchable_text(&self) -> String {
        let mut text = self.content.searchable_text();
        for extra in [&self.recognized, &self.source].into_iter().flatten() {
            text.push('\n');
            text.push_str(extra);
        }
        text
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct HistoryFile {
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

/// The clipboard history: entries newest first, pinned or not.
#[derive(Debug, Default)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    pub fn from_json(source: &str) -> serde_json::Result<Self> {
        let file: HistoryFile = serde_json::from_str(source)?;
        Ok(Self {
            entries: file.entries,
        })
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(&HistoryFile {
            version: VERSION,
            entries: self.entries.clone(),
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    #[cfg(test)]
    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Records a copy. Content already in the history moves to the top
    /// instead of appearing twice. Returns the files of entries that fell
    /// off the end, for the caller to delete.
    #[cfg(test)]
    pub fn record(&mut self, content: Content, now: u64) -> Vec<PathBuf> {
        self.record_from(content, now, None)
    }

    /// Records a copy from the application `source`.
    pub fn record_from(
        &mut self,
        content: Content,
        now: u64,
        source: Option<String>,
    ) -> Vec<PathBuf> {
        let id = content.fingerprint();
        let entry = match self.entries.iter().position(|entry| entry.id == id) {
            Some(ix) => {
                let mut entry = self.entries.remove(ix);
                entry.copied_at = now;
                entry.copies = entry.copies.saturating_add(1);
                if source.is_some() {
                    entry.source = source;
                }
                entry
            }
            None => Entry {
                id,
                content,
                copied_at: now,
                copies: 1,
                pinned: false,
                source,
                recognized: None,
            },
        };
        self.entries.insert(0, entry);
        self.trim()
    }

    /// Whether `content` is what was recorded last, so polling the same
    /// clipboard again records nothing.
    pub fn is_latest(&self, content: &Content) -> bool {
        self.entries
            .first()
            .is_some_and(|entry| entry.id == content.fingerprint())
    }

    /// Keeps the text recognized in the image entry `id`.
    pub fn set_recognized(&mut self, id: &str, text: String) -> bool {
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) => {
                entry.recognized = Some(text);
                true
            }
            None => false,
        }
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        match self.entries.iter_mut().find(|entry| entry.id == id) {
            Some(entry) if entry.pinned != pinned => {
                entry.pinned = pinned;
                true
            }
            _ => false,
        }
    }

    /// Removes one entry; returns its file, if it had one, to delete.
    pub fn remove(&mut self, id: &str) -> Option<Option<PathBuf>> {
        let ix = self.entries.iter().position(|entry| entry.id == id)?;
        Some(image_file(&self.entries.remove(ix)))
    }

    /// Removes entries copied before `cutoff` (Unix seconds), except pinned
    /// ones; returns their files.
    pub fn prune(&mut self, cutoff: u64) -> Vec<PathBuf> {
        let (kept, removed): (Vec<_>, Vec<_>) = self
            .entries
            .drain(..)
            .partition(|entry| entry.pinned || entry.copied_at >= cutoff);
        self.entries = kept;
        removed.iter().filter_map(image_file).collect()
    }

    /// Removes every entry that is not pinned; returns their files.
    pub fn clear(&mut self) -> Vec<PathBuf> {
        let (kept, removed): (Vec<_>, Vec<_>) =
            self.entries.drain(..).partition(|entry| entry.pinned);
        self.entries = kept;
        removed.iter().filter_map(image_file).collect()
    }

    fn trim(&mut self) -> Vec<PathBuf> {
        let mut unpinned = 0;
        let mut dropped = Vec::new();
        self.entries.retain(|entry| {
            if entry.pinned {
                return true;
            }
            unpinned += 1;
            let keep = unpinned <= MAX_ENTRIES;
            if !keep {
                dropped.extend(image_file(entry));
            }
            keep
        });
        dropped
    }
}

fn image_file(entry: &Entry) -> Option<PathBuf> {
    match &entry.content {
        Content::Image { path, .. } => Some(path.clone()),
        Content::Text { .. } | Content::Files { .. } => None,
    }
}

/// The file an image with these bytes is saved as in `directory`; named by
/// the bytes' hash, so copying the same image twice saves it once.
pub fn image_path(directory: &Path, bytes: &[u8], extension: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    directory.join(format!("{:016x}.{extension}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Content {
        Content::Text { text: text.into() }
    }

    #[test]
    fn test_record_moves_repeated_content_to_the_top() {
        let mut history = History::default();
        history.record(text("a"), 1);
        history.record(text("b"), 2);
        history.record(text("a"), 3);

        let entries = history.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].content(), &text("a"));
        assert_eq!(entries[0].copied_at(), 3);
        assert_eq!(entries[0].copies(), 2);
        assert!(history.is_latest(&text("a")));
        assert!(!history.is_latest(&text("b")));
    }

    #[test]
    fn test_pinned_entries_survive_clear_and_trim() {
        let mut history = History::default();
        history.record(text("keep"), 0);
        let keep = history.entries()[0].id().to_owned();
        assert!(history.set_pinned(&keep, true));
        for ix in 0..MAX_ENTRIES + 5 {
            history.record(text(&ix.to_string()), ix as u64 + 1);
        }
        assert_eq!(history.entries().len(), MAX_ENTRIES + 1);
        assert!(history.get(&keep).is_some());

        // The oldest unpinned entries left were copied at 6, 7, 8 and 9.
        assert!(history.prune(10).is_empty(), "text entries have no files");
        assert_eq!(history.entries().len(), MAX_ENTRIES + 1 - 4);
        assert!(
            history.get(&keep).is_some(),
            "pinned entries are not pruned"
        );
        history.clear();
        assert_eq!(history.entries().len(), 1);
        assert!(history.entries()[0].is_pinned());
    }

    #[test]
    fn test_content_types_and_round_trip() {
        assert_eq!(text("https://raycast.com").kind(), ContentType::Link);
        assert_eq!(text("see https://raycast.com").kind(), ContentType::Text);
        assert_eq!(text("#ff8800").kind(), ContentType::Color);
        assert_eq!(text("#ff88").kind(), ContentType::Text);

        let mut history = History::default();
        history.record(text("hello"), 5);
        history.record(
            Content::Image {
                path: "a.png".into(),
                width: 2,
                height: 3,
                bytes: 10,
            },
            6,
        );
        let restored = History::from_json(&history.to_json().unwrap()).unwrap();
        assert_eq!(restored.entries(), history.entries());
        assert_eq!(
            history.remove(restored.entries()[0].id()),
            Some(Some(PathBuf::from("a.png")))
        );
    }
}
