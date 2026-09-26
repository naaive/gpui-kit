use gpui_kit::{Context, SharedString, Window};

use super::Page;
use crate::{
    extensions::Catalog,
    model::{Action, Effect, Item, ItemId, ListModel, PageModel, RunHandler, Section},
    search::score_item,
};

/// The bottom of the navigation stack: every command, searchable.
///
/// It is an ordinary List page, drawn by the same renderer as an extension's,
/// so there is no second list implementation to keep in step.
pub struct RootSearchPage {
    commands: Vec<Item>,
    system: Vec<Item>,
    query: String,
}

impl RootSearchPage {
    pub fn new(catalog: &Catalog) -> Self {
        let commands = catalog
            .commands()
            .map(|(extension, command)| {
                let item = Item::new(
                    ItemId::new(command.id().to_string()),
                    command.title().clone(),
                )
                .with_subtitle(
                    command
                        .subtitle()
                        .cloned()
                        .unwrap_or_else(|| extension.name().clone()),
                )
                .with_accessory("Command")
                .with_action(Action::new(
                    "Open Command",
                    Effect::Launch(command.id().clone()),
                ));
                let item = match command.icon() {
                    Some(icon) => item.with_icon(icon.clone()),
                    None => item,
                };
                command
                    .keywords()
                    .iter()
                    .fold(item, |item, keyword| item.with_keyword(keyword.clone()))
            })
            .collect();

        let system = vec![
            Item::new(ItemId::new("system/website"), "Open GPUI Kit Website")
                .with_icon("globe")
                .with_accessory("Link")
                .with_action(Action::new(
                    "Open in Browser",
                    Effect::OpenUrl("https://gpui-kit.com".into()),
                ))
                .with_action(Action::new(
                    "Copy URL",
                    Effect::Copy("https://gpui-kit.com".into()),
                )),
            Item::new(ItemId::new("system/quit"), "Quit Launcher")
                .with_icon("circle-x")
                .with_accessory("Command")
                .with_keyword("exit")
                .with_action(Action::new(
                    "Quit",
                    Effect::Run(RunHandler::new(|_, cx| cx.quit())),
                )),
        ];

        Self {
            commands,
            system,
            query: String::new(),
        }
    }

    /// With no query, commands are grouped by where they come from. With a
    /// query, the matches form one ranked list: grouping would split the best
    /// match from the next best.
    fn list(&self) -> ListModel {
        let list = ListModel::new()
            .with_placeholder("Search for commands…")
            .with_filtering(false);
        if self.query.trim().is_empty() {
            return list
                .with_section(
                    Section::new()
                        .with_title("Extensions")
                        .with_items(self.commands.iter().cloned()),
                )
                .with_section(
                    Section::new()
                        .with_title("Launcher")
                        .with_items(self.system.iter().cloned()),
                );
        }
        let mut matches: Vec<_> = self
            .commands
            .iter()
            .chain(&self.system)
            .filter_map(|item| score_item(&self.query, item).map(|score| (score, item)))
            .collect();
        // Stable, so equal scores keep catalog order.
        matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        list.with_section(
            Section::new()
                .with_title("Results")
                .with_items(matches.into_iter().map(|(_, item)| item.clone())),
        )
    }
}

impl Page for RootSearchPage {
    fn title(&self) -> SharedString {
        "Launcher".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        PageModel::List(self.list())
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        self.query = query.to_owned();
        cx.notify();
    }
}
