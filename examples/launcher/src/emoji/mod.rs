//! Search Emoji & Symbols: every emoji, by name and shortcode, and the
//! symbols people otherwise look up in a character map, to paste or copy.
//!
//! The emoji most recently used come first, and a skin tone chosen once
//! applies to every emoji that has one.

mod symbols;

use std::path::PathBuf;

use anyhow::Result;
use emojis::{Group, SkinTone};
use gpui_kit::{App, AppContext as _, Context, SharedString, Window};
use serde::{Deserialize, Serialize};

use crate::{
    model::{
        Action, ActionEntry, ActionPanel, ActionSection, Choice, Dropdown, Effect, Image, Item,
        ItemId, Layout, ListModel, PageModel, RunHandler, Section, Submenu, TextHandler,
    },
    pages::{self, Page, PageHandle},
};

/// How many recently used characters are listed first.
const RECENT: usize = 16;
/// The newest emoji Windows' and macOS' fonts draw; newer ones would show
/// as boxes.
const NEWEST_UNICODE: (u32, u32) = (15, 1);

/// What is remembered between uses.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Saved {
    /// Characters, most recent first.
    recent: Vec<String>,
    /// The index of the chosen skin tone, 0 for the default yellow.
    skin_tone: u8,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("emoji.json"))
}

impl Saved {
    fn load() -> Self {
        path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let Some(path) = path() else {
            return;
        };
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory).ok();
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            std::fs::write(path, text).ok();
        }
    }

    fn used(character: &str) {
        let mut saved = Self::load();
        saved.recent.retain(|known| known != character);
        saved.recent.insert(0, character.to_owned());
        saved.recent.truncate(RECENT);
        saved.save();
    }
}

const TONES: [(SkinTone, &str); 6] = [
    (SkinTone::Default, "Default"),
    (SkinTone::Light, "Light"),
    (SkinTone::MediumLight, "Medium Light"),
    (SkinTone::Medium, "Medium"),
    (SkinTone::MediumDark, "Medium Dark"),
    (SkinTone::Dark, "Dark"),
];

fn group_title(group: Group) -> &'static str {
    match group {
        Group::SmileysAndEmotion => "Smileys & Emotion",
        Group::PeopleAndBody => "People & Body",
        Group::AnimalsAndNature => "Animals & Nature",
        Group::FoodAndDrink => "Food & Drink",
        Group::TravelAndPlaces => "Travel & Places",
        Group::Activities => "Activities",
        Group::Objects => "Objects",
        Group::Symbols => "Symbols",
        Group::Flags => "Flags",
    }
}

fn group_id(group: Group) -> String {
    group_title(group)
        .to_lowercase()
        .replace(" & ", "-")
        .replace(' ', "-")
}

/// Whether the system fonts can draw `emoji`.
fn is_drawable(emoji: &emojis::Emoji) -> bool {
    let version = emoji.unicode_version();
    (version.major(), version.minor()) <= NEWEST_UNICODE
}

/// Every emoji in its default skin tone, in Unicode's order.
fn all_emoji() -> impl Iterator<Item = &'static emojis::Emoji> {
    emojis::iter()
        .filter(|emoji| matches!(emoji.skin_tone(), None | Some(SkinTone::Default)))
        .filter(|emoji| is_drawable(emoji))
}

/// The emoji in the chosen skin tone, where it has one.
fn toned(emoji: &'static emojis::Emoji, tone: SkinTone) -> &'static str {
    match tone {
        SkinTone::Default => emoji.as_str(),
        tone => emoji
            .with_skin_tone(tone)
            .filter(|toned| is_drawable(toned))
            .map_or(emoji.as_str(), emojis::Emoji::as_str),
    }
}

pub fn search_emoji_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| EmojiPage {
        saved: Saved::load(),
        category: "all".into(),
        list: None,
    })))
}

struct EmojiPage {
    saved: Saved,
    category: SharedString,
    /// Two thousand items are built once per change, not once per frame.
    list: Option<ListModel>,
}

impl EmojiPage {
    fn tone(&self) -> SkinTone {
        TONES
            .get(self.saved.skin_tone as usize)
            .map_or(SkinTone::Default, |(tone, _)| *tone)
    }

    fn item(
        &self,
        id: String,
        character: &str,
        name: &str,
        keywords: impl IntoIterator<Item = String>,
        cx: &Context<Self>,
    ) -> Item {
        let paste = {
            let character = character.to_owned();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                Saved::used(&character);
                crate::shell::launcher::perform(Effect::Paste(character.clone().into()), cx);
            }))
        };
        let copy = {
            let character = character.to_owned();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                Saved::used(&character);
                crate::shell::launcher::perform(Effect::Copy(character.clone().into()), cx);
            }))
        };
        let codepoints = character
            .chars()
            .map(|c| format!("U+{:04X}", c as u32))
            .collect::<Vec<_>>()
            .join(" ");
        let actions = ActionPanel::new()
            .with_action(
                Action::new("Paste Character", paste)
                    .with_image(Image::Icon("clipboard-paste".into())),
            )
            .with_action(Action::new("Copy Character", copy).with_image(Image::Icon("copy".into())))
            .with_action(
                Action::new("Copy Name", Effect::Copy(name.to_owned().into()))
                    .with_image(Image::Icon("type".into()))
                    .with_shortcut("secondary-shift-c"),
            )
            .with_action(
                Action::new("Copy Code Point", Effect::Copy(codepoints.clone().into()))
                    .with_image(Image::Icon("hash".into())),
            )
            .with_section(
                ActionSection::new().with_entry(ActionEntry::Submenu(self.tone_menu(cx))),
            );
        keywords
            .into_iter()
            .fold(
                Item::new(ItemId::new(id), name.to_owned())
                    .with_image(Image::Glyph(character.to_owned().into()))
                    .with_keyword(codepoints),
                |item, keyword| item.with_keyword(keyword),
            )
            .with_actions(actions)
    }

    fn tone_menu(&self, cx: &Context<Self>) -> Submenu {
        let page = cx.entity().downgrade();
        TONES.iter().enumerate().fold(
            Submenu::new("Skin Tone")
                .with_image(Image::Icon("hand".into()))
                .with_shortcut("secondary-shift-t"),
            |menu, (index, (tone, title))| {
                let page = page.clone();
                let sample = emojis::get("👋").map_or("👋", |wave| toned(wave, *tone));
                let current = index as u8 == self.saved.skin_tone;
                menu.with_action(
                    Action::new(
                        match current {
                            true => format!("{title} ✓"),
                            false => (*title).to_owned(),
                        },
                        Effect::Run(RunHandler::new(move |(), _, cx| {
                            page.update(cx, |page, cx| {
                                page.saved = Saved::load();
                                page.saved.skin_tone = index as u8;
                                page.saved.save();
                                page.list = None;
                                cx.notify();
                            })
                            .ok();
                        })),
                    )
                    .with_image(Image::Glyph(sample.into())),
                )
            },
        )
    }

    fn emoji_item(&self, emoji: &'static emojis::Emoji, prefix: &str, cx: &Context<Self>) -> Item {
        let character = toned(emoji, self.tone());
        self.item(
            format!("{prefix}{}", emoji.as_str()),
            character,
            emoji.name(),
            emoji
                .shortcodes()
                .map(|code| code.replace('_', " "))
                .chain([group_title(emoji.group()).to_owned()]),
            cx,
        )
    }

    fn sections(&self, cx: &Context<Self>) -> Vec<Section> {
        let mut sections = Vec::new();
        let all = self.category == "all";
        if all && !self.saved.recent.is_empty() {
            sections.push(Section::new().with_title("Recently Used").with_items(
                self.saved.recent.iter().filter_map(|character| {
                    match emojis::get(character) {
                        Some(emoji) => Some(
                            self.emoji_item(
                                emoji
                                    .skin_tones()
                                    .and_then(|mut tones| tones.next())
                                    .unwrap_or(emoji),
                                "recent:",
                                cx,
                            ),
                        ),
                        None => symbols::CATEGORIES
                            .iter()
                            .flat_map(|(_, _, symbols)| symbols.iter())
                            .find(|(symbol, _)| symbol == character)
                            .map(|(symbol, name)| {
                                self.item(format!("recent:{symbol}"), symbol, name, [], cx)
                            }),
                    }
                }),
            ));
        }
        for group in Group::iter() {
            if !all && self.category.as_ref() != group_id(group) {
                continue;
            }
            sections.push(
                Section::new().with_title(group_title(group)).with_items(
                    all_emoji()
                        .filter(|emoji| emoji.group() == group)
                        .map(|emoji| self.emoji_item(emoji, "emoji:", cx)),
                ),
            );
        }
        for (id, title, symbols) in symbols::CATEGORIES {
            if !all && self.category.as_ref() != *id && self.category != "symbols" {
                continue;
            }
            sections.push(
                Section::new()
                    .with_title(*title)
                    .with_items(symbols.iter().map(|(symbol, name)| {
                        self.item(format!("symbol:{id}:{symbol}"), symbol, name, [], cx)
                    })),
            );
        }
        sections
    }

    fn dropdown(&self, cx: &Context<Self>) -> Dropdown {
        let page = cx.entity().downgrade();
        let choices = [("all".to_owned(), "All Categories".to_owned())]
            .into_iter()
            .chain(Group::iter().map(|group| (group_id(group), group_title(group).to_owned())))
            .chain([("symbols".to_owned(), "All Symbols".to_owned())])
            .chain(
                symbols::CATEGORIES
                    .iter()
                    .map(|(id, title, _)| ((*id).to_owned(), (*title).to_owned())),
            );
        choices
            .fold(Dropdown::new("Category"), |dropdown, (value, title)| {
                dropdown.with_choice(Choice::new(value, title))
            })
            .with_value(self.category.clone())
            .with_on_change(TextHandler::new(move |value, _, cx| {
                page.update(cx, |page, cx| {
                    page.category = value;
                    page.list = None;
                    cx.notify();
                })
                .ok();
            }))
    }
}

impl Page for EmojiPage {
    fn title(&self) -> SharedString {
        "Emoji & Symbols".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        if let Some(list) = &self.list {
            return list.clone().into();
        }
        let list = self.sections(cx).into_iter().fold(
            ListModel::new()
                .with_layout(Layout::Grid { columns: 8 })
                .with_placeholder("Search emoji and symbols…")
                .with_empty_title("No matching characters")
                .with_dropdown(self.dropdown(cx)),
            ListModel::with_section,
        );
        self.list = Some(list.clone());
        list.into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}

    fn did_reappear(&mut self, cx: &mut Context<Self>) {
        self.saved = Saved::load();
        self.list = None;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emoji_are_drawable_and_toned() {
        assert!(all_emoji().count() > 1500);
        assert!(all_emoji().all(|emoji| emoji.skin_tone() != Some(SkinTone::Light)));
        let wave = emojis::get("👋").unwrap();
        assert_eq!(toned(wave, SkinTone::Default), "👋");
        assert_eq!(toned(wave, SkinTone::Dark), "👋🏿");
        let cat = emojis::get("🐱").unwrap();
        assert_eq!(toned(cat, SkinTone::Dark), "🐱");
        assert_eq!(group_id(Group::SmileysAndEmotion), "smileys-emotion");
    }
}
