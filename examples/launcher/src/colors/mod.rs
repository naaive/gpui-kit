//! Colors: picking one from anywhere on screen, the colors picked before,
//! and a color typed into the root search shown in every notation.
//!
//! Picking shows a magnifier beside the pointer; a click takes the color
//! under it and copies it, Esc or a right click cancels. Windows only;
//! elsewhere Pick Color is not offered.

mod color;
#[cfg(target_os = "windows")]
mod picker;

use std::path::PathBuf;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Window};
use serde::{Deserialize, Serialize};

pub use color::{Color, Format};

use crate::{
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Effect, Image, Item, ItemId, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
    shell::platform::show_hud,
};

/// How many picked colors are kept.
const HISTORY: usize = 200;

/// A color picked before.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Picked {
    hex: String,
    /// Seconds since the Unix epoch.
    picked_at: u64,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("colors.json"))
}

fn load() -> Vec<Picked> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(colors: &[Picked]) {
    let Some(path) = path() else {
        return;
    };
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).ok();
    }
    if let Ok(text) = serde_json::to_string_pretty(colors) {
        std::fs::write(path, text).ok();
    }
}

/// Puts `color` first in the history.
fn remember(color: Color) {
    let hex = color.format(Format::Hex);
    let mut colors = load();
    colors.retain(|picked| picked.hex != hex);
    colors.insert(
        0,
        Picked {
            hex,
            picked_at: crate::search::now(),
        },
    );
    colors.truncate(HISTORY);
    save(&colors);
}

pub fn is_picking_supported() -> bool {
    cfg!(target_os = "windows")
}

/// Hides the launcher, lets the user pick a color on screen, then copies it
/// and remembers it.
pub fn pick_color(cx: &mut App) {
    crate::shell::launcher::hide(cx);
    #[cfg(target_os = "windows")]
    {
        let picked = picker::pick();
        cx.spawn(async move |cx| {
            let Ok(Some(color)) = picked.recv().await else {
                return;
            };
            remember(color);
            let hex = color.format(Format::Hex);
            cx.update(|cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(hex.clone()));
                show_hud(format!("Copied {hex}").into(), cx);
            });
        })
        .detach();
    }
}

/// The actions every color offers: copy it in each notation, the first one
/// being what Enter does.
fn copy_actions(color: Color) -> ActionPanel {
    Format::ALL
        .into_iter()
        .enumerate()
        .fold(ActionPanel::new(), |actions, (index, format)| {
            let text = color.format(format);
            let action = Action::new(
                format!("Copy {}", format.title()),
                Effect::Copy(text.into()),
            )
            .with_image(Image::Icon("copy".into()));
            actions.with_action(match index {
                0 => action,
                index => action.with_shortcut(format!("secondary-{index}")),
            })
        })
}

/// A typed color, shown with its notations: `#f80`, `rgb(…)`, `hsl(…)`.
pub fn item(query: &str) -> Option<Item> {
    let color = Color::parse(query)?;
    let hex = color.format(Format::Hex);
    Some(
        Item::new(ItemId::new(format!("color:{hex}")), hex.clone())
            .with_image(Image::Color(color.rgba()))
            .with_subtitle(format!(
                "{}  ·  {}",
                color.format(Format::Rgb),
                color.format(Format::Hsl)
            ))
            .with_accessory(Accessory::text("Color"))
            .with_actions(copy_actions(color)),
    )
}

pub fn search_colors_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| ColorsPage { colors: load() })))
}

struct ColorsPage {
    colors: Vec<Picked>,
}

impl ColorsPage {
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.colors = load();
        cx.notify();
    }

    fn pick_action() -> Action {
        Action::new(
            "Pick Color",
            Effect::Run(RunHandler::new(|(), _, cx| pick_color(cx))),
        )
        .with_image(Image::Icon("pipette".into()))
        .with_shortcut("secondary-p")
    }

    fn item(&self, picked: &Picked, cx: &Context<Self>) -> Option<Item> {
        let color = Color::parse(&picked.hex)?;
        let page = cx.entity().downgrade();
        let hex = picked.hex.clone();
        let delete = {
            let page = page.clone();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                let mut colors = load();
                colors.retain(|known| known.hex != hex);
                save(&colors);
                page.update(cx, |page, cx| page.reload(cx)).ok();
            }))
        };
        let clear = Effect::Confirm(
            Confirmation::new(
                "Delete all picked colors?",
                Effect::Run(RunHandler::new(move |(), _, cx| {
                    save(&[]);
                    page.update(cx, |page, cx| page.reload(cx)).ok();
                })),
            )
            .with_confirm_title("Delete All")
            .destructive(true),
        );
        let mut actions = copy_actions(color).with_action(
            Action::new("Paste HEX", Effect::Paste(picked.hex.clone().into()))
                .with_image(Image::Icon("clipboard-paste".into()))
                .with_shortcut("secondary-enter"),
        );
        if is_picking_supported() {
            actions = actions.with_action(Self::pick_action());
        }
        let actions = actions.with_section(
            ActionSection::new()
                .with_entry(ActionEntry::Action(
                    Action::new("Delete Color", delete)
                        .with_image(Image::Icon("trash".into()))
                        .with_style(ActionStyle::Destructive)
                        .with_shortcut("ctrl-x"),
                ))
                .with_entry(ActionEntry::Action(
                    Action::new("Delete All Colors", clear)
                        .with_image(Image::Icon("trash".into()))
                        .with_style(ActionStyle::Destructive)
                        .with_shortcut("ctrl-shift-x"),
                )),
        );
        Some(
            Item::new(
                ItemId::new(format!("picked:{}", picked.hex)),
                picked.hex.clone(),
            )
            .with_image(Image::Color(color.rgba()))
            .with_subtitle(color.format(Format::Rgb))
            .with_keyword(color.format(Format::Hsl))
            .with_accessory(Accessory::text(crate::format::relative_time(
                picked.picked_at,
                crate::search::now(),
            )))
            .with_actions(actions),
        )
    }
}

impl Page for ColorsPage {
    fn title(&self) -> SharedString {
        "Colors".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let pick = is_picking_supported().then(|| {
            Item::new(ItemId::new("pick-color"), "Pick Color")
                .with_icon("pipette")
                .with_subtitle("Take a color from anywhere on screen")
                .with_action(Self::pick_action())
        });
        let list = ListModel::new()
            .with_section(Section::new().with_items(pick))
            .with_placeholder("Search picked colors…")
            .with_empty_title("No colors picked yet")
            .with_empty_description(match is_picking_supported() {
                true => "Pick Color takes a color from anywhere on screen.",
                false => "Type a color such as #FF8800 in the root search.",
            })
            .with_section(
                Section::new().with_title("Picked Colors").with_items(
                    self.colors
                        .iter()
                        .filter_map(|picked| self.item(picked, cx)),
                ),
            );
        list.into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}

    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.reload(cx);
    }
}

/// The root search commands for colors.
pub fn commands() -> Vec<Item> {
    let mut commands = Vec::new();
    if is_picking_supported() {
        commands.push(
            Item::new(ItemId::new("system/pick-color"), "Pick Color")
                .with_icon("pipette")
                .with_accessory(Accessory::text("Command"))
                .with_keyword("color picker")
                .with_keyword("eyedropper")
                .with_action(Action::new(
                    "Pick Color",
                    Effect::Run(RunHandler::new(|(), _, cx| pick_color(cx))),
                )),
        );
    }
    commands.push(
        Item::new(ItemId::new("system/search-colors"), "Search Colors")
            .with_icon("palette")
            .with_accessory(Accessory::text("Command"))
            .with_keyword("color picker history")
            .with_action(Action::new(
                "Search Colors",
                Effect::Push(crate::model::PushHandler::new(search_colors_page)),
            )),
    );
    commands
}
