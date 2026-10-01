//! The quicklink pages: create or edit one, fill in its arguments, and the
//! list of every quicklink.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Subscription, Window};

use super::{Quicklink, QuicklinkStore, is_url, store};
use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Control, Effect, Field, FormHandler, FormModel, FormValue, FormValues, Image, Item, ItemId,
        ListModel, PageModel, PushHandler, RunHandler, Section, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::perform,
};

const NAME: &str = "name";
const LINK: &str = "link";
const PLACEHOLDER_HELP: &str = "Use {argument} for text typed when it opens (name several with \
     {argument name=\"city\"}), {clipboard} for what was copied, {selection} for the text \
     selected before the launcher opened, and {date} or {time}.";

fn quicklink_store(cx: &App) -> Result<Entity<QuicklinkStore>> {
    store(cx).ok_or_else(|| anyhow!("quicklinks are not loaded"))
}

/// The root search's and Search Quicklinks' rows, one per quicklink.
pub fn quicklink_items(quicklinks: &[Quicklink]) -> Vec<Item> {
    quicklinks.iter().map(item).collect()
}

fn item(quicklink: &Quicklink) -> Item {
    let edit = quicklink.clone();
    let delete = quicklink.id().to_owned();
    let actions = ActionPanel::new()
        .with_action(open_action(quicklink, None))
        .with_action(
            Action::new(
                "Copy Link",
                Effect::Copy(quicklink.link().to_owned().into()),
            )
            .with_image(Image::Icon("copy".into()))
            .with_shortcut("secondary-shift-c"),
        )
        .with_action(
            Action::new(
                "Edit Quicklink",
                Effect::Push(PushHandler::new(move |_, cx| {
                    let page = QuicklinkFormPage::new(Some(edit.clone()), cx)?;
                    Ok(pages::handle(cx.new(|_| page)))
                })),
            )
            .with_image(Image::Icon("pencil".into()))
            .with_shortcut("secondary-e"),
        )
        .with_section(
            ActionSection::new().with_entry(ActionEntry::Action(
                Action::new(
                    "Delete Quicklink",
                    Effect::Confirm(
                        Confirmation::new(
                            format!("Delete “{}”?", quicklink.name()),
                            Effect::Run(RunHandler::new(move |(), _, cx| {
                                if let Ok(store) = quicklink_store(cx) {
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
    Item::new(
        ItemId::new(format!("quicklink/{}", quicklink.id())),
        quicklink.name().to_owned(),
    )
    .with_subtitle(quicklink.link().to_owned())
    .with_icon(match is_url(quicklink.link()) {
        true => "link",
        false => "folder",
    })
    .with_accessory(Accessory::text("Quicklink"))
    .with_actions(actions)
}

/// Opens `quicklink`. Without arguments it opens at once; with arguments it
/// asks for them, unless `query` already holds the single one it needs.
fn open_action(quicklink: &Quicklink, query: Option<String>) -> Action {
    let arguments = quicklink.arguments();
    let quicklink = quicklink.clone();
    let effect = match (arguments.len(), query) {
        (0, _) => Effect::Run(RunHandler::new(move |(), _, cx| {
            open(&quicklink, &[], cx);
        })),
        (1, Some(query)) => Effect::Run(RunHandler::new(move |(), _, cx| {
            let values = [(arguments[0].name.clone(), query.clone())];
            open(&quicklink, &values, cx);
        })),
        _ => Effect::Push(PushHandler::new(move |_, cx| {
            Ok(pages::handle(cx.new(|_| ArgumentsPage {
                quicklink: quicklink.clone(),
            })))
        })),
    };
    Action::new("Open Quicklink", effect).with_image(Image::Icon("external-link".into()))
}

fn open(quicklink: &Quicklink, values: &[(String, String)], cx: &mut App) {
    let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
    perform(quicklink.open_effect(values, clipboard.as_deref()), cx);
}

/// The quicklinks that take one argument, offered with the root query.
pub fn fallback_items(quicklinks: &[Quicklink], query: &str) -> Vec<Item> {
    quicklinks
        .iter()
        .filter(|quicklink| quicklink.arguments().len() == 1)
        .map(|quicklink| {
            Item::new(
                ItemId::new(format!("fallback/quicklink/{}", quicklink.id())),
                quicklink.name().to_owned(),
            )
            .with_icon("link")
            .with_accessory(Accessory::text("Quicklink"))
            .with_action(open_action(quicklink, Some(query.to_owned())))
        })
        .collect()
}

fn text(values: &FormValues, id: &str) -> String {
    match values.get(id) {
        Some(FormValue::Text(text)) => text.trim().to_owned(),
        Some(FormValue::Empty | FormValue::Bool(_) | FormValue::List(_)) | None => String::new(),
    }
}

/// Asks for a quicklink's arguments, then opens it.
struct ArgumentsPage {
    quicklink: Quicklink,
}

impl Page for ArgumentsPage {
    fn title(&self) -> SharedString {
        self.quicklink.name().to_owned().into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let arguments = self.quicklink.arguments();
        let quicklink = self.quicklink.clone();
        let names: Vec<String> = arguments.iter().map(|a| a.name.clone()).collect();
        let submit = FormHandler::new(move |values, _, cx| {
            let values: Vec<(String, String)> = names
                .iter()
                .map(|name| (name.clone(), text(&values, &format!("argument:{name}"))))
                .collect();
            open(&quicklink, &values, cx);
        });
        arguments
            .iter()
            .fold(
                FormModel::new().with_actions(
                    ActionPanel::new()
                        .with_action(Action::new("Open Quicklink", Effect::SubmitForm(submit))),
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

/// Creates a quicklink, or edits one.
struct QuicklinkFormPage {
    store: Entity<QuicklinkStore>,
    editing: Option<Quicklink>,
    /// What was submitted last, shown again beside its errors.
    draft: Option<FormValues>,
    errors: HashMap<&'static str, &'static str>,
}

impl QuicklinkFormPage {
    fn new(editing: Option<Quicklink>, cx: &App) -> Result<Self> {
        Ok(Self {
            store: quicklink_store(cx)?,
            editing,
            draft: None,
            errors: HashMap::new(),
        })
    }

    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let (name, link) = (text(&values, NAME), text(&values, LINK));
        self.errors.clear();
        if name.is_empty() {
            self.errors.insert(NAME, "Give the quicklink a name");
        }
        if link.is_empty() {
            self.errors.insert(LINK, "Enter a link or a path");
        }
        if !self.errors.is_empty() {
            self.draft = Some(values);
            cx.notify();
            return;
        }
        let quicklink = match &self.editing {
            Some(editing) => editing.with_contents(name.clone(), link),
            None => Quicklink::new(name.clone(), link),
        };
        self.store.update(cx, |store, cx| store.save(quicklink, cx));
        perform(Effect::Pop, cx);
        perform(
            Effect::ShowToast(Toast::new(ToastStyle::Success, format!("Saved “{name}”"))),
            cx,
        );
    }

    fn field(&self, id: &'static str, title: &str, saved: &str, placeholder: &str) -> Field {
        let value = match &self.draft {
            Some(draft) => text(draft, id),
            None => saved.to_owned(),
        };
        let field = Field::new(
            id,
            title.to_owned(),
            Control::Text {
                placeholder: Some(placeholder.to_owned().into()),
                value: value.into(),
            },
        );
        match self.errors.get(id) {
            Some(error) => field.with_error(*error),
            None => field,
        }
    }
}

impl Page for QuicklinkFormPage {
    fn title(&self) -> SharedString {
        match self.editing {
            Some(_) => "Edit Quicklink".into(),
            None => "Create Quicklink".into(),
        }
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let (name, link) = self
            .editing
            .as_ref()
            .map(|quicklink| (quicklink.name(), quicklink.link()))
            .unwrap_or_default();
        FormModel::new()
            .with_field(self.field(NAME, "Name", name, "GitHub Search"))
            .with_field(
                self.field(LINK, "Link", link, "https://github.com/search?q={argument}")
                    .with_info(PLACEHOLDER_HELP),
            )
            .with_actions(
                ActionPanel::new()
                    .with_action(Action::new("Save Quicklink", Effect::SubmitForm(submit))),
            )
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

/// The Create Quicklink command.
pub fn create_quicklink_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let page = QuicklinkFormPage::new(None, cx)?;
    Ok(pages::handle(cx.new(|_| page)))
}

/// The Create Quicklink form filled in, as an extension's
/// `Action.create_quicklink` asks; nothing is saved until the user submits.
pub fn create_quicklink_page_with(name: &str, link: &str, cx: &mut App) -> Result<PageHandle> {
    let mut page = QuicklinkFormPage::new(None, cx)?;
    page.draft = Some(
        FormValues::new()
            .with(NAME, FormValue::Text(name.to_owned().into()))
            .with(LINK, FormValue::Text(link.to_owned().into())),
    );
    Ok(pages::handle(cx.new(|_| page)))
}

/// The Search Quicklinks command.
pub fn search_quicklinks_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let store = quicklink_store(cx)?;
    Ok(pages::handle(cx.new(|cx| SearchQuicklinksPage {
        _subscription: cx.observe(&store, |_, _, cx| cx.notify()),
        store,
    })))
}

struct SearchQuicklinksPage {
    store: Entity<QuicklinkStore>,
    _subscription: Subscription,
}

impl Page for SearchQuicklinksPage {
    fn title(&self) -> SharedString {
        "Quicklinks".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let create = Item::new(ItemId::new("quicklink-create"), "Create Quicklink")
            .with_icon("plus")
            .with_action(Action::new(
                "Create Quicklink",
                Effect::Push(PushHandler::new(create_quicklink_page)),
            ));
        ListModel::new()
            .with_placeholder("Search quicklinks…")
            .with_section(
                Section::new()
                    .with_title("Quicklinks")
                    .with_items(quicklink_items(self.store.read(cx).quicklinks())),
            )
            .with_section(Section::new().with_item(create))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
