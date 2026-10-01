//! Floating Notes: quick notes in a small window that stays above other
//! windows, and a page to search them, as Raycast Notes does.
//!
//! Each note is a Markdown file in the launcher's data directory, so notes
//! survive the launcher and can be read by anything else. A note's title is
//! its first line.

mod window;

pub use window::{open_note, reload_open_note, toggle_window};

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext as _, Context, SharedString, Window};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        DetailModel, Effect, Image, Item, ItemId, ListModel, PageModel, PushHandler, RunHandler,
        Section,
    },
    pages::{self, Page, PageHandle},
};

/// A saved note.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// The file name without `.md`.
    pub id: String,
    pub text: String,
    /// Seconds since the Unix epoch.
    pub modified: u64,
}

impl Note {
    /// The first line, without Markdown heading marks.
    pub fn title(&self) -> String {
        title_of(&self.text)
    }
}

pub fn title_of(text: &str) -> String {
    text.lines()
        .map(|line| line.trim().trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
        .unwrap_or_else(|| "Untitled".to_owned())
}

pub fn directory() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("notes"))
}

fn note_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.md"))
}

/// Every note, most recently changed first.
pub fn list(directory: &Path) -> Vec<Note> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut notes: Vec<Note> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()? != "md" {
                return None;
            }
            let id = path.file_stem()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).ok()?;
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());
            Some(Note { id, text, modified })
        })
        .collect();
    notes.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.id.cmp(&a.id)));
    notes
}

pub fn read(directory: &Path, id: &str) -> Option<String> {
    std::fs::read_to_string(note_path(directory, id)).ok()
}

pub fn write(directory: &Path, id: &str, text: &str) -> Result<()> {
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let path = note_path(directory, id);
    let temporary = path.with_extension("md.tmp");
    std::fs::write(&temporary, text)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, &path).with_context(|| format!("cannot write {}", path.display()))
}

pub fn delete(directory: &Path, id: &str) -> Result<()> {
    std::fs::remove_file(note_path(directory, id)).context("cannot delete the note")
}

/// A new note's id: when it was created, which also sorts.
pub fn new_id() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S-%3f").to_string()
}

pub fn search_notes_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| {
        NotesPage {
            notes: directory()
                .map(|directory| list(&directory))
                .unwrap_or_default(),
        }
    })))
}

struct NotesPage {
    notes: Vec<Note>,
}

impl NotesPage {
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.notes = directory()
            .map(|directory| list(&directory))
            .unwrap_or_default();
        cx.notify();
    }

    fn item(&self, note: &Note, cx: &Context<Self>) -> Item {
        let page = cx.entity().downgrade();
        let id = note.id.clone();
        let open = {
            let id = id.clone();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                open_note(Some(id.clone()), cx)
            }))
        };
        let delete = Effect::Confirm(
            Confirmation::new(
                format!("Delete “{}”?", note.title()),
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    if let Some(directory) = directory() {
                        delete(&directory, &id).ok();
                    }
                    window::forget_note(&id, cx);
                    page.update(cx, |page, cx| page.reload(cx)).ok();
                })),
            )
            .with_message("The note cannot be recovered.")
            .with_confirm_title("Delete")
            .destructive(true),
        );
        let lines = note.text.lines().count();
        let words = note.text.split_whitespace().count();
        Item::new(ItemId::new(format!("note:{}", note.id)), note.title())
            .with_image(Image::Icon("sticky-note".into()))
            .with_keyword(note.text.chars().take(2000).collect::<String>())
            .with_accessory(Accessory::text(crate::format::relative_time(
                note.modified,
                crate::search::now(),
            )))
            .with_detail(DetailModel::new(note.text.clone()).with_metadata(
                crate::model::Metadata::new(
                    "Words",
                    crate::model::MetadataValue::Text(format!("{words} in {lines} lines").into()),
                ),
            ))
            .with_actions(
                ActionPanel::new()
                    .with_action(
                        Action::new("Open in Floating Notes", open)
                            .with_image(Image::Icon("sticky-note".into())),
                    )
                    .with_action(
                        Action::new("Copy Note", Effect::Copy(note.text.clone().into()))
                            .with_image(Image::Icon("copy".into()))
                            .with_shortcut("secondary-shift-c"),
                    )
                    .with_action(
                        Action::new("Paste Note", Effect::Paste(note.text.clone().into()))
                            .with_image(Image::Icon("clipboard-paste".into()))
                            .with_shortcut("secondary-enter"),
                    )
                    .with_section(
                        ActionSection::new()
                            .with_entry(ActionEntry::Action(new_note_action()))
                            .with_entry(ActionEntry::Action(
                                Action::new("Delete Note", delete)
                                    .with_image(Image::Icon("trash".into()))
                                    .with_style(ActionStyle::Destructive)
                                    .with_shortcut("ctrl-x"),
                            )),
                    ),
            )
    }
}

fn new_note_action() -> Action {
    Action::new(
        "Create Note",
        Effect::Run(RunHandler::new(|(), _, cx| open_note(None, cx))),
    )
    .with_image(Image::Icon("plus".into()))
    .with_shortcut("secondary-n")
}

impl Page for NotesPage {
    fn title(&self) -> SharedString {
        "Notes".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let create = self.notes.is_empty().then(|| {
            Item::new(ItemId::new("create-note"), "Create Note")
                .with_icon("notebook-pen")
                .with_subtitle("Opens a new note in Floating Notes")
                .with_action(new_note_action())
        });
        ListModel::new()
            .with_section(Section::new().with_items(create))
            .with_placeholder("Search notes…")
            .with_showing_detail(!self.notes.is_empty())
            .with_empty_title("No notes yet")
            .with_empty_description("Create Note opens a new one in Floating Notes.")
            .with_section(
                Section::new()
                    .with_title("Notes")
                    .with_subtitle(self.notes.len().to_string())
                    .with_items(self.notes.iter().map(|note| self.item(note, cx))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}

    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }
}

/// The root search commands for notes.
pub fn commands() -> Vec<Item> {
    let command = |id: &str, title: &str, icon: &str| {
        Item::new(ItemId::new(format!("system/{id}")), title.to_owned())
            .with_icon(icon.to_owned())
            .with_accessory(Accessory::text("Command"))
            .with_keyword("notes")
            .with_keyword("floating")
    };
    vec![
        command(
            "toggle-floating-notes",
            "Toggle Floating Notes",
            "sticky-note",
        )
        .with_action(Action::new(
            "Toggle Floating Notes",
            Effect::Run(RunHandler::new(|(), _, cx| toggle_window(cx))),
        )),
        command("create-note", "Create Note", "notebook-pen")
            .with_keyword("new note")
            .with_action(new_note_action()),
        command("search-notes", "Search Notes", "sticky-note").with_action(Action::new(
            "Search Notes",
            Effect::Push(PushHandler::new(search_notes_page)),
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notes_round_trip_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let directory = directory.path().join("notes");
        assert!(list(&directory).is_empty());
        write(&directory, "a", "# Groceries\n\nmilk").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(&directory, "b", "\n\n  Ideas  \nmore").unwrap();
        let notes = list(&directory);
        assert_eq!(
            notes.iter().map(Note::title).collect::<Vec<_>>(),
            ["Ideas", "Groceries"]
        );
        assert_eq!(
            read(&directory, "a").as_deref(),
            Some("# Groceries\n\nmilk")
        );
        delete(&directory, "a").unwrap();
        assert_eq!(list(&directory).len(), 1);
        assert_eq!(title_of("   \n"), "Untitled");
    }
}
