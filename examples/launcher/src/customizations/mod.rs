//! What the user set on root search items: aliases, favorites and global
//! hotkeys, as Raycast offers on every command.
//!
//! - An alias typed exactly puts its item first.
//! - Favorites lead the empty search, in the order they were added.
//! - A hotkey opens its item from any application.

mod pages;

pub use pages::{alias_page, hotkey_page};

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use gpui_kit::{App, AppContext as _, Context, Entity, Global, Task};
use serde::{Deserialize, Serialize};

use crate::search::write_snapshot;

const VERSION: u32 = 1;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct CustomizationsFile {
    version: u32,
    /// Alias by item id.
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    /// Item ids, in the order they were added.
    #[serde(default)]
    favorites: Vec<String>,
    /// Shortcut (as GPUI writes it, `ctrl-alt-c`) by item id.
    #[serde(default)]
    hotkeys: BTreeMap<String, String>,
    /// Items left out of the root search, and whose hotkeys do nothing.
    #[serde(default)]
    disabled: BTreeSet<String>,
}

pub struct Customizations {
    file: CustomizationsFile,
    path: Option<PathBuf>,
    save_task: Option<Task<()>>,
}

struct GlobalCustomizations(Entity<Customizations>);

impl Global for GlobalCustomizations {}

/// Loads what was customized; [`crate::shell::launcher`] then registers the
/// hotkeys.
pub fn start(cx: &mut App) {
    let path = crate::shell::data_directory().map(|dir| dir.join("customizations.json"));
    let file = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|source| {
            serde_json::from_str::<CustomizationsFile>(&source)
                .map_err(|error| tracing::warn!("ignoring damaged customizations: {error}"))
                .ok()
        })
        .unwrap_or_default();
    let store = cx.new(|_| Customizations {
        file,
        path,
        save_task: None,
    });
    cx.set_global(GlobalCustomizations(store));
}

pub fn store(cx: &App) -> Option<Entity<Customizations>> {
    cx.try_global::<GlobalCustomizations>()
        .map(|store| store.0.clone())
}

impl Customizations {
    pub fn alias(&self, item: &str) -> Option<&str> {
        self.file.aliases.get(item).map(String::as_str)
    }

    /// The item `alias` belongs to, ignoring case.
    pub fn item_with_alias(&self, alias: &str) -> Option<&str> {
        self.file
            .aliases
            .iter()
            .find(|(_, known)| known.eq_ignore_ascii_case(alias))
            .map(|(item, _)| item.as_str())
    }

    pub fn aliases(&self) -> impl Iterator<Item = (&str, &str)> {
        self.file
            .aliases
            .iter()
            .map(|(item, alias)| (item.as_str(), alias.as_str()))
    }

    pub fn favorites(&self) -> &[String] {
        &self.file.favorites
    }

    pub fn hotkey(&self, item: &str) -> Option<&str> {
        self.file.hotkeys.get(item).map(String::as_str)
    }

    pub fn hotkeys(&self) -> impl Iterator<Item = (&str, &str)> {
        self.file
            .hotkeys
            .iter()
            .map(|(item, shortcut)| (item.as_str(), shortcut.as_str()))
    }

    /// Whether `item` is left out of the root search.
    pub fn is_disabled(&self, item: &str) -> bool {
        self.file.disabled.contains(item)
    }

    pub fn disabled(&self) -> &BTreeSet<String> {
        &self.file.disabled
    }

    /// Enables or disables every item in `items` at once, such as all the
    /// commands of an extension.
    pub fn set_enabled<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a str>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        for item in items {
            match enabled {
                true => self.file.disabled.remove(item),
                false => self.file.disabled.insert(item.to_owned()),
            };
        }
        self.changed(cx);
    }

    /// Sets `item`'s alias; an empty one removes it.
    pub fn set_alias(&mut self, item: &str, alias: &str, cx: &mut Context<Self>) {
        match alias.trim() {
            "" => self.file.aliases.remove(item),
            alias => self.file.aliases.insert(item.to_owned(), alias.to_owned()),
        };
        self.changed(cx);
    }

    pub fn set_favorite(&mut self, item: &str, favorite: bool, cx: &mut Context<Self>) {
        self.file.favorites.retain(|known| known != item);
        if favorite {
            self.file.favorites.push(item.to_owned());
        }
        self.changed(cx);
    }

    /// Moves a favorite one place up (`-1`) or down (`1`).
    pub fn move_favorite(&mut self, item: &str, step: isize, cx: &mut Context<Self>) {
        let Some(ix) = self.file.favorites.iter().position(|known| known == item) else {
            return;
        };
        let target = ix as isize + step;
        if (0..self.file.favorites.len() as isize).contains(&target) {
            self.file.favorites.swap(ix, target as usize);
            self.changed(cx);
        }
    }

    /// Sets `item`'s hotkey; an empty one removes it.
    pub fn set_hotkey(&mut self, item: &str, shortcut: &str, cx: &mut Context<Self>) {
        match shortcut.trim() {
            "" => self.file.hotkeys.remove(item),
            shortcut => self
                .file
                .hotkeys
                .insert(item.to_owned(), shortcut.to_owned()),
        };
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(path) = self.path.clone() else {
            return;
        };
        let file = CustomizationsFile {
            version: VERSION,
            ..self.file.clone()
        };
        let json = match serde_json::to_string_pretty(&file) {
            Ok(json) => json,
            Err(error) => {
                tracing::warn!("cannot serialize customizations: {error}");
                return;
            }
        };
        let previous = self.save_task.take();
        self.save_task = Some(cx.background_spawn(async move {
            if let Some(previous) = previous {
                previous.await;
            }
            if let Err(error) = write_snapshot(&path, &json) {
                tracing::warn!("cannot save customizations: {error}");
            }
        }));
    }
}
