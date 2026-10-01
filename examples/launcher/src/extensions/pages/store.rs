//! The Extension Store: what the store lists, with its README beside it, to
//! install, update or uninstall.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, WeakEntity, Window};

use super::show_failure;
use crate::{
    extensions::{
        host::Services,
        install,
        store::{self, Listing, StoreIndex, StoreSource},
    },
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Choice,
        Confirmation, DetailModel, Dropdown, Effect, Image, Item, ItemId, ListModel, Metadata,
        MetadataValue, PageModel, RunHandler, Section, Tag, TextHandler, Toast, ToastStyle, Tone,
    },
    pages::{self, Page, PageHandle},
};

const ALL: &str = "all";

pub struct StorePage {
    services: Services,
    source: Result<StoreSource, String>,
    /// `None` while the listing loads.
    index: Option<Result<StoreIndex, String>>,
    /// The installed extensions' versions, by id.
    installed: BTreeMap<String, String>,
    category: SharedString,
    /// READMEs fetched so far, by extension id; `None` while one loads.
    readmes: HashMap<String, Option<String>>,
    /// Extensions being installed, updated or removed.
    busy: HashSet<String>,
    _load: Option<Task<()>>,
}

/// Opens the store at `source`.
pub fn store_page(
    services: Services,
    source: Result<StoreSource, String>,
    cx: &mut App,
) -> PageHandle {
    pages::handle(cx.new(|cx| {
        let mut page = StorePage {
            services,
            source,
            index: None,
            installed: BTreeMap::new(),
            category: ALL.into(),
            readmes: HashMap::new(),
            busy: HashSet::new(),
            _load: None,
        };
        page.load(cx);
        page
    }))
}

impl StorePage {
    fn load(&mut self, cx: &mut Context<Self>) {
        self.read_installed();
        let source = match &self.source {
            Ok(source) => source.clone(),
            Err(error) => {
                self.index = Some(Err(error.clone()));
                return;
            }
        };
        self.index = None;
        let task = cx.background_spawn(async move { source.index() });
        self._load = Some(cx.spawn(async move |this, cx| {
            let index = task.await.map_err(|error| format!("{error:#}"));
            this.update(cx, |page, cx| {
                page.index = Some(index);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn read_installed(&mut self) {
        self.installed = install::installed(&self.services.data)
            .map(|extensions| {
                extensions
                    .into_iter()
                    .map(|extension| (extension.id().to_owned(), extension.version().to_owned()))
                    .collect()
            })
            .unwrap_or_default();
    }

    fn fetch_readme(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.readmes.contains_key(id) {
            return;
        }
        let Some((listing, Ok(source))) = self.listing(id).cloned().zip(Some(self.source.clone()))
        else {
            return;
        };
        let Some(readme) = listing.readme().cloned() else {
            self.readmes.insert(id.to_owned(), Some(String::new()));
            return;
        };
        self.readmes.insert(id.to_owned(), None);
        let id = id.to_owned();
        let path = format!("{}/{}", listing.path, readme.path);
        let task = cx.background_spawn(async move { source.read(&path) });
        cx.spawn(async move |this, cx| {
            let text = task
                .await
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            this.update(cx, |page, cx| {
                page.readmes.insert(id, Some(text));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn listing(&self, id: &str) -> Option<&Listing> {
        self.index
            .as_ref()?
            .as_ref()
            .ok()?
            .extensions
            .iter()
            .find(|listing| listing.id == id)
    }

    /// Installs or updates `id` from the store, off the main thread.
    fn install(&mut self, id: String, cx: &mut Context<Self>) {
        let (Some(listing), Ok(source)) = (self.listing(&id).cloned(), self.source.clone()) else {
            return;
        };
        if !self.busy.insert(id.clone()) {
            return;
        }
        let updating = self.installed.contains_key(&id);
        let toast_id = format!("store/{id}");
        self.services.effects.toast(
            Toast::new(
                ToastStyle::Progress,
                match updating {
                    true => format!("Updating “{}”…", listing.name),
                    false => format!("Installing “{}”…", listing.name),
                },
            )
            .with_id(toast_id.clone()),
            cx,
        );
        let data = self.services.data.clone();
        let work_listing = listing.clone();
        let task =
            cx.background_spawn(async move { store::install(&data, &source, &work_listing) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |page, cx| {
                page.busy.remove(&id);
                let toast = match result {
                    Ok(installed) => {
                        page.read_installed();
                        page.services.changed.notify(cx);
                        Toast::new(
                            ToastStyle::Success,
                            format!("Installed “{}” {}", installed.name(), installed.version()),
                        )
                        .with_message("Its commands are in the root search.")
                    }
                    Err(error) => Toast::new(
                        ToastStyle::Failure,
                        format!("Couldn’t install “{}”", listing.name),
                    )
                    .with_message(format!("{error:#}")),
                };
                page.services.effects.toast(toast.with_id(toast_id), cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn uninstall(&mut self, id: String, cx: &mut Context<Self>) {
        let services = self.services.clone();
        match install::uninstall(
            &services.data,
            &id,
            &services.permissions,
            &services.preferences,
        ) {
            Ok(()) => {
                self.read_installed();
                services.changed.notify(cx);
                services
                    .effects
                    .toast(Toast::new(ToastStyle::Success, "Uninstalled"), cx);
            }
            Err(error) => show_failure(&services, "Couldn’t uninstall the extension", &error, cx),
        }
        cx.notify();
    }

    fn item(&self, listing: &Listing, page: &WeakEntity<Self>) -> Item {
        let id = listing.id.clone();
        let installed = self.installed.get(&id);
        let has_update = installed.is_some_and(|version| *version != listing.version);
        let busy = self.busy.contains(&id);
        let source = self.source.as_ref().ok();

        let run = |work: fn(&mut Self, String, &mut Context<Self>)| {
            let (page, id) = (page.clone(), id.clone());
            Effect::Run(RunHandler::new(move |(), _, cx| {
                let id = id.clone();
                page.update(cx, |page, cx| work(page, id, cx)).ok();
            }))
        };
        let mut actions = ActionPanel::new();
        if !busy {
            match (installed, has_update) {
                (None, _) => {
                    actions = actions.with_action(
                        Action::new("Install", run(Self::install))
                            .with_image(Image::Icon("download".into())),
                    )
                }
                (Some(_), true) => {
                    actions = actions.with_action(
                        Action::new(format!("Update to {}", listing.version), run(Self::install))
                            .with_image(Image::Icon("refresh-cw".into())),
                    )
                }
                (Some(_), false) => {}
            }
        }
        if let Some(source) = source {
            actions = actions.with_action(
                Action::new(
                    "View Source",
                    Effect::OpenUrl(source.page_url(listing).into()),
                )
                .with_image(Image::Icon("github".into())),
            );
        }
        actions = actions.with_action(
            Action::new("Refresh Store", {
                let page = page.clone();
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    page.update(cx, |page, cx| {
                        page.readmes.clear();
                        page.load(cx)
                    })
                    .ok();
                }))
            })
            .with_image(Image::Icon("rotate-cw".into()))
            .with_shortcut("secondary-r"),
        );
        if installed.is_some() && !busy {
            actions = actions.with_section(
                ActionSection::new().with_entry(ActionEntry::Action(
                    Action::new(
                        "Uninstall",
                        Effect::Confirm(
                            Confirmation::new(
                                format!("Uninstall “{}”?", listing.name),
                                run(Self::uninstall),
                            )
                            .with_message(
                                "Its preferences, permissions and stored data are removed too.",
                            )
                            .with_confirm_title("Uninstall")
                            .destructive(true),
                        ),
                    )
                    .with_style(ActionStyle::Destructive)
                    .with_shortcut("secondary-backspace"),
                )),
            );
        }

        let accessory = match (busy, installed, has_update) {
            (true, _, _) => Accessory::tag("Working…", Tone::Neutral),
            (false, Some(_), true) => Accessory::tag("Update", Tone::Warning),
            (false, Some(_), false) => Accessory::tag("Installed", Tone::Success),
            (false, None, _) => Accessory::text(listing.version.clone()),
        };
        let item = Item::new(ItemId::new(format!("store/{id}")), listing.name.clone())
            .with_subtitle(listing.description.clone())
            .with_image(Image::Icon(
                listing
                    .icon
                    .clone()
                    .unwrap_or_else(|| "package".into())
                    .into(),
            ))
            .with_accessory(accessory)
            .with_keyword(listing.author.clone())
            .with_detail(self.detail(listing))
            .with_actions(actions);
        listing
            .categories
            .iter()
            .chain(listing.commands.iter().map(|command| &command.title))
            .fold(item, |item, keyword| item.with_keyword(keyword.clone()))
    }

    fn detail(&self, listing: &Listing) -> DetailModel {
        let mut markdown = format!(
            "# {}\n\n{}\n\n## Commands\n\n",
            listing.name, listing.description
        );
        for command in &listing.commands {
            markdown.push_str(&format!("- **{}**", command.title));
            if let Some(subtitle) = &command.subtitle {
                markdown.push_str(&format!(" — {subtitle}"));
            }
            match command.mode.as_str() {
                "menu-bar" => markdown.push_str(" *(menu bar)*"),
                "no-view" => markdown.push_str(" *(runs without a page)*"),
                _ => {}
            }
            markdown.push('\n');
        }
        if let Ok(source) = &self.source {
            let screenshots: Vec<String> = listing
                .screenshots()
                .map(|path| {
                    format!(
                        "![Screenshot]({})",
                        source.picture_url(&format!("{}/{path}", listing.path))
                    )
                })
                .collect();
            if !screenshots.is_empty() {
                markdown.push('\n');
                markdown.push_str(&screenshots.join("\n\n"));
                markdown.push('\n');
            }
        }
        let readme = self.readmes.get(&listing.id);
        if let Some(Some(readme)) = readme
            && !readme.trim().is_empty()
        {
            markdown.push_str("\n---\n\n");
            markdown.push_str(readme);
        }
        let mut detail = DetailModel::new(markdown)
            .with_loading(matches!(readme, Some(None)))
            .with_metadata(Metadata::new(
                "Version",
                MetadataValue::Text(listing.version.clone().into()),
            ));
        if !listing.author.is_empty() {
            detail = detail.with_metadata(Metadata::new(
                "Author",
                MetadataValue::Text(listing.author.clone().into()),
            ));
        }
        if !listing.categories.is_empty() {
            detail = detail.with_metadata(Metadata::new(
                "Categories",
                MetadataValue::Tags(
                    listing
                        .categories
                        .iter()
                        .map(|category| Tag::new(category.clone()))
                        .collect(),
                ),
            ));
        }
        if let Some(version) = self.installed.get(&listing.id) {
            detail = detail.with_metadata(Metadata::new(
                "Installed",
                MetadataValue::Text(version.clone().into()),
            ));
        }
        detail
    }
}

impl Page for StorePage {
    fn title(&self) -> SharedString {
        "Extension Store".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let list = ListModel::new()
            .with_placeholder("Search extensions…")
            .with_showing_detail(true)
            .with_loading(self.index.is_none());
        let index = match &self.index {
            None => return list.into(),
            Some(Err(error)) => {
                return list
                    .with_empty_title("Couldn’t reach the store")
                    .with_empty_description(error.clone())
                    .into();
            }
            Some(Ok(index)) => index,
        };
        let mut categories: Vec<&String> = index
            .extensions
            .iter()
            .flat_map(|listing| &listing.categories)
            .collect();
        categories.sort();
        categories.dedup();
        let dropdown = categories.iter().fold(
            Dropdown::new("Category")
                .with_choice(Choice::new(ALL, "All Categories"))
                .with_value(self.category.clone())
                .with_on_change(TextHandler::new({
                    let page = page.clone();
                    move |category, _, cx| {
                        page.update(cx, |page, cx| {
                            page.category = category;
                            cx.notify();
                        })
                        .ok();
                    }
                })),
            |dropdown, category| {
                dropdown.with_choice(Choice::new((*category).clone(), (*category).clone()))
            },
        );
        let shown: Vec<&Listing> = index
            .extensions
            .iter()
            .filter(|listing| {
                self.category == ALL
                    || listing
                        .categories
                        .iter()
                        .any(|category| category.as_str() == self.category.as_ref())
            })
            .collect();
        let (installed, available): (Vec<&Listing>, Vec<&Listing>) = shown
            .into_iter()
            .partition(|listing| self.installed.contains_key(&listing.id));
        let outdated: Vec<String> = installed
            .iter()
            .filter(|listing| {
                self.installed.get(&listing.id) != Some(&listing.version)
                    && !self.busy.contains(&listing.id)
            })
            .map(|listing| listing.id.clone())
            .collect();
        crate::shell::launcher::set_store_update_count(outdated.len(), cx);
        let update_all = (outdated.len() > 1).then(|| {
            let page = page.clone();
            Item::new(
                ItemId::new("store-update-all"),
                format!("Update All ({})", outdated.len()),
            )
            .with_image(Image::Icon("refresh-cw".into()))
            .with_action(Action::new(
                "Update All",
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    let outdated = outdated.clone();
                    page.update(cx, |page, cx| {
                        for id in outdated {
                            page.install(id, cx);
                        }
                    })
                    .ok();
                })),
            ))
        });
        let section = |title: &str, listings: Vec<&Listing>| {
            Section::new().with_title(title.to_owned()).with_items(
                listings
                    .into_iter()
                    .map(|listing| self.item(listing, &page)),
            )
        };
        let installed_section = section("Installed", installed);
        let installed_section = match update_all {
            Some(item) => Section::new()
                .with_title("Installed")
                .with_item(item)
                .with_items(installed_section.items().iter().cloned()),
            None => installed_section,
        };
        list.with_dropdown(dropdown)
            .with_section(installed_section)
            .with_section(section("Available", available))
            .with_empty_title("No extensions in this category")
            .with_on_selection_change(TextHandler::new(move |id, _, cx| {
                let Some(id) = id.strip_prefix("store/").map(str::to_owned) else {
                    return;
                };
                page.update(cx, |page, cx| page.fetch_readme(&id, cx)).ok();
            }))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows a store folder's listing and installs from it, as a click does.
    #[gpui::test]
    fn test_store_page_lists_and_installs(cx: &mut gpui::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let store_folder = folder.path().join("store");
        let extension = store_folder.join("extensions/hello");
        std::fs::create_dir_all(&extension).unwrap();
        std::fs::write(
            extension.join("gpui-shell.json"),
            r#"{ "id": "com.example.hello", "name": "Hello", "version": "1.0.0", "entry": "main.js" }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("launcher.json"),
            r#"{ "description": "Says hello", "categories": ["Fun"],
                 "commands": [{ "name": "hello", "title": "Say Hello", "module": "main.js" }] }"#,
        )
        .unwrap();
        std::fs::write(extension.join("main.js"), "export default 1;").unwrap();
        std::fs::write(extension.join("README.md"), "Hello **world**").unwrap();
        store::write_index(&store_folder).unwrap();

        let services = Services::for_tests(folder.path().join("data"));
        let page = cx.update(|cx| {
            gpui_kit::init(cx);
            store_page(
                services.clone(),
                Ok(StoreSource::Folder(store_folder.clone())),
                cx,
            )
        });
        cx.run_until_parked();
        let window = cx.add_window(|_, _| gpui::Empty);
        let mut cx = gpui::VisualTestContext::from_window(*window, cx);
        let model = |cx: &mut gpui::VisualTestContext| {
            let PageModel::List(list) = cx.update(|window, cx| page.model(window, cx)) else {
                panic!("a list");
            };
            list
        };
        let list = model(&mut cx);
        assert_eq!(
            list.sections()[1].title().map(|t| t.as_ref()),
            Some("Available")
        );
        let item = list.items().next().unwrap().clone();
        assert_eq!(item.subtitle().map(|s| s.as_ref()), Some("Says hello"));

        let handler = list.on_selection_change().unwrap().clone();
        cx.update(|window, cx| handler.call("store/com.example.hello".into(), window, cx));
        cx.run_until_parked();
        let item = model(&mut cx).items().next().unwrap().clone();
        assert!(
            item.detail()
                .unwrap()
                .markdown()
                .contains("Hello **world**")
        );

        let Effect::Run(install) = item.actions().primary().unwrap().effect().clone() else {
            panic!("Install runs");
        };
        cx.update(|window, cx| install.run(window, cx));
        cx.run_until_parked();
        let list = model(&mut cx);
        assert_eq!(
            list.sections()[0].title().map(|t| t.as_ref()),
            Some("Installed")
        );
        assert_eq!(list.sections()[0].items().len(), 1);
        assert!(
            services
                .data
                .extension_dir("com.example.hello")
                .join("main.js")
                .is_file()
        );
    }
}
