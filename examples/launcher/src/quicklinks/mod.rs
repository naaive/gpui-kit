//! Quicklinks: saved links and paths, opened from the root search.
//!
//! A link may hold placeholders that are filled in when it opens:
//! `{argument}` (or `{query}`) asks for text, `{argument name="city"}` names
//! one of several, and `{clipboard}`, `{date}`, `{time}` and `{datetime}`
//! insert what they say. In a URL, inserted text is percent-encoded so it
//! cannot change the URL's structure.

mod pages;

pub use pages::{
    create_quicklink_page, create_quicklink_page_with, fallback_items, quicklink_items,
    search_quicklinks_page,
};

use std::path::PathBuf;

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};
use percent_encoding::utf8_percent_encode;
use serde::{Deserialize, Serialize};

use crate::{
    model::Effect,
    placeholders::{self, Argument},
    search::write_snapshot,
    sources::fallback::QUERY_COMPONENT,
};

const VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Quicklink {
    id: String,
    name: String,
    link: String,
}

impl Quicklink {
    pub fn new(name: impl Into<String>, link: impl Into<String>) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        Self {
            id: format!("{nanos:x}"),
            name: name.into(),
            link: link.into(),
        }
    }

    /// The same quicklink with another name and link.
    pub fn with_contents(&self, name: impl Into<String>, link: impl Into<String>) -> Self {
        Self {
            id: self.id.clone(),
            name: name.into(),
            link: link.into(),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn link(&self) -> &str {
        &self.link
    }

    /// The text the link asks for, in order of first appearance.
    pub fn arguments(&self) -> Vec<Argument> {
        placeholders::arguments(&self.link)
    }

    /// The link with every placeholder filled in. `values` holds an
    /// argument's value by name; a missing one takes its default.
    pub fn expand(&self, values: &[(String, String)], clipboard: Option<&str>) -> String {
        match is_url(&self.link) {
            true => placeholders::expand(&self.link, values, clipboard, |text| {
                utf8_percent_encode(text, QUERY_COMPONENT).to_string()
            }),
            false => placeholders::expand(&self.link, values, clipboard, str::to_owned),
        }
    }

    /// What opening the link with `values` does: a URL opens in its
    /// application, anything else is a file or folder.
    pub fn open_effect(&self, values: &[(String, String)], clipboard: Option<&str>) -> Effect {
        let target = self.expand(values, clipboard);
        match is_url(&target) {
            true => Effect::OpenUrl(target.into()),
            false => Effect::OpenPath(expand_home(target.trim())),
        }
    }
}

/// A scheme such as `https:` or `vscode:`, but not a Windows drive (`C:`).
fn is_url(link: &str) -> bool {
    let link = link.trim();
    let Some((scheme, _)) = link.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest.trim_start_matches(['/', '\\'])))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct QuicklinksFile {
    version: u32,
    #[serde(default)]
    quicklinks: Vec<Quicklink>,
}

/// The saved quicklinks. Pages and the root search observe this entity.
pub struct QuicklinkStore {
    quicklinks: Vec<Quicklink>,
    path: Option<PathBuf>,
    save_task: Option<Task<()>>,
}

struct GlobalQuicklinkStore(Entity<QuicklinkStore>);

impl Global for GlobalQuicklinkStore {}

/// Loads the saved quicklinks.
pub fn start(cx: &mut App) {
    let path = crate::shell::data_directory().map(|dir| dir.join("quicklinks.json"));
    let quicklinks = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|source| {
            serde_json::from_str::<QuicklinksFile>(&source)
                .map_err(|error| tracing::warn!("ignoring damaged quicklinks file: {error}"))
                .ok()
        })
        .map(|file| file.quicklinks)
        .unwrap_or_default();
    let store = cx.new(|_| QuicklinkStore {
        quicklinks,
        path,
        save_task: None,
    });
    cx.set_global(GlobalQuicklinkStore(store));
}

pub fn store(cx: &App) -> Option<Entity<QuicklinkStore>> {
    cx.try_global::<GlobalQuicklinkStore>()
        .map(|store| store.0.clone())
}

impl QuicklinkStore {
    pub fn quicklinks(&self) -> &[Quicklink] {
        &self.quicklinks
    }

    /// Adds `quicklink`, or replaces the one with its id.
    pub fn save(&mut self, quicklink: Quicklink, cx: &mut Context<Self>) {
        match self
            .quicklinks
            .iter_mut()
            .find(|known| known.id == quicklink.id)
        {
            Some(known) => *known = quicklink,
            None => self.quicklinks.push(quicklink),
        }
        self.quicklinks
            .sort_by_cached_key(|quicklink| quicklink.name.to_lowercase());
        self.changed(cx);
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.quicklinks.retain(|quicklink| quicklink.id != id);
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(path) = self.path.clone() else {
            return;
        };
        let json = match serde_json::to_string_pretty(&QuicklinksFile {
            version: VERSION,
            quicklinks: self.quicklinks.clone(),
        }) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!("cannot serialize quicklinks: {error}");
                return;
            }
        };
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = write_snapshot(&path, &json) {
                tracing::warn!("cannot save quicklinks: {error}");
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arguments_are_collected_once_in_order() {
        let link = Quicklink::new(
            "Weather",
            r#"https://w.example/{argument name="city" default="Paris"}/{query}/{argument name="city"}/{clipboard}"#,
        );
        assert_eq!(
            link.arguments(),
            [
                Argument {
                    name: "city".into(),
                    default: "Paris".into()
                },
                Argument {
                    name: String::new(),
                    default: String::new()
                },
            ]
        );
    }

    #[test]
    fn test_expand_encodes_values_in_urls_only() {
        let search = Quicklink::new("GitHub", "https://github.com/search?q={argument}&type=code");
        assert_eq!(
            search.expand(&[(String::new(), "a&b #1".into())], None),
            "https://github.com/search?q=a%26b%20%231&type=code"
        );
        let named = Quicklink::new(
            "City",
            r#"https://w.example/{argument name="city" default="Paris"}"#,
        );
        assert_eq!(named.expand(&[], None), "https://w.example/Paris");

        let folder = Quicklink::new("Project", r"C:\work\{argument}\{unknown}");
        assert_eq!(
            folder.expand(&[(String::new(), "a b".into())], Some("x")),
            r"C:\work\a b\{unknown}"
        );
        assert!(matches!(folder.open_effect(&[], None), Effect::OpenPath(_)));
        assert!(matches!(search.open_effect(&[], None), Effect::OpenUrl(_)));
        assert!(is_url("vscode://file/a"));
        assert!(!is_url(r"C:\Users"));
    }

    #[test]
    fn test_clipboard_placeholder() {
        let link = Quicklink::new("Translate", "https://t.example/?text={clipboard}");
        assert_eq!(
            link.expand(&[], Some("hello world")),
            "https://t.example/?text=hello%20world"
        );
    }
}
