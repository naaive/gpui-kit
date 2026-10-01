//! The Clipboard History command: past copies beside a preview, to paste or
//! copy again, pin, or delete.

use anyhow::Result;
use chrono::Datelike as _;
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Window};

use super::{ClipboardStore, Content, ContentType, Entry, clipboard_item, store};
use crate::{
    format::{code_block, format_bytes, format_time, local, relative_time},
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Choice,
        Confirmation, DetailModel, Dropdown, Effect, Image, Item, ItemId, ListModel, Metadata,
        MetadataValue, PageModel, RunHandler, Section, TextHandler,
    },
    pages::{self, Page, PageHandle},
    search::now,
};

/// The preview shows at most this much text; the whole entry is still pasted.
const PREVIEW_CHARS: usize = 4000;
/// A row's title is the first line, cut to this length.
const TITLE_CHARS: usize = 120;
const ALL_TYPES: &str = "all";

/// Builds the page, as the root search's Clipboard History command pushes it.
pub fn clipboard_history_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = store(cx).ok_or_else(|| anyhow::anyhow!("clipboard history is not running"))?;
    Ok(pages::handle(
        cx.new(|cx| ClipboardHistoryPage::new(store, cx)),
    ))
}

pub struct ClipboardHistoryPage {
    store: Entity<ClipboardStore>,
    query: String,
    filter: Option<ContentType>,
    /// The list for the current entries, query and filter; the window asks
    /// for the model every frame.
    list: Option<ListModel>,
    _subscription: Subscription,
}

impl ClipboardHistoryPage {
    fn new(store: Entity<ClipboardStore>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&store, |page, _, cx| page.invalidate(cx));
        Self {
            store,
            query: String::new(),
            filter: None,
            list: None,
            _subscription: subscription,
        }
    }

    fn invalidate(&mut self, cx: &mut Context<Self>) {
        self.list = None;
        cx.notify();
    }

    fn build_list(&self, cx: &mut Context<Self>) -> ListModel {
        let page = cx.entity().downgrade();
        let filter = Dropdown::new("Filter by type")
            .with_choice(Choice::new(ALL_TYPES, "All Types"))
            .with_value(self.filter.map_or(ALL_TYPES, ContentType::value))
            .with_on_change(TextHandler::new(move |value, _, cx| {
                page.update(cx, |page, cx| {
                    page.filter = ContentType::parse(&value);
                    page.invalidate(cx);
                })
                .ok();
            }));
        let filter = ContentType::ALL.into_iter().fold(filter, |filter, kind| {
            filter.with_choice(Choice::new(kind.value(), kind.plural_title()))
        });

        let terms: Vec<String> = self
            .query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let entries: Vec<&Entry> = self
            .store
            .read(cx)
            .entries()
            .iter()
            .filter(|entry| {
                self.filter
                    .is_none_or(|kind| entry.content().kind() == kind)
            })
            .filter(|entry| {
                terms.is_empty() || {
                    let text = entry.searchable_text().to_lowercase();
                    terms.iter().all(|term| text.contains(term.as_str()))
                }
            })
            .collect();

        // Pinned entries lead, wherever their copies fall in time; the rest
        // are grouped by the day they were copied.
        let now = now();
        let (pinned, recent): (Vec<&Entry>, Vec<&Entry>) =
            entries.into_iter().partition(|entry| entry.is_pinned());
        let mut sections: Vec<(SharedString, Vec<Item>)> = Vec::new();
        if !pinned.is_empty() {
            sections.push((
                "Pinned".into(),
                pinned.iter().map(|entry| self.item(entry, now)).collect(),
            ));
        }
        for entry in recent {
            let title = day_title(entry.copied_at(), now);
            let item = self.item(entry, now);
            match sections.last_mut() {
                Some((last, items)) if *last == title => items.push(item),
                _ => sections.push((title, vec![item])),
            }
        }

        let (empty_title, empty_description) = match self.store.read(cx).entries().is_empty() {
            true => (
                "Nothing copied yet",
                "Text, links, images and files you copy appear here.",
            ),
            false => ("No matching entries", "Try another search or type."),
        };
        sections.into_iter().fold(
            ListModel::new()
                .with_placeholder("Type to filter entries…")
                .with_filtering(false)
                .with_showing_detail(true)
                .with_dropdown(filter)
                .with_empty_title(empty_title)
                .with_empty_description(empty_description),
            |list, (title, items)| {
                list.with_section(Section::new().with_title(title).with_items(items))
            },
        )
    }

    fn item(&self, entry: &Entry, now: u64) -> Item {
        let content = entry.content();
        let kind = content.kind();
        let image = match content {
            Content::Image { path, .. } => Image::File(path.clone()),
            Content::Text { .. } | Content::Files { .. } => Image::Icon(kind.icon().into()),
        };
        Item::new(ItemId::new(entry.id().to_owned()), title(content))
            .with_image(image)
            .with_accessory(
                Accessory::text(relative_time(entry.copied_at(), now))
                    .with_tooltip(format_time(entry.copied_at())),
            )
            .with_detail(detail(entry))
            .with_actions(self.actions(entry))
    }

    fn actions(&self, entry: &Entry) -> ActionPanel {
        let content = entry.content();
        let (paste, copy) = match content {
            Content::Text { text } => (
                Effect::Paste(text.clone().into()),
                Effect::Copy(text.clone().into()),
            ),
            // Images are read from disk only when the action runs, not each
            // time the list is built.
            Content::Image { .. } | Content::Files { .. } => {
                let later = |paste: bool| {
                    let content = content.clone();
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        let effect = match (clipboard_item(&content), paste) {
                            (Some(item), true) => Effect::PasteItem(item),
                            (Some(item), false) => Effect::CopyItem(item),
                            (None, _) => Effect::ShowToast(crate::model::Toast::new(
                                crate::model::ToastStyle::Failure,
                                "The copied file no longer exists",
                            )),
                        };
                        crate::shell::launcher::perform(effect, cx);
                    }))
                };
                (later(true), later(false))
            }
        };
        let mut actions = ActionPanel::new()
            .with_action(
                Action::new("Paste to Active App", paste)
                    .with_image(Image::Icon("clipboard-paste".into())),
            )
            .with_action(
                Action::new("Copy to Clipboard", copy).with_image(Image::Icon("copy".into())),
            );
        actions = match content {
            Content::Text { text } if content.kind() == ContentType::Link => actions.with_action(
                Action::new(
                    "Open in Browser",
                    Effect::OpenUrl(text.trim().to_owned().into()),
                )
                .with_image(Image::Icon("globe".into()))
                .with_shortcut("secondary-o"),
            ),
            Content::Image { path, .. } => actions.with_action(
                Action::new("Open Image", Effect::OpenPath(path.clone()))
                    .with_image(Image::Icon("external-link".into()))
                    .with_shortcut("secondary-o"),
            ),
            Content::Files { paths } => match paths.as_slice() {
                [path] => actions
                    .with_action(
                        Action::new("Open", Effect::OpenPath(path.clone()))
                            .with_image(Image::Icon("external-link".into()))
                            .with_shortcut("secondary-o"),
                    )
                    .with_action(
                        Action::new(
                            crate::sources::applications::REVEAL_TITLE,
                            Effect::RevealPath(path.clone()),
                        )
                        .with_image(Image::Icon("folder-open".into()))
                        .with_shortcut("secondary-shift-f"),
                    ),
                _ => actions,
            },
            Content::Text { .. } => actions,
        };

        let pinned = entry.is_pinned();
        let id = entry.id().to_owned();
        let store = self.store.clone();
        actions = actions.with_action(
            Action::new(
                match pinned {
                    true => "Unpin Entry",
                    false => "Pin Entry",
                },
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    store.update(cx, |store, cx| store.set_pinned(&id, !pinned, cx));
                })),
            )
            .with_image(Image::Icon(
                match pinned {
                    true => "pin-off",
                    false => "pin",
                }
                .into(),
            ))
            .with_shortcut("secondary-shift-p"),
        );

        if let Some(text) = entry.recognized_text() {
            actions = actions.with_action(
                Action::new("Copy Text in Image", Effect::Copy(text.to_owned().into()))
                    .with_image(Image::Icon("scan-text".into()))
                    .with_shortcut("secondary-shift-t"),
            );
        }
        if let Some(source) = entry.source() {
            let source = source.to_owned();
            actions = actions.with_action(
                Action::new(
                    format!("Ignore Copies from {source}"),
                    Effect::Run(RunHandler::new(move |(), window, cx| {
                        let settings = crate::shell::launcher::settings(cx)
                            .with_clipboard_ignored_app(&source);
                        crate::shell::launcher::update_settings(settings, window, cx).ok();
                        crate::shell::launcher::perform(
                            Effect::ShowToast(crate::model::Toast::new(
                                crate::model::ToastStyle::Success,
                                format!("Copies from {source} are no longer recorded"),
                            )),
                            cx,
                        );
                    })),
                )
                .with_image(Image::Icon("eye-off".into())),
            );
        }

        let id = entry.id().to_owned();
        let store = self.store.clone();
        let delete = Action::new(
            "Delete Entry",
            Effect::Run(RunHandler::new(move |(), _, cx| {
                store.update(cx, |store, cx| store.remove(&id, cx));
            })),
        )
        .with_image(Image::Icon("trash".into()))
        .with_shortcut("secondary-shift-x")
        .with_style(ActionStyle::Destructive);
        let store = self.store.clone();
        let delete_all = Action::new(
            "Delete All Entries…",
            Effect::Confirm(
                Confirmation::new(
                    "Delete all clipboard entries?",
                    Effect::Run(RunHandler::new(move |(), _, cx| {
                        store.update(cx, |store, cx| store.clear(cx));
                    })),
                )
                .with_message("Pinned entries are kept. This can’t be undone.")
                .with_confirm_title("Delete All")
                .destructive(true),
            ),
        )
        .with_image(Image::Icon("trash".into()))
        .with_style(ActionStyle::Destructive);
        actions.with_section(
            ActionSection::new()
                .with_entry(ActionEntry::Action(delete))
                .with_entry(ActionEntry::Action(delete_all)),
        )
    }
}

impl Page for ClipboardHistoryPage {
    fn title(&self) -> SharedString {
        "Clipboard History".into()
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
            self.invalidate(cx);
        }
    }
}

/// The row title: the first line of text, the image size, or the file name.
fn title(content: &Content) -> String {
    match content {
        Content::Text { text } => {
            let line = text
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or_default();
            let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
            match line.chars().count() > TITLE_CHARS {
                true => format!("{}…", line.chars().take(TITLE_CHARS).collect::<String>()),
                false => line,
            }
        }
        Content::Image { width, height, .. } => format!("Image ({width}×{height})"),
        Content::Files { paths } => match paths.as_slice() {
            [path] => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            paths => format!("{} Files", paths.len()),
        },
    }
}

fn detail(entry: &Entry) -> DetailModel {
    let content = entry.content();
    let detail = match content {
        Content::Text { text } => {
            let preview: String = text.chars().take(PREVIEW_CHARS).collect();
            let more = match text.chars().count() > PREVIEW_CHARS {
                true => "\n\n*…the rest is pasted but not shown.*",
                false => "",
            };
            DetailModel::new(format!("{}{more}", code_block(&preview)))
        }
        Content::Image { path, .. } => DetailModel::new("").with_image(path.clone()),
        Content::Files { paths } => DetailModel::new(
            paths
                .iter()
                .map(|path| format!("- `{}`", path.display()))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    };
    let label =
        |label: &str, value: String| Metadata::new(label, MetadataValue::Text(value.into()));
    let detail = detail.with_metadata(label("Content Type", content.kind().title().into()));
    let detail = match content {
        Content::Text { text } => detail
            .with_metadata(label("Characters", text.chars().count().to_string()))
            .with_metadata(label("Words", text.split_whitespace().count().to_string()))
            .with_metadata(label("Lines", text.lines().count().max(1).to_string())),
        Content::Image {
            width,
            height,
            bytes,
            ..
        } => detail
            .with_metadata(label("Dimensions", format!("{width} × {height}")))
            .with_metadata(label("Image Size", format_bytes(*bytes))),
        Content::Files { paths } => detail.with_metadata(label("Files", paths.len().to_string())),
    };
    let detail = match entry.source() {
        Some(source) => detail.with_metadata(label("Copied From", source.to_owned())),
        None => detail,
    };
    let detail = match entry.recognized_text() {
        Some(text) => detail.with_metadata(label(
            "Text in Image",
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(200)
                .collect(),
        )),
        None => detail,
    };
    detail
        .with_metadata(label("Times Copied", entry.copies().to_string()))
        .with_metadata(label("Last Copied", format_time(entry.copied_at())))
}

/// Which day's section an entry falls in.
fn day_title(copied_at: u64, now: u64) -> SharedString {
    let (Some(copied), Some(today)) = (local(copied_at), local(now)) else {
        return "Earlier".into();
    };
    let days = today
        .date_naive()
        .signed_duration_since(copied.date_naive())
        .num_days();
    match days {
        ..=0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => copied.format("%A").to_string().into(),
        _ if copied.year() == today.year() => copied.format("%B %-d").to_string().into(),
        _ => copied.format("%B %-d, %Y").to_string().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_titles_and_fences() {
        assert_eq!(
            title(&Content::Text {
                text: "\n\n  first   line \nsecond".into()
            }),
            "first line"
        );
        assert_eq!(
            title(&Content::Files {
                paths: vec!["a".into(), "b".into()]
            }),
            "2 Files"
        );
        assert_eq!(code_block("a ``` b"), "````text\na ``` b\n````");
        assert_eq!(code_block("plain"), "```text\nplain\n```");
    }

    #[test]
    fn test_relative_time_and_sizes() {
        assert_eq!(relative_time(100, 130), "Just now");
        assert_eq!(relative_time(0, 600), "10m ago");
        assert_eq!(relative_time(0, 7200), "2h ago");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(day_title(now(), now()), "Today");
    }
}
