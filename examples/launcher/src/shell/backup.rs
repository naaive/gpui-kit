//! Exporting the launcher's settings and data to one file, and importing it
//! on another computer or after a reinstall.
//!
//! The file holds the settings, aliases, hotkeys and favorites, quicklinks,
//! snippets, notes, script commands, custom window layouts, picked colors
//! and what the root search learned. Clipboard history stays behind: it is
//! large and private. Importing replaces what it contains and keeps the
//! rest, then reloads it all without a restart.

use std::{collections::BTreeMap, path::Path};

use anyhow::{Context as _, Result, bail};
use gpui_kit::{App, PathPromptOptions};
use serde::{Deserialize, Serialize};

use super::data_directory;
use crate::model::{Accessory, Action, Confirmation, Effect, Item, ItemId, RunHandler};

const FORMAT: &str = "gpui-kit-launcher-export";
const VERSION: u32 = 1;

/// The data files exported as they are, by name in the data directory.
const FILES: [&str; 9] = [
    "settings.json",
    "customizations.json",
    "quicklinks.json",
    "snippets.json",
    "colors.json",
    "emoji.json",
    "window-layouts.json",
    "focus.json",
    "usage.json",
];

/// Folders whose files are exported one by one.
const FOLDERS: [&str; 2] = ["notes", "script-commands"];

#[derive(Debug, Deserialize, Serialize)]
struct Export {
    format: String,
    version: u32,
    /// Unix seconds.
    exported_at: u64,
    /// JSON files, parsed, so the export reads as one JSON document.
    files: BTreeMap<String, serde_json::Value>,
    /// Text files in folders, by `folder/name`.
    #[serde(default)]
    documents: BTreeMap<String, String>,
}

fn export_to(path: &Path) -> Result<usize> {
    let directory = data_directory().context("this system has no data directory")?;
    let mut files = BTreeMap::new();
    for name in FILES {
        let Ok(text) = std::fs::read_to_string(directory.join(name)) else {
            continue;
        };
        let mut value: serde_json::Value =
            serde_json::from_str(&text).with_context(|| format!("{name} is not valid JSON"))?;
        // A running focus session belongs to this computer and this hour.
        if name == "focus.json"
            && let Some(object) = value.as_object_mut()
        {
            object.remove("session");
        }
        files.insert(name.to_owned(), value);
    }
    let mut documents = BTreeMap::new();
    for folder in FOLDERS {
        let Ok(entries) = std::fs::read_dir(directory.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Text only; anything else in the folder is not ours to copy.
            if let Ok(text) = std::fs::read_to_string(&path) {
                documents.insert(
                    format!("{folder}/{}", entry.file_name().to_string_lossy()),
                    text,
                );
            }
        }
    }
    let count = files.len() + documents.len();
    let export = Export {
        format: FORMAT.to_owned(),
        version: VERSION,
        exported_at: crate::search::now(),
        files,
        documents,
    };
    std::fs::write(path, serde_json::to_string_pretty(&export)?)
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(count)
}

/// Writes an export's files into the data directory; returns how many.
fn import_from(path: &Path) -> Result<usize> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    let export: Export = serde_json::from_str(&text).context("this is not a launcher export")?;
    if export.format != FORMAT {
        bail!("this is not a launcher export");
    }
    if export.version > VERSION {
        bail!("this export comes from a newer launcher");
    }
    let directory = data_directory().context("this system has no data directory")?;
    std::fs::create_dir_all(&directory)?;
    let mut written = 0;
    for (name, value) in &export.files {
        // Only the files an export can hold; a crafted export cannot write
        // anywhere else.
        if !FILES.contains(&name.as_str()) {
            continue;
        }
        std::fs::write(directory.join(name), serde_json::to_string_pretty(value)?)
            .with_context(|| format!("cannot write {name}"))?;
        written += 1;
    }
    for (name, text) in &export.documents {
        let Some((folder, file)) = name.split_once('/') else {
            continue;
        };
        let is_plain_name = !file.is_empty()
            && !file.contains(['/', '\\'])
            && file != "."
            && file != ".."
            && !file.contains(':');
        if !FOLDERS.contains(&folder) || !is_plain_name {
            continue;
        }
        std::fs::create_dir_all(directory.join(folder))?;
        std::fs::write(directory.join(folder).join(file), text)
            .with_context(|| format!("cannot write {name}"))?;
        written += 1;
    }
    Ok(written)
}

/// Reports the outcome in a HUD: the launcher has hidden behind the file
/// dialog by now.
fn report(result: Result<String>, cx: &mut App) {
    let message = match result {
        Ok(message) => message,
        Err(error) => format!("{error:#}"),
    };
    crate::shell::platform::show_hud(message.into(), cx);
}

fn export(cx: &mut App) {
    let directory = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default();
    let name = format!(
        "launcher-export-{}.json",
        chrono::Local::now().format("%Y-%m-%d")
    );
    let chosen = cx.prompt_for_new_path(&directory, Some(&name));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = chosen.await else {
            return;
        };
        let result =
            export_to(&path).map(|count| format!("Exported {count} files to {}", path.display()));
        cx.update(|cx| report(result, cx));
    })
    .detach();
}

fn import(cx: &mut App) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some("Import".into()),
    });
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        cx.update(|cx| {
            let result = import_from(&path).map(|count| {
                super::launcher::reload_data(cx);
                format!("Imported {count} files")
            });
            report(result, cx);
        });
    })
    .detach();
}

/// The root search commands.
pub fn commands() -> Vec<Item> {
    vec![
        Item::new(ItemId::new("system/export-data"), "Export Settings & Data")
            .with_icon("download")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("backup")
            .with_action(Action::new(
                "Export Settings & Data",
                Effect::Run(RunHandler::new(|(), _, cx| export(cx))),
            )),
        Item::new(ItemId::new("system/import-data"), "Import Settings & Data")
            .with_icon("upload")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("restore")
            .with_keyword("backup")
            .with_action(Action::new(
                "Import Settings & Data",
                Effect::Confirm(
                    Confirmation::new(
                        "Import settings and data?",
                        Effect::Run(RunHandler::new(|(), _, cx| import(cx))),
                    )
                    .with_message(
                        "What the file contains replaces the same settings, quicklinks, \
                         snippets and notes here.",
                    )
                    .with_confirm_title("Choose File…"),
                ),
            )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_import_refuses_other_files_and_paths() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.json");
        std::fs::write(
            &path,
            r#"{"format":"other","version":1,"exported_at":0,"files":{}}"#,
        )
        .unwrap();
        assert!(import_from(&path).is_err());
        std::fs::write(
            &path,
            r#"{"format":"gpui-kit-launcher-export","version":99,"exported_at":0,"files":{}}"#,
        )
        .unwrap();
        assert!(import_from(&path).is_err());
    }
}
