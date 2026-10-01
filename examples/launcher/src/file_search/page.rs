//! The Search Files page: recent files with nothing typed, matches by name
//! otherwise, and a preview of the selected one: an image, the start of a
//! text file, a folder's contents, or the system's thumbnail of anything
//! else (a PDF's first page, a video's frame). Quick Look shows the preview
//! on a page of its own.

use std::{
    collections::HashMap,
    io::Read as _,
    path::{Path, PathBuf},
};

use anyhow::Result;
use gpui_kit::{
    App, AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity, ExternalPaths,
    SharedString, Subscription, Task, Window,
};

use super::{FileIndex, file_index, index};
use crate::{
    format::{code_block_in, format_bytes, format_time},
    model::{
        Accessory, Action, ActionPanel, Choice, DetailModel, Dropdown, Effect, Image, Item, ItemId,
        ListModel, Metadata, MetadataValue, PageModel, PushHandler, Section, TextHandler,
    },
    pages::{self, Page, PageHandle},
    sources::applications::{REVEAL_TITLE, file_thumbnail},
};

const RESULTS: usize = 100;
const RECENT: usize = 30;
/// The text preview reads at most this much of a file.
const PREVIEW_BYTES: usize = 16 * 1024;
const PREVIEW_LINES: usize = 80;
/// Larger images are not previewed; decoding them would stall the list.
const PREVIEW_IMAGE_BYTES: u64 = 25 * 1024 * 1024;
/// Quick Look reads more of a text file than the side panel.
const QUICK_LOOK_BYTES: usize = 256 * 1024;
const QUICK_LOOK_LINES: usize = 2000;
/// A folder's preview lists at most this many of its entries.
const FOLDER_ENTRIES: usize = 60;

pub fn search_files_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let index = file_index(cx);
    index.update(cx, |index, cx| index.refresh(cx));
    Ok(pages::handle(cx.new(|cx| SearchFilesPage {
        _subscription: cx.observe(&index, |page: &mut SearchFilesPage, _, cx| {
            page.invalidate(cx)
        }),
        index,
        query: String::new(),
        selected: None,
        filter: None,
        list: None,
        thumbnails: HashMap::new(),
        thumbnail_task: None,
    })))
}

struct SearchFilesPage {
    index: Entity<FileIndex>,
    query: String,
    /// The selected file's path; only it gets a content preview.
    selected: Option<SharedString>,
    /// The kind of file shown, or every kind.
    filter: Option<Group>,
    list: Option<ListModel>,
    /// The system's thumbnails of files previewed so far.
    thumbnails: HashMap<PathBuf, Option<PathBuf>>,
    thumbnail_task: Option<Task<()>>,
    _subscription: Subscription,
}

impl SearchFilesPage {
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.list = None;
        cx.notify();
    }

    /// Draws the thumbnail of `path` in the background, then the list again.
    fn request_thumbnail(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.thumbnail_task = Some(cx.spawn(async move |this, cx| {
            let thumbnail = {
                let path = path.clone();
                cx.background_spawn(async move { file_thumbnail(&path) })
                    .await
            };
            this.update(cx, |page, cx| {
                page.thumbnails.insert(path, thumbnail);
                page.invalidate(cx);
            })
            .ok();
        }));
    }

    fn build_list(&mut self, cx: &mut Context<Self>) -> ListModel {
        let page = cx.entity().downgrade();
        let index = self.index.read(cx);
        let files = index.files().clone();
        let query = self.query.trim().to_owned();
        let filter = self.filter;
        let keep = |file: &index::FileEntry| {
            filter.is_none_or(|group| Some(group) == Group::of(&file.path, file.is_dir))
        };
        let (title, found) = match query.is_empty() {
            true => ("Recent Files", index::recent(&files, RECENT, keep)),
            false => ("Files", index::search(&files, &query, RESULTS, keep)),
        };
        // The first row is selected until the user picks another.
        let selected = self
            .selected
            .clone()
            .filter(|selected| {
                found
                    .iter()
                    .any(|file| file.path.to_string_lossy() == **selected)
            })
            .or_else(|| {
                found
                    .first()
                    .map(|file| file.path.to_string_lossy().into_owned().into())
            });
        let wanted = found
            .iter()
            .find(|file| Some(file.path.to_string_lossy().as_ref()) == selected.as_deref())
            .filter(|file| needs_thumbnail(file))
            .map(|file| file.path.clone())
            .filter(|path| !self.thumbnails.contains_key(path));
        let items: Vec<Item> = found
            .iter()
            .map(|file| {
                let id: SharedString = file.path.to_string_lossy().into_owned().into();
                let preview = selected.as_ref() == Some(&id);
                let thumbnail = self.thumbnails.get(&file.path).cloned().flatten();
                item(file, preview, thumbnail)
            })
            .collect();
        let indexing = index.is_indexing();
        if let Some(path) = wanted {
            self.request_thumbnail(path, cx);
        }
        let filter_page = cx.entity().downgrade();
        let dropdown = Group::ALL.into_iter().fold(
            Dropdown::new("Kind")
                .with_choice(Choice::new("all", "All Kinds"))
                .with_value(self.filter.map_or("all", Group::value))
                .with_on_change(TextHandler::new(move |value, _, cx| {
                    filter_page
                        .update(cx, |page, cx| {
                            page.filter = Group::parse(&value);
                            page.invalidate(cx);
                        })
                        .ok();
                })),
            |dropdown, group| dropdown.with_choice(Choice::new(group.value(), group.title())),
        );
        ListModel::new()
            .with_dropdown(dropdown)
            .with_placeholder("Search files by name…")
            .with_filtering(false)
            .with_showing_detail(true)
            .with_loading(indexing)
            .with_empty_title(match (indexing, query.is_empty()) {
                (true, _) => "Indexing your files…",
                (false, true) => "No recent files",
                (false, false) => "No files found",
            })
            .with_on_selection_change(TextHandler::new(move |id, _, cx| {
                page.update(cx, |page, cx| {
                    if page.selected.as_ref() != Some(&id) {
                        page.selected = Some(id);
                        page.invalidate(cx);
                    }
                })
                .ok();
            }))
            .with_section(Section::new().with_title(title).with_items(items))
    }
}

impl Page for SearchFilesPage {
    fn title(&self) -> SharedString {
        "Search Files".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let list = match self.list.take() {
            Some(list) => list,
            None => self.build_list(cx),
        };
        self.list = Some(list.clone());
        PageModel::List(list)
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.selected = None;
            self.invalidate(cx);
        }
    }
}

/// The kinds the list can be narrowed to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Group {
    Folders,
    Documents,
    Images,
    Code,
    Media,
    Archives,
}

impl Group {
    const ALL: [Self; 6] = [
        Self::Folders,
        Self::Documents,
        Self::Images,
        Self::Code,
        Self::Media,
        Self::Archives,
    ];

    fn value(self) -> &'static str {
        match self {
            Self::Folders => "folders",
            Self::Documents => "documents",
            Self::Images => "images",
            Self::Code => "code",
            Self::Media => "media",
            Self::Archives => "archives",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Folders => "Folders",
            Self::Documents => "Documents",
            Self::Images => "Images",
            Self::Code => "Code",
            Self::Media => "Audio & Video",
            Self::Archives => "Archives",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.value() == value)
    }

    /// The group a file falls in; files of no group (`.exe`, unknown
    /// extensions) only show under All Kinds.
    fn of(path: &Path, is_dir: bool) -> Option<Self> {
        let (_, icon, _) = kind(path, is_dir);
        Some(match icon {
            "folder" => Self::Folders,
            "file-image" => Self::Images,
            "file-code" => Self::Code,
            "file-music" | "file-video-camera" => Self::Media,
            "file-archive" => Self::Archives,
            "file-text" | "file-spreadsheet" => Self::Documents,
            _ => return None,
        })
    }
}

/// What kind of file a path is, by its extension: an icon and, for text, the
/// language to highlight it as.
fn kind(path: &Path, is_dir: bool) -> (&'static str, &'static str, Option<&'static str>) {
    if is_dir {
        return ("Folder", "folder", None);
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tiff" => {
            ("Image", "file-image", None)
        }
        "mp3" | "wav" | "flac" | "m4a" | "ogg" | "aac" => ("Audio", "file-music", None),
        "mp4" | "mov" | "mkv" | "avi" | "webm" => ("Video", "file-video-camera", None),
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" => ("Archive", "file-archive", None),
        "xlsx" | "xls" | "csv" | "ods" | "numbers" => ("Spreadsheet", "file-spreadsheet", None),
        "pdf" | "doc" | "docx" | "pptx" | "ppt" | "key" | "pages" => {
            ("Document", "file-text", None)
        }
        "md" | "markdown" => ("Markdown", "file-text", Some("markdown")),
        "txt" | "log" | "ini" | "cfg" | "conf" => ("Text", "file-text", Some("text")),
        "rs" => ("Rust", "file-code", Some("rust")),
        "py" => ("Python", "file-code", Some("python")),
        "js" | "mjs" | "cjs" => ("JavaScript", "file-code", Some("javascript")),
        "ts" | "tsx" => ("TypeScript", "file-code", Some("typescript")),
        "json" => ("JSON", "file-code", Some("json")),
        "toml" => ("TOML", "file-code", Some("toml")),
        "yaml" | "yml" => ("YAML", "file-code", Some("yaml")),
        "html" | "htm" => ("HTML", "file-code", Some("html")),
        "css" => ("CSS", "file-code", Some("css")),
        "c" | "h" => ("C", "file-code", Some("c")),
        "cpp" | "hpp" | "cc" => ("C++", "file-code", Some("cpp")),
        "go" => ("Go", "file-code", Some("go")),
        "java" => ("Java", "file-code", Some("java")),
        "sh" | "bash" | "zsh" | "ps1" => ("Script", "file-code", Some("bash")),
        _ => ("File", "file", None),
    }
}

/// Whether the preview of `file` is the system's thumbnail: it is not a
/// folder, a text file, or an image small enough to show itself.
fn needs_thumbnail(file: &index::FileEntry) -> bool {
    let (kind_title, _, language) = kind(&file.path, file.is_dir);
    !file.is_dir
        && language.is_none()
        && !(kind_title == "Image" && file.size <= PREVIEW_IMAGE_BYTES)
}

fn item(file: &index::FileEntry, preview: bool, thumbnail: Option<PathBuf>) -> Item {
    let (kind_title, icon, language) = kind(&file.path, file.is_dir);
    let parent = file
        .path
        .parent()
        .map(|parent| home_relative(parent))
        .unwrap_or_default();
    let detail = detail(file, kind_title, language, preview, thumbnail.clone());
    let quick_look = {
        let (file, thumbnail) = (file.clone(), thumbnail);
        PushHandler::new(move |_, cx| Ok(quick_look_page(&file, thumbnail.clone(), cx)))
    };
    let copy_file = ClipboardItem {
        entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
            vec![file.path.clone()].into(),
        ))],
    };
    Item::new(
        ItemId::new(file.path.to_string_lossy().into_owned()),
        file.name(),
    )
    .with_subtitle(parent)
    .with_image(Image::Icon(icon.into()))
    .with_accessory(Accessory::text(match file.is_dir {
        true => String::new(),
        false => format_bytes(file.size),
    }))
    .with_detail(detail)
    .with_actions(open_with(
        ActionPanel::new()
            .with_action(
                Action::new("Open", Effect::OpenPath(file.path.clone()))
                    .with_image(Image::Icon("external-link".into())),
            )
            .with_action(
                Action::new(REVEAL_TITLE, Effect::RevealPath(file.path.clone()))
                    .with_image(Image::Icon("folder-open".into())),
            )
            .with_action(
                Action::new("Quick Look", Effect::Push(quick_look))
                    .with_image(Image::Icon("eye".into()))
                    .with_shortcut("secondary-y"),
            )
            .with_action(
                Action::new(
                    "Copy Path",
                    Effect::Copy(file.path.display().to_string().into()),
                )
                .with_image(Image::Icon("copy".into()))
                .with_shortcut("secondary-shift-c"),
            )
            .with_action(
                Action::new("Copy File", Effect::CopyItem(copy_file))
                    .with_image(Image::Icon("files".into()))
                    .with_shortcut("secondary-shift-."),
            )
            .with_action(
                Action::new("Copy Name", Effect::Copy(file.name().into()))
                    .with_image(Image::Icon("type".into())),
            ),
        &file.path,
        file.is_dir,
    ))
}

/// Adds Open With… where the system has a chooser for it (Windows).
fn open_with(actions: ActionPanel, path: &Path, is_dir: bool) -> ActionPanel {
    if !cfg!(target_os = "windows") || is_dir {
        return actions;
    }
    let path = path.to_path_buf();
    actions.with_action(
        Action::new(
            "Open With…",
            crate::model::Effect::Run(crate::model::RunHandler::new(move |(), _, cx| {
                crate::shell::launcher::hide(cx);
                show_open_with(path.clone());
            })),
        )
        .with_image(Image::Icon("app-window".into()))
        .with_shortcut("secondary-shift-o"),
    )
}

/// The system's Open With dialog for `path`. It is modal, so it gets a
/// thread of its own.
#[cfg(target_os = "windows")]
fn show_open_with(path: std::path::PathBuf) {
    std::thread::spawn(move || {
        use windows::{
            Win32::{
                Foundation::HWND,
                System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
                UI::Shell::{
                    OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO,
                    SHOpenWithDialog,
                },
            },
            core::{HSTRING, PCWSTR},
        };
        let file = HSTRING::from(path.as_os_str());
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            if let Err(error) = SHOpenWithDialog(HWND::default(), &info) {
                tracing::warn!("cannot show Open With for {}: {error}", path.display());
            }
            if initialized {
                CoUninitialize();
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn show_open_with(_: std::path::PathBuf) {}

/// The path with the home folder written `~`.
fn home_relative(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(relative) if relative.as_os_str().is_empty() => "~".into(),
        Some(relative) => format!("~{}{}", std::path::MAIN_SEPARATOR, relative.display()),
        None => path.display().to_string(),
    }
}

fn detail(
    file: &index::FileEntry,
    kind_title: &str,
    language: Option<&str>,
    preview: bool,
    thumbnail: Option<PathBuf>,
) -> DetailModel {
    let detail = match preview {
        true => preview_of(file, kind_title, language, thumbnail, false),
        false => DetailModel::new(""),
    };
    let label =
        |label: &str, value: String| Metadata::new(label, MetadataValue::Text(value.into()));
    let detail = detail
        .with_metadata(label("Name", file.name()))
        .with_metadata(label("Kind", kind_title.to_owned()));
    let detail = match file.is_dir {
        true => detail,
        false => detail.with_metadata(label("Size", format_bytes(file.size))),
    };
    detail
        .with_metadata(label("Modified", format_time(file.modified)))
        .with_metadata(label(
            "Where",
            file.path.parent().map(home_relative).unwrap_or_default(),
        ))
}

/// What the preview of `file` shows: the image, the start of the text, the
/// folder's entries or the thumbnail. `full` reads more, for Quick Look.
fn preview_of(
    file: &index::FileEntry,
    kind_title: &str,
    language: Option<&str>,
    thumbnail: Option<PathBuf>,
    full: bool,
) -> DetailModel {
    let (bytes, lines) = match full {
        true => (QUICK_LOOK_BYTES, QUICK_LOOK_LINES),
        false => (PREVIEW_BYTES, PREVIEW_LINES),
    };
    if file.is_dir {
        return DetailModel::new(folder_preview(&file.path));
    }
    if kind_title == "Image" && file.size <= PREVIEW_IMAGE_BYTES {
        return DetailModel::new("").with_image(file.path.clone());
    }
    if let Some(language) = language {
        return match read_preview(&file.path, bytes, lines) {
            Some(text) => DetailModel::new(code_block_in(&text, language)),
            None => DetailModel::new(""),
        };
    }
    match thumbnail {
        Some(thumbnail) => DetailModel::new("").with_image(thumbnail),
        None => DetailModel::new(""),
    }
}

/// A folder's entries as a Markdown list, folders first.
fn folder_preview(path: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(path) else {
        return "*This folder can’t be read.*".into();
    };
    let mut entries: Vec<(bool, String)> = entries
        .flatten()
        .map(|entry| {
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            (is_dir, entry.file_name().to_string_lossy().into_owned())
        })
        .filter(|(_, name)| !name.starts_with('.'))
        .collect();
    if entries.is_empty() {
        return "*This folder is empty.*".into();
    }
    entries.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    let total = entries.len();
    let mut text: String = entries
        .iter()
        .take(FOLDER_ENTRIES)
        .map(|(is_dir, name)| {
            let name = name.replace('*', "\\*").replace('_', "\\_");
            match is_dir {
                true => format!("- **{name}/**\n"),
                false => format!("- {name}\n"),
            }
        })
        .collect();
    if total > FOLDER_ENTRIES {
        text.push_str(&format!("\n*and {} more*", total - FOLDER_ENTRIES));
    }
    text
}

/// The start of a text file, or `None` when it is not text.
fn read_preview(path: &Path, limit: usize, lines: usize) -> Option<String> {
    let mut bytes = Vec::with_capacity(limit.min(PREVIEW_BYTES));
    std::fs::File::open(path)
        .ok()?
        .take(limit as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    Some(text.lines().take(lines).collect::<Vec<_>>().join("\n"))
}

/// Quick Look: the preview of `file` on a page of its own, larger.
fn quick_look_page(
    file: &index::FileEntry,
    thumbnail: Option<PathBuf>,
    cx: &mut App,
) -> PageHandle {
    let (kind_title, _, language) = kind(&file.path, file.is_dir);
    let detail = preview_of(file, kind_title, language, thumbnail, true).with_actions(
        ActionPanel::new()
            .with_action(
                Action::new("Open", Effect::OpenPath(file.path.clone()))
                    .with_image(Image::Icon("external-link".into())),
            )
            .with_action(
                Action::new(REVEAL_TITLE, Effect::RevealPath(file.path.clone()))
                    .with_image(Image::Icon("folder-open".into())),
            )
            .with_action(
                Action::new(
                    "Copy Path",
                    Effect::Copy(file.path.display().to_string().into()),
                )
                .with_image(Image::Icon("copy".into()))
                .with_shortcut("secondary-shift-c"),
            ),
    );
    pages::handle(cx.new(|_| QuickLookPage {
        title: file.name().into(),
        detail,
    }))
}

struct QuickLookPage {
    title: SharedString,
    detail: DetailModel,
}

impl Page for QuickLookPage {
    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        PageModel::Detail(self.detail.clone())
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
