//! Snippets: saved text to paste, found by name, keyword or content.
//!
//! A snippet's text may hold the placeholders of [`crate::placeholders`];
//! `{argument}` asks for text before pasting. Typing a snippet's keyword in
//! the root search puts it first.

mod clipboard_backup;
mod expansion;
mod pages;

pub use pages::{create_snippet_page, search_snippets_page, snippet_items};

use std::path::PathBuf;

use std::time::Duration;

use gpui_kit::{App, AppContext as _, AsyncApp, ClipboardItem, Context, Entity, Global, Task};

use self::expansion::{Expander, Keywords, erase_and_paste};
use serde::{Deserialize, Serialize};

use crate::{placeholders, search::write_snapshot};

const VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Snippet {
    id: String,
    name: String,
    text: String,
    #[serde(default)]
    keyword: String,
}

impl Snippet {
    pub fn new(name: String, text: String, keyword: String) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        Self {
            id: format!("{nanos:x}"),
            name,
            text,
            keyword,
        }
    }

    pub fn with_contents(&self, name: String, text: String, keyword: String) -> Self {
        Self {
            id: self.id.clone(),
            name,
            text,
            keyword,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn keyword(&self) -> &str {
        &self.keyword
    }

    pub fn arguments(&self) -> Vec<placeholders::Argument> {
        placeholders::arguments(&self.text)
    }

    /// The text to paste, placeholders filled in.
    pub fn expand(&self, values: &[(String, String)], clipboard: Option<&str>) -> String {
        placeholders::expand(&self.text, values, clipboard, str::to_owned)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SnippetsFile {
    version: u32,
    #[serde(default)]
    snippets: Vec<Snippet>,
}

pub struct SnippetStore {
    snippets: Vec<Snippet>,
    /// The keywords as the expansion worker reads them.
    keywords: Keywords,
    /// Watches typing while expansion is on.
    expander: Option<Expander>,
    path: Option<PathBuf>,
    save_task: Option<Task<()>>,
}

struct GlobalSnippetStore(Entity<SnippetStore>);

impl Global for GlobalSnippetStore {}

/// Loads the saved snippets.
pub fn start(cx: &mut App) {
    let path = crate::shell::data_directory().map(|dir| dir.join("snippets.json"));
    let snippets = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|source| {
            serde_json::from_str::<SnippetsFile>(&source)
                .map_err(|error| tracing::warn!("ignoring damaged snippets file: {error}"))
                .ok()
        })
        .map(|file| file.snippets)
        .unwrap_or_default();
    let store = cx.new(|_| SnippetStore {
        keywords: Keywords::default(),
        expander: None,
        snippets,
        path,
        save_task: None,
    });
    store.update(cx, |store, _| store.share_keywords());
    cx.set_global(GlobalSnippetStore(store));
}

pub fn store(cx: &App) -> Option<Entity<SnippetStore>> {
    cx.try_global::<GlobalSnippetStore>()
        .map(|store| store.0.clone())
}

impl SnippetStore {
    pub fn snippets(&self) -> &[Snippet] {
        &self.snippets
    }

    /// Whether another snippet already uses `keyword`.
    pub fn keyword_taken(&self, keyword: &str, except: Option<&str>) -> bool {
        !keyword.is_empty()
            && self.snippets.iter().any(|snippet| {
                snippet.keyword.eq_ignore_ascii_case(keyword) && Some(snippet.id()) != except
            })
    }

    /// Adds `snippet`, or replaces the one with its id.
    pub fn save(&mut self, snippet: Snippet, cx: &mut Context<Self>) {
        match self
            .snippets
            .iter_mut()
            .find(|known| known.id == snippet.id)
        {
            Some(known) => *known = snippet,
            None => self.snippets.push(snippet),
        }
        self.snippets
            .sort_by_cached_key(|snippet| snippet.name.to_lowercase());
        self.changed(cx);
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.snippets.retain(|snippet| snippet.id != id);
        self.changed(cx);
    }

    /// Hands the keywords to the expansion worker.
    fn share_keywords(&self) {
        if let Ok(mut keywords) = self.keywords.lock() {
            *keywords = self
                .snippets
                .iter()
                .filter(|snippet| !snippet.keyword.is_empty() && snippet.arguments().is_empty())
                .map(|snippet| (snippet.keyword.clone(), snippet.id.clone()))
                .collect();
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.share_keywords();
        cx.notify();
        let Some(path) = self.path.clone() else {
            return;
        };
        let json = match serde_json::to_string_pretty(&SnippetsFile {
            version: VERSION,
            snippets: self.snippets.clone(),
        }) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!("cannot serialize snippets: {error}");
                return;
            }
        };
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = write_snapshot(&path, &json) {
                tracing::warn!("cannot save snippets: {error}");
            }
        }));
    }
}

/// Turns expansion of keywords typed in any application on or off.
pub fn set_expansion(enabled: bool, cx: &mut App) {
    let Some(store) = store(cx) else {
        return;
    };
    let running = store.read(cx).expander.is_some();
    if enabled == running {
        return;
    }
    if !enabled {
        store.update(cx, |store, _| store.expander = None);
        return;
    }
    let (found, completed) = smol::channel::unbounded::<(String, String)>();
    let keywords = store.read(cx).keywords.clone();
    let Some(expander) = Expander::start(keywords, found) else {
        return;
    };
    store.update(cx, |store, _| store.expander = Some(expander));
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok((keyword, id)) = completed.recv().await {
            let snippet = cx.update(|cx| {
                self::store(cx).and_then(|store| {
                    store
                        .read(cx)
                        .snippets()
                        .iter()
                        .find(|snippet| snippet.id() == id)
                        .cloned()
                })
            });
            if let Some(snippet) = snippet {
                expand(&snippet, &keyword, cx).await;
            }
        }
    })
    .detach();
}

/// Replaces the typed `keyword` with `snippet` through the clipboard, then
/// puts back what was on the clipboard.
async fn expand(snippet: &Snippet, keyword: &str, cx: &mut AsyncApp) {
    // Every format, where the platform allows; what GPUI can read otherwise.
    let backup = cx
        .background_executor()
        .spawn(async { clipboard_backup::ClipboardBackup::take() })
        .await;
    let previous = cx.update(|cx| {
        let previous = cx.read_from_clipboard();
        let clipboard = previous.as_ref().and_then(ClipboardItem::text);
        let text = snippet.expand(&[], clipboard.as_deref());
        if let Some(history) = crate::clipboard::store(cx) {
            history.update(cx, |history, _| {
                history.ignore_changes_for(Duration::from_secs(2))
            });
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        previous
    });
    let count = keyword.chars().count();
    cx.background_executor()
        .spawn(async move { erase_and_paste(count) })
        .await;
    // The application reads the clipboard when it handles the paste.
    cx.background_executor()
        .timer(Duration::from_millis(600))
        .await;
    let restored = match backup {
        Some(backup) => {
            cx.background_executor()
                .spawn(async move { backup.restore() })
                .await
        }
        None => false,
    };
    if !restored && let Some(previous) = previous {
        cx.update(|cx| cx.write_to_clipboard(previous));
    }
}
