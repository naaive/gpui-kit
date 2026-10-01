//! Calculator History: the calculations and conversions whose answer was
//! copied or pasted from the root search, newest first, kept in
//! `calculator-history.json`.

use std::path::PathBuf;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Window};
use serde::{Deserialize, Serialize};

use crate::{
    format::relative_time,
    model::{
        Accessory, Action, ActionEntry, ActionPanel, ActionSection, ActionStyle, Confirmation,
        Effect, Image, Item, ItemId, ListModel, PageModel, RunHandler, Section,
    },
    pages::{self, Page, PageHandle},
    sources::calculator,
};

/// The most calculations kept.
const LIMIT: usize = 200;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Calculation {
    expression: String,
    /// The value copied, such as `1.5`.
    answer: String,
    /// The answer as shown, such as `1.5 km`.
    display: String,
    /// Unix seconds.
    at: u64,
}

fn path() -> Option<PathBuf> {
    crate::shell::data_directory().map(|directory| directory.join("calculator-history.json"))
}

fn load() -> Vec<Calculation> {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(history: &[Calculation]) {
    let Some(path) = path() else {
        return;
    };
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory).ok();
    }
    if let Ok(text) = serde_json::to_string_pretty(history)
        && let Err(error) = std::fs::write(&path, text)
    {
        tracing::warn!("cannot save the calculator history: {error}");
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Adds `calculation` to the front of `history`, once.
fn push(history: &mut Vec<Calculation>, calculation: Calculation) {
    history.retain(|known| known.expression != calculation.expression);
    history.insert(0, calculation);
    history.truncate(LIMIT);
}

/// Remembers the calculation `query`, whose answer was just used.
pub fn record(query: &str) {
    let expression = query.trim();
    let Some((answer, display)) = calculator::answer(expression) else {
        return;
    };
    let mut history = load();
    push(
        &mut history,
        Calculation {
            expression: expression.to_owned(),
            answer: answer.to_string(),
            display,
            at: now(),
        },
    );
    save(&history);
}

pub fn history_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    Ok(pages::handle(cx.new(|_| HistoryPage { history: load() })))
}

struct HistoryPage {
    history: Vec<Calculation>,
}

impl HistoryPage {
    fn item(&self, ix: usize, calculation: &Calculation, now: u64, cx: &Context<Self>) -> Item {
        let page = cx.entity().downgrade();
        let expression = calculation.expression.clone();
        let delete = move |all: bool| {
            let page = page.clone();
            let expression = expression.clone();
            Effect::Run(RunHandler::new(move |(), _, cx| {
                let mut history = load();
                match all {
                    true => history.clear(),
                    false => history.retain(|known| known.expression != expression),
                }
                save(&history);
                page.update(cx, |page, cx| {
                    page.history = history;
                    cx.notify();
                })
                .ok();
            }))
        };
        let answer: SharedString = calculation.answer.clone().into();
        Item::new(
            ItemId::new(format!("calculation/{ix}")),
            format!("= {}", calculation.display),
        )
        .with_subtitle(calculation.expression.clone())
        .with_image(Image::Icon("calculator".into()))
        .with_keyword(calculation.expression.clone())
        .with_accessory(Accessory::text(relative_time(calculation.at, now)))
        .with_actions(
            ActionPanel::new()
                .with_action(
                    Action::new("Copy Answer", Effect::Copy(answer.clone()))
                        .with_image(Image::Icon("copy".into())),
                )
                .with_action(
                    Action::new("Paste Answer", Effect::Paste(answer))
                        .with_image(Image::Icon("clipboard-paste".into())),
                )
                .with_action(
                    Action::new(
                        "Copy Expression",
                        Effect::Copy(calculation.expression.clone().into()),
                    )
                    .with_image(Image::Icon("square-function".into()))
                    .with_shortcut("secondary-shift-c"),
                )
                .with_section(
                    ActionSection::new()
                        .with_entry(ActionEntry::Action(
                            Action::new("Delete Calculation", delete(false))
                                .with_image(Image::Icon("trash".into()))
                                .with_style(ActionStyle::Destructive)
                                .with_shortcut("ctrl-x"),
                        ))
                        .with_entry(ActionEntry::Action(
                            Action::new(
                                "Clear History",
                                Effect::Confirm(
                                    Confirmation::new(
                                        "Clear the calculator history?",
                                        delete(true),
                                    )
                                    .with_confirm_title("Clear")
                                    .destructive(true),
                                ),
                            )
                            .with_image(Image::Icon("trash-off".into()))
                            .with_style(ActionStyle::Destructive)
                            .with_shortcut("ctrl-shift-x"),
                        )),
                ),
        )
    }
}

impl Page for HistoryPage {
    fn title(&self) -> SharedString {
        "Calculator History".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let now = now();
        let items: Vec<Item> = self
            .history
            .iter()
            .enumerate()
            .map(|(ix, calculation)| self.item(ix, calculation, now, cx))
            .collect();
        ListModel::new()
            .with_placeholder("Search calculations…")
            .with_empty_title("No calculations yet")
            .with_empty_description(
                "Type a calculation in the root search, such as 12% of 80, and copy its answer.",
            )
            .with_section(Section::new().with_title("History").with_items(items))
            .into()
    }

    fn set_query(&mut self, _: &str, _: &mut Window, _: &mut Context<Self>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calculation(expression: &str, at: u64) -> Calculation {
        Calculation {
            expression: expression.into(),
            answer: "2".into(),
            display: "2".into(),
            at,
        }
    }

    #[test]
    fn test_newest_first_and_each_expression_once() {
        let mut history = Vec::new();
        push(&mut history, calculation("1+1", 1));
        push(&mut history, calculation("4/2", 2));
        push(&mut history, calculation("1+1", 3));
        assert_eq!(
            history.iter().map(|c| c.at).collect::<Vec<_>>(),
            [3, 2],
            "the repeated calculation moves to the front"
        );
        for at in 0..LIMIT as u64 + 10 {
            push(&mut history, calculation(&format!("{at}*1"), at));
        }
        assert_eq!(history.len(), LIMIT);
    }
}
