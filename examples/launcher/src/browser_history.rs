//! Search Browser History: the pages visited in Chromium browsers (Chrome,
//! Edge, Brave, Vivaldi, Chromium) and Firefox, newest first.
//!
//! A running browser keeps its history database locked, so each is copied
//! to a temporary folder when the page opens, and searched there as the
//! query changes. The copies are removed when the page closes.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};
use rusqlite::{Connection, OpenFlags, params_from_iter};

use crate::{
    bookmarks::{browsers, host},
    format::{format_time, relative_time},
    model::{
        Accessory, Action, ActionPanel, Choice, Dropdown, Effect, Image, Item, ItemId, ListModel,
        PageModel, Section, TextHandler,
    },
    pages::{self, Page, PageHandle},
};

const ALL: &str = "all";
/// Rows read from each database for one search.
const RESULTS: usize = 150;
/// Typing faster than this does not search on each key.
const DEBOUNCE: Duration = Duration::from_millis(120);
/// Microseconds from 1601-01-01, where Chromium counts from, to 1970-01-01.
const CHROMIUM_EPOCH: i64 = 11_644_473_600_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Engine {
    Chromium,
    Firefox,
}

/// A browser profile's history, copied where it can be read.
#[derive(Clone, Debug)]
struct Database {
    source: String,
    engine: Engine,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
struct Visit {
    title: String,
    url: String,
    /// Unix seconds.
    last_visit: u64,
    visits: u64,
    source: String,
}

/// Every profile's history file: `(source, engine, file)`.
fn history_files() -> Vec<(String, Engine, PathBuf)> {
    let mut found = Vec::new();
    for (browser, folder) in browsers() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|profile| profile.join("History").is_file())
            .collect();
        profiles.sort();
        let several = profiles.len() > 1;
        for profile in profiles {
            found.push((
                source(browser, &profile, several),
                Engine::Chromium,
                profile.join("History"),
            ));
        }
    }
    if let Some(folder) = firefox_profiles()
        && let Ok(entries) = std::fs::read_dir(&folder)
    {
        let mut profiles: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|profile| profile.join("places.sqlite").is_file())
            .collect();
        profiles.sort();
        let several = profiles.len() > 1;
        for profile in profiles {
            found.push((
                source("Firefox", &profile, several),
                Engine::Firefox,
                profile.join("places.sqlite"),
            ));
        }
    }
    found
}

fn firefox_profiles() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        dirs::data_dir().map(|data| data.join("Mozilla/Firefox/Profiles"))
    } else if cfg!(target_os = "macos") {
        dirs::data_dir().map(|data| data.join("Firefox/Profiles"))
    } else {
        dirs::home_dir().map(|home| home.join(".mozilla/firefox"))
    }
}

/// The browser, and the profile when there are several.
fn source(browser: &str, profile: &Path, several: bool) -> String {
    match several {
        true => format!(
            "{browser} ({})",
            profile
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        ),
        false => browser.to_owned(),
    }
}

/// Where this page's copies go; one folder per page, so two pages never
/// share files.
fn copies_folder() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    std::env::temp_dir().join(format!(
        "launcher-history-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Copies every history database into `folder`. Blocking.
fn snapshot(folder: &Path) -> Vec<Database> {
    if std::fs::create_dir_all(folder).is_err() {
        return Vec::new();
    }
    history_files()
        .into_iter()
        .enumerate()
        .filter_map(|(ix, (source, engine, file))| {
            let path = folder.join(format!("{ix}.sqlite"));
            std::fs::copy(&file, &path).ok()?;
            // Firefox keeps recent visits in its write-ahead log.
            for suffix in ["-wal", "-journal"] {
                let mut side = file.clone().into_os_string();
                side.push(suffix);
                let mut copy = path.clone().into_os_string();
                copy.push(suffix);
                let _ = std::fs::copy(PathBuf::from(side), PathBuf::from(copy));
            }
            Some(Database {
                source,
                engine,
                path,
            })
        })
        .collect()
}

/// `term` for a `LIKE` pattern, with its wildcards taken literally.
fn like(term: &str) -> String {
    let escaped = term
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// The visits in `database` whose title or address has every word of
/// `query`, newest first.
fn search_one(database: &Database, query: &str) -> rusqlite::Result<Vec<Visit>> {
    let connection = Connection::open_with_flags(
        &database.path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let terms: Vec<String> = query.split_whitespace().map(like).collect();
    let filter: String = terms
        .iter()
        .map(|_| " AND (url LIKE ? ESCAPE '\\' OR title LIKE ? ESCAPE '\\')")
        .collect();
    let sql = match database.engine {
        Engine::Chromium => format!(
            "SELECT title, url, last_visit_time, visit_count FROM urls \
             WHERE hidden = 0 AND last_visit_time > 0{filter} \
             ORDER BY last_visit_time DESC LIMIT {RESULTS}"
        ),
        Engine::Firefox => format!(
            "SELECT title, url, last_visit_date, visit_count FROM moz_places \
             WHERE hidden = 0 AND last_visit_date IS NOT NULL{filter} \
             ORDER BY last_visit_date DESC LIMIT {RESULTS}"
        ),
    };
    let mut statement = connection.prepare(&sql)?;
    let arguments = terms.iter().flat_map(|term| [term, term]);
    let rows = statement.query_map(params_from_iter(arguments), |row| {
        let micros: i64 = row.get(2)?;
        let unix = match database.engine {
            Engine::Chromium => micros - CHROMIUM_EPOCH,
            Engine::Firefox => micros,
        };
        let url: String = row.get(1)?;
        Ok(Visit {
            title: row
                .get::<_, Option<String>>(0)?
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| url.clone()),
            url,
            last_visit: (unix / 1_000_000).max(0) as u64,
            visits: row.get::<_, i64>(3)?.max(0) as u64,
            source: database.source.clone(),
        })
    })?;
    rows.collect()
}

/// Matches across every database, newest first, each address once.
fn search(databases: &[Database], query: &str) -> Vec<Visit> {
    let mut visits: Vec<Visit> = databases
        .iter()
        .flat_map(|database| match search_one(database, query) {
            Ok(visits) => visits,
            Err(error) => {
                tracing::warn!("cannot read {} history: {error}", database.source);
                Vec::new()
            }
        })
        .collect();
    visits.sort_by_key(|visit| std::cmp::Reverse(visit.last_visit));
    let mut seen = std::collections::HashSet::new();
    visits.retain(|visit| seen.insert(visit.url.clone()));
    visits.truncate(RESULTS);
    visits
}

pub fn search_history_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|cx| {
        let folder = copies_folder();
        let copy_into = folder.clone();
        let task = cx.spawn(async move |this, cx| {
            let databases = cx
                .background_spawn(async move { snapshot(&copy_into) })
                .await;
            this.update(cx, |page: &mut HistoryPage, cx| {
                page.databases = Some(databases);
                page.search(cx);
            })
            .ok();
        });
        HistoryPage {
            folder,
            databases: None,
            query: String::new(),
            visits: None,
            source: None,
            task,
        }
    })))
}

struct HistoryPage {
    folder: PathBuf,
    databases: Option<Vec<Database>>,
    query: String,
    /// The latest search's results; `None` while the first one runs.
    visits: Option<Vec<Visit>>,
    /// The browser shown, or every browser.
    source: Option<String>,
    task: Task<()>,
}

impl Drop for HistoryPage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

impl HistoryPage {
    fn search(&mut self, cx: &mut Context<Self>) {
        let Some(databases) = self.databases.clone() else {
            return;
        };
        let databases: Vec<Database> = databases
            .into_iter()
            .filter(|database| {
                self.source
                    .as_ref()
                    .is_none_or(|source| *source == database.source)
            })
            .collect();
        let query = self.query.clone();
        let wait = self.visits.is_some() && !query.is_empty();
        self.task = cx.spawn(async move |this, cx| {
            if wait {
                cx.background_executor().timer(DEBOUNCE).await;
            }
            let visits = cx
                .background_spawn(async move { search(&databases, &query) })
                .await;
            this.update(cx, |page, cx| {
                page.visits = Some(visits);
                cx.notify();
            })
            .ok();
        });
    }
}

fn item(ix: usize, visit: &Visit, several: bool, now: u64) -> Item {
    let item = Item::new(ItemId::new(format!("history/{ix}")), visit.title.clone())
        .with_subtitle(host(&visit.url))
        .with_image(Image::Icon("clock".into()))
        .with_accessory(
            Accessory::text(relative_time(visit.last_visit, now)).with_tooltip(format!(
                "Last visited {} · {} visits",
                format_time(visit.last_visit),
                visit.visits
            )),
        )
        .with_actions(
            ActionPanel::new()
                .with_action(
                    Action::new("Open in Browser", Effect::OpenUrl(visit.url.clone().into()))
                        .with_image(Image::Icon("globe".into())),
                )
                .with_action(
                    Action::new("Copy URL", Effect::Copy(visit.url.clone().into()))
                        .with_image(Image::Icon("copy".into()))
                        .with_shortcut("secondary-shift-c"),
                )
                .with_action(
                    Action::new(
                        "Copy as Markdown Link",
                        Effect::Copy(format!("[{}]({})", visit.title, visit.url).into()),
                    )
                    .with_image(Image::Icon("link".into())),
                ),
        );
    match several {
        true => item.with_accessory(Accessory::text(visit.source.clone())),
        false => item,
    }
}

impl Page for HistoryPage {
    fn title(&self) -> SharedString {
        "Browser History".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let databases = self.databases.as_deref().unwrap_or_default();
        let page = cx.entity().downgrade();
        let dropdown = databases.iter().fold(
            Dropdown::new("Browser")
                .with_choice(Choice::new(ALL, "All Browsers"))
                .with_value(self.source.clone().unwrap_or_else(|| ALL.into()))
                .with_on_change(TextHandler::new(move |value, _, cx| {
                    page.update(cx, |page, cx| {
                        page.source = (value != ALL).then(|| value.to_string());
                        page.search(cx);
                    })
                    .ok();
                })),
            |dropdown, database| {
                dropdown.with_choice(Choice::new(
                    database.source.clone(),
                    database.source.clone(),
                ))
            },
        );
        let several = databases.len() > 1 && self.source.is_none();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let visits = self.visits.as_deref().unwrap_or_default();
        let items = visits
            .iter()
            .enumerate()
            .map(|(ix, visit)| item(ix, visit, several, now));
        ListModel::new()
            .with_placeholder("Search browser history…")
            .with_filtering(false)
            .with_loading(self.visits.is_none())
            .with_dropdown(dropdown)
            .with_empty_title(match (&self.visits, databases.is_empty()) {
                (None, _) => "Reading history…",
                (Some(_), true) => "No browser history found",
                (Some(_), false) => "No pages match",
            })
            .with_empty_description("Chrome, Edge, Brave, Vivaldi, Chromium and Firefox are read.")
            .with_section(
                Section::new()
                    .with_title(match self.query.trim().is_empty() {
                        true => "Recently Visited",
                        false => "Pages",
                    })
                    .with_items(items),
            )
            .into()
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.search(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chromium(folder: &Path) -> Database {
        let path = folder.join("History");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE urls (id INTEGER PRIMARY KEY, url TEXT, title TEXT, \
                 visit_count INTEGER, typed_count INTEGER, last_visit_time INTEGER, \
                 hidden INTEGER);
                 INSERT INTO urls VALUES (1, 'https://gpui-kit.com/docs', 'GPUI Kit docs', 3, 0, 13370000000000000, 0);
                 INSERT INTO urls VALUES (2, 'https://example.com/100%', '', 1, 0, 13380000000000000, 0);
                 INSERT INTO urls VALUES (3, 'https://hidden.example', 'Hidden', 1, 0, 13390000000000000, 1);",
            )
            .unwrap();
        Database {
            source: "Chrome".into(),
            engine: Engine::Chromium,
            path,
        }
    }

    #[test]
    fn test_searches_titles_and_addresses_newest_first() {
        let folder = tempfile::tempdir().unwrap();
        let databases = [chromium(folder.path())];
        let all = search(&databases, "");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].url, "https://example.com/100%");
        // An untitled page is shown by its address.
        assert_eq!(all[0].title, "https://example.com/100%");
        assert_eq!(
            all[1].last_visit,
            13370000000000000 / 1_000_000 - 11_644_473_600
        );
        let found = search(&databases, "gpui DOCS");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].visits, 3);
        // `%` is matched literally.
        assert_eq!(search(&databases, "100%").len(), 1);
        assert!(search(&databases, "0%x").is_empty());
    }
}
