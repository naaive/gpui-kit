//! Setting an item's alias or hotkey.

use anyhow::{Result, anyhow};
use gpui_kit::{App, AppContext as _, Context, Entity, SharedString, Window};

use super::{Customizations, store};
use crate::{
    model::{
        Action, ActionPanel, Control, Effect, Field, FormHandler, FormModel, FormValue, FormValues,
        PageModel, Toast, ToastStyle,
    },
    pages::{self, Page, PageHandle},
    shell::launcher::{perform, set_command_hotkey},
};

const VALUE: &str = "value";

fn customizations(cx: &App) -> Result<Entity<Customizations>> {
    store(cx).ok_or_else(|| anyhow!("customizations are not loaded"))
}

fn submitted(values: &FormValues) -> String {
    match values.get(VALUE) {
        Some(FormValue::Text(text)) => text.trim().to_owned(),
        Some(FormValue::Empty | FormValue::Bool(_) | FormValue::List(_)) | None => String::new(),
    }
}

fn saved(message: String, cx: &mut App) {
    perform(Effect::Pop, cx);
    perform(
        Effect::ShowToast(Toast::new(ToastStyle::Success, message)),
        cx,
    );
}

/// What the page sets.
#[derive(Clone, Copy, PartialEq)]
enum Setting {
    Alias,
    Hotkey,
}

struct SettingPage {
    setting: Setting,
    item: String,
    title: SharedString,
    store: Entity<Customizations>,
    draft: Option<String>,
    error: Option<String>,
}

/// Sets the alias of the root search item `item`, titled `title`.
pub fn alias_page(item: String, title: SharedString, cx: &mut App) -> Result<PageHandle> {
    page(Setting::Alias, item, title, cx)
}

/// Sets the hotkey of the root search item `item`, titled `title`.
pub fn hotkey_page(item: String, title: SharedString, cx: &mut App) -> Result<PageHandle> {
    page(Setting::Hotkey, item, title, cx)
}

fn page(setting: Setting, item: String, title: SharedString, cx: &mut App) -> Result<PageHandle> {
    let store = customizations(cx)?;
    Ok(pages::handle(cx.new(|_| SettingPage {
        setting,
        item,
        title,
        store,
        draft: None,
        error: None,
    })))
}

impl SettingPage {
    fn submit(&mut self, values: FormValues, cx: &mut Context<Self>) {
        let value = submitted(&values);
        let result = match self.setting {
            Setting::Alias => self.save_alias(&value, cx),
            Setting::Hotkey => set_command_hotkey(&self.item, &value, cx),
        };
        match result {
            Ok(()) => {
                let message = match (self.setting, value.is_empty()) {
                    (Setting::Alias, false) => format!("“{value}” now opens {}", self.title),
                    (Setting::Hotkey, false) => format!("{value} now opens {}", self.title),
                    (Setting::Alias, true) => format!("Removed the alias of {}", self.title),
                    (Setting::Hotkey, true) => format!("Removed the hotkey of {}", self.title),
                };
                saved(message, cx);
            }
            Err(error) => {
                self.draft = Some(value);
                self.error = Some(format!("{error:#}"));
                cx.notify();
            }
        }
    }

    fn save_alias(&mut self, alias: &str, cx: &mut Context<Self>) -> Result<()> {
        if alias.contains(char::is_whitespace) {
            return Err(anyhow!("An alias is one word."));
        }
        if let Some(other) = self.store.read(cx).item_with_alias(alias)
            && other != self.item
        {
            return Err(anyhow!("Another command already uses this alias."));
        }
        let item = self.item.clone();
        self.store
            .update(cx, |store, cx| store.set_alias(&item, alias, cx));
        Ok(())
    }
}

impl Page for SettingPage {
    fn title(&self) -> SharedString {
        match self.setting {
            Setting::Alias => format!("Alias for {}", self.title).into(),
            Setting::Hotkey => format!("Hotkey for {}", self.title).into(),
        }
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let submit = FormHandler::new(move |values, _, cx| {
            page.update(cx, |page, cx| page.submit(values, cx)).ok();
        });
        let store = self.store.read(cx);
        let current = match self.setting {
            Setting::Alias => store.alias(&self.item),
            Setting::Hotkey => store.hotkey(&self.item),
        }
        .unwrap_or_default()
        .to_owned();
        let (label, placeholder, info) = match self.setting {
            Setting::Alias => (
                "Alias",
                "gh",
                "Typing the alias in the search puts this command first. Leave it empty to remove it.",
            ),
            Setting::Hotkey => (
                "Hotkey",
                "ctrl-alt-c",
                "Opens this command from any application. Write it as modifiers and a key, \
                 such as ctrl-alt-c or ctrl-shift-f1. Leave it empty to remove it.",
            ),
        };
        let field = Field::new(
            VALUE,
            label,
            Control::Text {
                placeholder: Some(placeholder.into()),
                value: self.draft.clone().unwrap_or(current).into(),
            },
        )
        .with_info(info);
        let field = match &self.error {
            Some(error) => field.with_error(error.clone()),
            None => field,
        };
        FormModel::new()
            .with_field(field)
            .with_actions(ActionPanel::new().with_action(Action::new(
                match self.setting {
                    Setting::Alias => "Save Alias",
                    Setting::Hotkey => "Save Hotkey",
                },
                Effect::SubmitForm(submit),
            )))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}
