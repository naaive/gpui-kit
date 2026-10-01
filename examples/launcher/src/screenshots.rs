//! Search Screenshots: the screenshots the system saved, newest first, found
//! by the words in them as well as by their date.
//!
//! Text is recognized once per screenshot, in the background, and kept in a
//! cache next to the other launcher data, so the page opens at once and a
//! screenshot taken a minute ago is searchable a moment later.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Task, Window};
use serde::{Deserialize, Serialize};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, Effect, Image, Item, ItemId,
        Layout, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
    sources::applications::REVEAL_TITLE,
};

/// How many screenshots are listed; the oldest beyond this are left out.
const MAX_SCREENSHOTS: usize = 500;

/// Where the system saves screenshots: Windows' Screenshots folder (also
/// under OneDrive when it backs up Pictures), the desktop on macOS.
fn folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    #[cfg(target_os = "windows")]
    folders.extend(known_screenshots_folder());
    if let Some(pictures) = dirs::picture_dir() {
        folders.push(pictures.join("Screenshots"));
    }
    if let Some(home) = dirs::home_dir() {
        folders.push(home.join("OneDrive").join("Pictures").join("Screenshots"));
        folders.push(home.join("OneDrive").join("图片").join("Screenshots"));
    }
    if cfg!(target_os = "macos")
        && let Some(desktop) = dirs::desktop_dir()
    {
        folders.push(desktop);
    }
    folders.retain(|folder| folder.is_dir());
    folders.dedup();
    folders
}

/// The folder Windows saves screenshots in, wherever the user moved it.
#[cfg(target_os = "windows")]
fn known_screenshots_folder() -> Option<PathBuf> {
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Screenshots, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    };
    unsafe {
        let path = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None).ok()?;
        let folder = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as *const std::ffi::c_void));
        folder
    }
}

fn is_screenshot(path: &Path) -> bool {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg") {
        return false;
    }
    // The desktop holds other images; only its screenshots count.
    !cfg!(target_os = "macos")
        || path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("Screenshot"))
}

/// One screenshot on disk.
#[derive(Clone, Debug)]
struct Screenshot {
    path: PathBuf,
    /// Seconds since the Unix epoch.
    modified: u64,
}

fn scan() -> Vec<Screenshot> {
    let mut found: Vec<Screenshot> = folders()
        .iter()
        .filter_map(|folder| std::fs::read_dir(folder).ok())
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !is_screenshot(&path) {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())?
                .as_secs();
            Some(Screenshot { path, modified })
        })
        .collect();
    found.sort_by_key(|screenshot| std::cmp::Reverse(screenshot.modified));
    found.truncate(MAX_SCREENSHOTS);
    found
}

/// Recognized text by path, with the modification time it was read at.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct TextCache {
    entries: HashMap<PathBuf, (u64, String)>,
}

fn cache_path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("screenshot-text.json"))
}

impl TextCache {
    fn load() -> Self {
        cache_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let (Some(path), Ok(text)) = (cache_path(), serde_json::to_string(self)) {
            crate::search::write_snapshot(&path, &text).ok();
        }
    }

    fn text(&self, screenshot: &Screenshot) -> Option<&str> {
        self.entries
            .get(&screenshot.path)
            .filter(|(modified, _)| *modified == screenshot.modified)
            .map(|(_, text)| text.as_str())
    }
}

/// The screenshots and their text, shared by the page and the indexer.
pub struct ScreenshotIndex {
    screenshots: Vec<Screenshot>,
    cache: TextCache,
    indexing: Option<Task<()>>,
}

struct GlobalIndex(Entity<ScreenshotIndex>);

impl Global for GlobalIndex {}

fn index(cx: &mut App) -> Entity<ScreenshotIndex> {
    if let Some(index) = cx.try_global::<GlobalIndex>() {
        return index.0.clone();
    }
    let index = cx.new(|_| ScreenshotIndex {
        screenshots: Vec::new(),
        cache: TextCache::load(),
        indexing: None,
    });
    cx.set_global(GlobalIndex(index.clone()));
    index
}

impl ScreenshotIndex {
    /// Lists the screenshots again, then reads the text of the new ones,
    /// newest first, one at a time.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.screenshots = scan();
        cx.notify();
        if self.indexing.is_some() {
            return;
        }
        let pending: Vec<Screenshot> = self
            .screenshots
            .iter()
            .filter(|screenshot| self.cache.text(screenshot).is_none())
            .cloned()
            .collect();
        if pending.is_empty() {
            return;
        }
        self.indexing = Some(cx.spawn(async move |this, cx| {
            for (done, screenshot) in pending.into_iter().enumerate() {
                let path = screenshot.path.clone();
                let text = cx
                    .background_spawn(async move { crate::ocr::recognize(&path) })
                    .await
                    .unwrap_or_default();
                let alive = this
                    .update(cx, |index, cx| {
                        index
                            .cache
                            .entries
                            .insert(screenshot.path, (screenshot.modified, text));
                        // Saved now and then, not after every image.
                        if done % 10 == 9 {
                            index.cache.save();
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    return;
                }
            }
            this.update(cx, |index, cx| {
                // Screenshots that are gone need no text.
                let existing: std::collections::HashSet<&PathBuf> =
                    index.screenshots.iter().map(|s| &s.path).collect();
                index
                    .cache
                    .entries
                    .retain(|path, _| existing.contains(path));
                index.cache.save();
                index.indexing = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn pending(&self) -> usize {
        self.screenshots
            .iter()
            .filter(|screenshot| self.cache.text(screenshot).is_none())
            .count()
    }
}

pub fn search_screenshots_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let index = index(cx);
    index.update(cx, |index, cx| index.refresh(cx));
    Ok(pages::handle(cx.new(|cx| {
        let subscription = cx.observe(&index, |page: &mut ScreenshotsPage, _, cx| {
            page.list = None;
            cx.notify();
        });
        ScreenshotsPage {
            index,
            list: None,
            _subscription: subscription,
        }
    })))
}

struct ScreenshotsPage {
    index: Entity<ScreenshotIndex>,
    /// Built when the index changes, not on every frame.
    list: Option<ListModel>,
    _subscription: gpui_kit::Subscription,
}

fn item(screenshot: &Screenshot, text: Option<&str>) -> Item {
    let name = screenshot
        .path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let date = crate::format::format_time(screenshot.modified);
    let path = screenshot.path.clone();
    let copy = {
        let path = path.clone();
        Effect::Run(RunHandler::new(move |(), _, cx| {
            let effect = match std::fs::read(&path) {
                Ok(bytes) => Effect::CopyItem(gpui_kit::ClipboardItem::new_image(
                    &gpui_kit::Image::from_bytes(image_format(&path), bytes),
                )),
                Err(_) => Effect::ShowHud("The screenshot no longer exists".into()),
            };
            crate::shell::launcher::perform(effect, cx);
        }))
    };
    let mut actions = ActionPanel::new()
        .with_action(
            Action::new("Open Screenshot", Effect::OpenPath(path.clone()))
                .with_image(Image::Icon("external-link".into())),
        )
        .with_action(
            Action::new("Copy Image", copy)
                .with_image(Image::Icon("copy".into()))
                .with_shortcut("secondary-shift-c"),
        )
        .with_action(
            Action::new(REVEAL_TITLE, Effect::RevealPath(path.clone()))
                .with_image(Image::Icon("folder-open".into()))
                .with_shortcut("secondary-shift-f"),
        );
    if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
        actions = actions.with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new(
                    "Copy Text in Screenshot",
                    Effect::Copy(text.to_owned().into()),
                )
                .with_image(Image::Icon("scan-text".into()))
                .with_shortcut("secondary-shift-t"),
            )),
        );
    }
    Item::new(
        ItemId::new(format!("screenshot:{}", path.display())),
        date.clone(),
    )
    .with_image(Image::File(path))
    .with_keyword(name)
    .with_keyword(text.unwrap_or_default().to_owned())
    .with_accessory(Accessory::text(date))
    .with_actions(actions)
}

fn image_format(path: &Path) -> gpui_kit::ImageFormat {
    match path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => gpui_kit::ImageFormat::Jpeg,
        _ => gpui_kit::ImageFormat::Png,
    }
}

impl Page for ScreenshotsPage {
    fn title(&self) -> SharedString {
        "Screenshots".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        if let Some(list) = &self.list {
            return list.clone().into();
        }
        let index = self.index.read(cx);
        let pending = index.pending();
        let section = Section::new()
            .with_title("Screenshots")
            .with_subtitle(match pending {
                0 => index.screenshots.len().to_string(),
                pending => format!("{} · reading text in {pending}", index.screenshots.len()),
            })
            .with_items(
                index
                    .screenshots
                    .iter()
                    .map(|screenshot| item(screenshot, index.cache.text(screenshot))),
            );
        let list = ListModel::new()
            .with_layout(Layout::Grid { columns: 4 })
            .with_placeholder("Search screenshots by the text in them…")
            .with_empty_title("No screenshots")
            .with_empty_description(match folders().first() {
                Some(folder) => format!("Screenshots saved to {} appear here.", folder.display()),
                None => "Win+PrtScn saves screenshots to Pictures\\Screenshots.".to_owned(),
            })
            .with_section(section);
        self.list = Some(list.clone());
        list.into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
