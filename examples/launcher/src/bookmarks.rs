//! Search Bookmarks: the bookmarks of Chromium browsers (Chrome, Edge,
//! Brave, Vivaldi, Chromium), read from each profile's `Bookmarks` file.

use std::path::{Path, PathBuf};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};
use serde::Deserialize;

use crate::{
    model::{
        Accessory, Action, ActionPanel, Choice, Dropdown, Effect, Image, Item, ItemId, ListModel,
        PageModel, Section, TextHandler,
    },
    pages::{self, Page, PageHandle},
};

const ALL: &str = "all";

/// A browser's name and its profiles' folder, per platform.
pub(crate) fn browsers() -> Vec<(&'static str, PathBuf)> {
    let base = if cfg!(target_os = "windows") {
        dirs::data_local_dir()
    } else {
        dirs::config_dir()
    };
    let Some(base) = base else {
        return Vec::new();
    };
    let folders: &[(&str, &str)] = if cfg!(target_os = "windows") {
        &[
            ("Chrome", "Google/Chrome/User Data"),
            ("Edge", "Microsoft/Edge/User Data"),
            ("Brave", "BraveSoftware/Brave-Browser/User Data"),
            ("Vivaldi", "Vivaldi/User Data"),
            ("Chromium", "Chromium/User Data"),
        ]
    } else if cfg!(target_os = "macos") {
        &[
            ("Chrome", "Google/Chrome"),
            ("Edge", "Microsoft Edge"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Vivaldi", "Vivaldi"),
            ("Arc", "Arc/User Data"),
            ("Chromium", "Chromium"),
        ]
    } else {
        &[
            ("Chrome", "google-chrome"),
            ("Edge", "microsoft-edge"),
            ("Brave", "BraveSoftware/Brave-Browser"),
            ("Vivaldi", "vivaldi"),
            ("Chromium", "chromium"),
        ]
    };
    folders
        .iter()
        .map(|(name, folder)| (*name, base.join(folder)))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
struct Bookmark {
    title: String,
    url: String,
    /// The folders it is in, such as `Bookmarks bar / Work`.
    folder: String,
    /// The browser, and the profile when there are several.
    source: String,
}

#[derive(Deserialize)]
struct BookmarksFile {
    roots: std::collections::BTreeMap<String, Node>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    children: Vec<Node>,
}

fn collect(node: &Node, folder: &str, source: &str, found: &mut Vec<Bookmark>) {
    match node.kind.as_str() {
        "url" => found.push(Bookmark {
            title: match node.name.trim() {
                "" => node.url.clone(),
                name => name.to_owned(),
            },
            url: node.url.clone(),
            folder: folder.to_owned(),
            source: source.to_owned(),
        }),
        _ => {
            let folder = match (folder.is_empty(), node.name.is_empty()) {
                (_, true) => folder.to_owned(),
                (true, false) => node.name.clone(),
                (false, false) => format!("{folder} / {}", node.name),
            };
            for child in &node.children {
                collect(child, &folder, source, found);
            }
        }
    }
}

/// Reads one `Bookmarks` file.
fn read(path: &Path, source: &str) -> Vec<Bookmark> {
    let Some(file) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<BookmarksFile>(&text).ok())
    else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for root in file.roots.values() {
        collect(root, "", source, &mut found);
    }
    found
}

/// Every bookmark of every profile of every installed browser. Blocking.
fn load() -> Vec<Bookmark> {
    let mut bookmarks = Vec::new();
    for (browser, folder) in browsers() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|profile| profile.join("Bookmarks").is_file())
            .collect();
        profiles.sort();
        let several = profiles.len() > 1;
        for profile in profiles {
            let source = match several {
                true => format!(
                    "{browser} ({})",
                    profile
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ),
                false => browser.to_owned(),
            };
            bookmarks.extend(read(&profile.join("Bookmarks"), &source));
        }
    }
    bookmarks
}

pub(crate) fn host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_default()
}

pub fn search_bookmarks_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let task = cx.spawn(async move |this, cx| {
            let bookmarks = cx.background_spawn(async { load() }).await;
            this.update(cx, |page: &mut BookmarksPage, cx| {
                page.bookmarks = Some(bookmarks);
                cx.notify();
            })
            .ok();
        });
        BookmarksPage {
            bookmarks: None,
            source: None,
            _task: task,
        }
    })))
}

struct BookmarksPage {
    bookmarks: Option<Vec<Bookmark>>,
    /// The browser shown, or every browser.
    source: Option<String>,
    _task: Task<()>,
}

impl Page for BookmarksPage {
    fn title(&self) -> SharedString {
        "Bookmarks".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let bookmarks = self.bookmarks.as_deref().unwrap_or_default();
        let mut sources: Vec<&str> = bookmarks.iter().map(|b| b.source.as_str()).collect();
        sources.sort();
        sources.dedup();
        let page = cx.entity().downgrade();
        let dropdown = sources.iter().fold(
            Dropdown::new("Browser")
                .with_choice(Choice::new(ALL, "All Browsers"))
                .with_value(self.source.clone().unwrap_or_else(|| ALL.into()))
                .with_on_change(TextHandler::new(move |value, _, cx| {
                    page.update(cx, |page, cx| {
                        page.source = (value != ALL).then(|| value.to_string());
                        cx.notify();
                    })
                    .ok();
                })),
            |dropdown, source| dropdown.with_choice(Choice::new(*source, *source)),
        );
        let items = bookmarks
            .iter()
            .enumerate()
            .filter(|(_, bookmark)| {
                self.source
                    .as_ref()
                    .is_none_or(|source| *source == bookmark.source)
            })
            .map(|(ix, bookmark)| {
                let site = host(&bookmark.url);
                let item = Item::new(
                    ItemId::new(format!("bookmark/{ix}")),
                    bookmark.title.clone(),
                )
                .with_subtitle(site.clone())
                .with_image(Image::Icon("bookmark".into()))
                .with_keyword(bookmark.url.clone())
                .with_keyword(bookmark.folder.clone())
                .with_actions(
                    ActionPanel::new()
                        .with_action(
                            Action::new(
                                "Open in Browser",
                                Effect::OpenUrl(bookmark.url.clone().into()),
                            )
                            .with_image(Image::Icon("globe".into())),
                        )
                        .with_action(
                            Action::new("Copy URL", Effect::Copy(bookmark.url.clone().into()))
                                .with_image(Image::Icon("copy".into()))
                                .with_shortcut("secondary-shift-c"),
                        )
                        .with_action(
                            Action::new(
                                "Copy as Markdown Link",
                                Effect::Copy(
                                    format!("[{}]({})", bookmark.title, bookmark.url).into(),
                                ),
                            )
                            .with_image(Image::Icon("link".into())),
                        ),
                );
                match (sources.len() > 1, bookmark.folder.is_empty()) {
                    (true, _) => item.with_accessory(Accessory::text(bookmark.source.clone())),
                    (false, false) => item.with_accessory(Accessory::text(bookmark.folder.clone())),
                    (false, true) => item,
                }
            });
        ListModel::new()
            .with_placeholder("Search bookmarks…")
            .with_loading(self.bookmarks.is_none())
            .with_dropdown(dropdown)
            .with_empty_title(match self.bookmarks {
                None => "Reading bookmarks…",
                Some(_) => "No bookmarks found",
            })
            .with_empty_description("Chrome, Edge, Brave, Vivaldi and Chromium are read.")
            .with_section(Section::new().with_title("Bookmarks").with_items(items))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_nested_bookmarks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Bookmarks");
        std::fs::write(
            &path,
            r#"{"roots":{
                "bookmark_bar":{"name":"Bookmarks bar","type":"folder","children":[
                    {"name":"GPUI","type":"url","url":"https://gpui-kit.com"},
                    {"name":"Work","type":"folder","children":[
                        {"name":"","type":"url","url":"https://example.com/x"}]}]},
                "other":{"name":"Other","type":"folder","children":[]}},
              "version":1}"#,
        )
        .unwrap();
        let found = read(&path, "Chrome");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].folder, "Bookmarks bar");
        assert_eq!(found[1].title, "https://example.com/x");
        assert_eq!(found[1].folder, "Bookmarks bar / Work");
        assert_eq!(host("https://www.example.com/x"), "example.com");
    }
}
