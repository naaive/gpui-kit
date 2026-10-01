//! The snippet pages: create or edit one, fill in its arguments, and the
//! list of every snippet with a preview.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Window};

use super::{Snippet, SnippetStore, store};
use crate::{
    format::code_block,
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Control, DetailModel, Effect, Field, FormHandler, FormModel, FormValue, FormValues, Image,
        Item, ItemId, ListModel, Metadata, MetadataValue, PageModel, PushHandler, RunHandler,
        Section, Toast, ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

const NAME: &str = "name";
const TEXT: &str = "text";
const KEYWORD: &str = "keyword";
const PLACEHOLDER_HELP: &str = "Placeholders: {clipboard}, {selection}, {date}, {time}, \
     {datetime}, {uuid}, and {argument name=\"…\"} for text asked for when it is pasted.";

fn snippet_store(cx: &App) -> Result<Entity<SnippetStore>> {
    store(cx).ok_or_else(|| anyhow!("snippets are not loaded"))
}

/// What to do with a snippet's text once its placeholders are filled in.
#[derive(Clone, Copy, PartialEq)]
enum Use {
    Paste,
    Copy,
}

/// Fills in `snippet` and pastes or copies it; asks for arguments first
/// when it has any.
fn use_effect(snippet: &Snippet, how: Use) -> Effect {
    let snippet = snippet.clone();
    match snippet.arguments().is_empty() {
        true => Effect::Run(RunHandler::new(move |(), _, cx| {
            finish(&snippet, &[], how, cx);
        })),
        false => Effect::Push(PushHandler::new(move |_, cx| {
            Ok(pages::handle(cx.new(|_| ArgumentsPage {
                snippet: snippet.clone(),
                how,
            })))
        })),
    }
}

fn finish(snippet: &Snippet, values: &[(String, String)], how: Use, cx: &mut App) {
    let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
    let text: SharedString = snippet.expand(values, clipboard.as_deref()).into();
    perform(
        match how {
            Use::Paste => Effect::Paste(text),
            Use::Copy => Effect::Copy(text),
        },
        cx,
    );
}

/// One row per snippet, for the root search and Search Snippets.
pub fn snippet_items(snippets: &[Snippet]) -> Vec<Item> {
    snippets.iter().map(item).collect()
}

fn item(snippet: &Snippet) -> Item {
    let edit = snippet.clone();
    let delete = snippet.id().to_owned();
    let preview = snippet
        .text()
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned();
    let actions = ActionPanel::new()
        .with_action(
            Action::new("Paste Snippet", use_effect(snippet, Use::Paste))
                .with_image(Image::Icon("clipboard-paste".into())),
        )
        .with_action(
            Action::new("Copy to Clipboard", use_effect(snippet, Use::Copy))
                .with_image(Image::Icon("copy".into())),
        )
        .with_action(
            Action::new(
                "Edit Snippet",
                Effect::Push(PushHandler::new(move |_, cx| {
                    let page = SnippetFormPage::new(Some(edit.clone()), cx)?;
                    Ok(pages::handle(cx.new(|_| page)))
                })),
            )
            .with_image(Image::Icon("pencil".into()))
            .with_shortcut("secondary-e"),
        )
        .with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new(
                    "Delete Snippet",
                    Effect::Confirm(
                        Confirmation::new(
                            format!("Delete “{}”?", snippet.name()),
                            Effect::Run(RunHandler::new(move |(), _, cx| {
                                if let Ok(store) = snippet_store(cx) {
                                    store.update(cx, |store, cx| store.remove(&delete, cx));
                                }
                            })),
                        )
                        .with_confirm_title("Delete")
                        .destructive(true),
                    ),
                )
                .with_image(Image::Icon("trash".into()))
                .with_shortcut("secondary-shift-x")
                .with_style(ActionStyle::Destructive),
            )),
        );
    let label =
        |label: &str, value: String| Metadata::new(label, MetadataValue::Text(value.into()));
    let detail = DetailModel::new(code_block(snippet.text()))
        .with_metadata(label("Name", snippet.name().to_owned()));
    let detail = match snippet.keyword().is_empty() {
        true => detail,
        false => detail.with_metadata(label("Keyword", snippet.keyword().to_owned())),
    }
    .with_metadata(label(
        "Characters",
        snippet.text().chars().count().to_string(),
    ));
    let item = Item::new(
        ItemId::new(format!("snippet/{}", snippet.id())),
        snippet.name().to_owned(),
    )
    .with_subtitle(preview)
    .with_icon("text-cursor-input")
    .with_detail(detail)
    .with_actions(actions);
    match snippet.keyword().is_empty() {
        true => item.with_accessory(Accessory::text("Snippet")),
        false => item
            .with_keyword(snippet.keyword().to_owned())
            .with_accessory(Accessory::tag(snippet.keyword().to_owned(), Tone::Neutral)),
    }
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.to_string(),
        Some(FormValue::Empty | FormValue::Bool(_)) | None => String::new(),
    }
}

/// Asks for a snippet's arguments, then pastes or copies it.
struct ArgumentsPage {
    snippet: Snippet,
    how: Use,
}

impl Page for ArgumentsPage {
    fn title(&self) -> SharedString {
        self.snippet.name().to_owned().into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let arguments = self.snippet.arguments();
        let (snippet, how) = (self.snippet.clone(), self.how);
        let names: Vec<String> = arguments.iter().map(|a| a.name.clone()).collect();
        let submit = FormHandler::new(move |values, _, cx| {
            let values: Vec<(String, String)> = names
                .iter()
                .map(|name| (name.clone(), text(&values, &format!("argument:{name}"))))
                .collect();
            finish(&snippet, &values, how, cx);
        });
        let title = match how {
            Use::Paste => "Paste Snippet",
            Use::Copy => "Copy to Clipboard",
        };
        arguments
            .iter()
            .fold(
                FormModel::new().with_actions(
                    ActionPanel::new().with_action(Action::new(title, Effect::SubmitForm(submit))),
                ),
                |form, argument| {
                    form.with_field(Field::new(
                        format!("argument:{}", argument.name),
                        argument.title(),
                        Control::Text {
                            placeholder: (!argument.default.is_empty())
                                .then(|| argument.default.clone().into()),
                            value: SharedString::default(),
                        },
                    ))
                },
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// Creates a snippet, or edits one.
struct SnippetFormPage {
    store: Entity<SnippetStore>,
    editing: Option<Snippet>,
    draft: Option<FormValues>,
    errors: HashMap<&'static str, &'static str>,
}

impl SnippetFormPage {
    fn new(editing: Option<Snippet>, cx: &App) -> Result<Self> {
        Ok(Self {
            store: snippet_store(cx)?,
            editing,
            draft: None,
            errors: HashMap::new(),
        })
    }

    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let name = text(&values, NAME).trim().to_owned();
        let body = text(&values, TEXT);
        let keyword = text(&values, KEYWORD).trim().to_owned();
        self.errors.clear();
        if name.is_empty() {
            self.errors.insert(NAME, "Give the snippet a name");
        }
        if body.trim().is_empty() {
            self.errors.insert(TEXT, "Enter the text to paste");
        }
        if keyword.contains(char::is_whitespace) {
            self.errors.insert(KEYWORD, "A keyword has no spaces");
        } else if self
            .store
            .read(cx)
            .keyword_taken(&keyword, self.editing.as_ref().map(Snippet::id))
        {
            self.errors
                .insert(KEYWORD, "Another snippet uses this keyword");
        }
        if !self.errors.is_empty() {
            self.draft = Some(values);
            cx.notify();
            return;
        }
        let snippet = match &self.editing {
            Some(editing) => editing.with_contents(name.clone(), body, keyword),
            None => Snippet::new(name.clone(), body, keyword),
        };
        self.store.update(cx, |store, cx| store.save(snippet, cx));
        perform(Effect::Pop, cx);
        perform(
            Effect::ShowToast(Toast::new(ToastStyle::Success, format!("Saved “{name}”"))),
            cx,
        );
    }

    fn value(&self, id: &'static str, saved: &str) -> SharedString {
        match &self.draft {
            Some(draft) => text(draft, id).into(),
            None => saved.to_owned().into(),
        }
    }

    fn with_error(&self, field: Field) -> Field {
        match self.errors.get(field.id().as_ref()) {
            Some(error) => field.with_error(*error),
            None => field,
        }
    }
}

impl Page for SnippetFormPage {
    fn title(&self) -> SharedString {
        match self.editing {
            Some(_) => "Edit Snippet".into(),
            None => "Create Snippet".into(),
        }
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let (name, body, keyword) = self
            .editing
            .as_ref()
            .map(|snippet| (snippet.name(), snippet.text(), snippet.keyword()))
            .unwrap_or_default();
        FormModel::new()
            .with_field(self.with_error(Field::new(
                NAME,
                "Name",
                Control::Text {
                    placeholder: Some("Email signature".into()),
                    value: self.value(NAME, name),
                },
            )))
            .with_field(
                self.with_error(Field::new(
                    TEXT,
                    "Snippet",
                    Control::TextArea {
                        placeholder: Some("Best regards,\nAda".into()),
                        value: self.value(TEXT, body),
                    },
                ))
                .with_info(PLACEHOLDER_HELP),
            )
            .with_field(
                self.with_error(Field::new(
                    KEYWORD,
                    "Keyword",
                    Control::Text {
                        placeholder: Some("!sig".into()),
                        value: self.value(KEYWORD, keyword),
                    },
                ))
                .with_info("Typing the keyword in the search puts the snippet first."),
            )
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Save Snippet", Effect::SubmitForm(submit))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// The Create Snippet command.
pub fn create_snippet_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let page = SnippetFormPage::new(None, cx)?;
    Ok(pages::handle(cx.new(|_| page)))
}

/// The Search Snippets command.
pub fn search_snippets_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = snippet_store(cx)?;
    Ok(pages::handle(cx.new(|cx| SearchSnippetsPage {
        _subscription: cx.observe(&store, |_, _, cx| cx.notify()),
        store,
    })))
}

struct SearchSnippetsPage {
    store: Entity<SnippetStore>,
    _subscription: Subscription,
}

impl Page for SearchSnippetsPage {
    fn title(&self) -> SharedString {
        "Snippets".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let snippets = self.store.read(cx).snippets();
        let create = Item::new(ItemId::new("snippet-create"), "Create Snippet")
            .with_icon("plus")
            .with_action(Action::new(
                "Create Snippet",
                Effect::Push(PushHandler::new(create_snippet_page)),
            ));
        ListModel::new()
            .with_placeholder("Search snippets…")
            .with_showing_detail(!snippets.is_empty())
            .with_section(
                Section::new()
                    .with_title("Snippets")
                    .with_items(snippet_items(snippets)),
            )
            .with_section(Section::new().with_item(create))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
