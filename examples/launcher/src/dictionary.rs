//! Define Word: a word's meanings, pronunciation and examples from
//! Wiktionary, through the Free Dictionary API. English words are looked up
//! in English and Chinese ones in Chinese. The word selected when the
//! launcher was summoned is looked up while nothing is typed.

use std::time::Duration;

use anyhow::Result;
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};
use serde::Deserialize;

use crate::{
    model::{
        Accessory, Action, ActionPanel, DetailModel, Effect, Image, Item, ItemId, ListModel,
        PageModel, Section,
    },
    pages::{self, Page, PageHandle},
};

/// Typing faster than this does not look up each key.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// Senses listed per part of speech, subsenses included.
const SENSES: usize = 12;
/// Examples longer than this are cut short in a row's subtitle.
const EXAMPLE_CHARS: usize = 140;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
struct Response {
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Entry {
    #[serde(rename = "partOfSpeech", default)]
    part_of_speech: String,
    #[serde(default)]
    pronunciations: Vec<Pronunciation>,
    #[serde(default)]
    senses: Vec<Sense>,
    #[serde(default)]
    synonyms: Vec<String>,
    #[serde(default)]
    antonyms: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Pronunciation {
    #[serde(default)]
    text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Sense {
    #[serde(default)]
    definition: String,
    #[serde(default)]
    examples: Vec<String>,
    #[serde(default)]
    synonyms: Vec<String>,
    #[serde(default)]
    subsenses: Vec<Sense>,
}

impl Entry {
    fn pronunciation(&self) -> Option<&str> {
        self.pronunciations
            .iter()
            .map(|pronunciation| pronunciation.text.as_str())
            .find(|text| !text.is_empty())
    }

    /// Senses and their subsenses, in order, up to [`SENSES`].
    fn all_senses(&self) -> Vec<&Sense> {
        self.senses
            .iter()
            .flat_map(|sense| std::iter::once(sense).chain(sense.subsenses.iter()))
            .filter(|sense| !sense.definition.trim().is_empty())
            .take(SENSES)
            .collect()
    }
}

/// A lookup's outcome: entries, none, or what went wrong.
#[derive(Clone, Debug, PartialEq)]
enum Lookup {
    Found(Vec<Entry>),
    NotFound,
    Failed(String),
}

fn is_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF))
}

/// Looks `word` up. Blocking.
fn look_up(word: &str) -> Lookup {
    let language = match is_cjk(word) {
        true => "zh",
        false => "en",
    };
    let url = format!(
        "https://freedictionaryapi.com/api/v1/entries/{language}/{}",
        percent_encoding::utf8_percent_encode(word, percent_encoding::NON_ALPHANUMERIC)
    );
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .and_then(|client| client.get(url).send());
    match response {
        Ok(response) if response.status() == reqwest::StatusCode::NOT_FOUND => Lookup::NotFound,
        Ok(response) => match response
            .error_for_status()
            .and_then(|response| response.json::<Response>())
        {
            Ok(response) if response.entries.is_empty() => Lookup::NotFound,
            Ok(response) => Lookup::Found(response.entries),
            Err(error) => Lookup::Failed(error.to_string()),
        },
        Err(error) => Lookup::Failed(error.to_string()),
    }
}

/// The word to look up in `text`: a single word or short phrase, trimmed of
/// punctuation; `None` for anything longer.
fn word_in(text: &str) -> Option<String> {
    let word = text
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '\'' && c != '-')
        .to_lowercase();
    let short = word.split_whitespace().count() <= 3 && word.chars().count() <= 32;
    (!word.is_empty() && short).then_some(word)
}

pub fn define_word_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let selection = crate::selection::latest().and_then(|text| word_in(&text));
    Ok(pages::handle(cx.new(|cx| {
        let mut page = DictionaryPage {
            query: String::new(),
            selection,
            word: None,
            lookup: None,
            task: None,
        };
        page.look_up(cx);
        page
    })))
}

struct DictionaryPage {
    query: String,
    selection: Option<String>,
    /// The word the shown lookup is for.
    word: Option<String>,
    lookup: Option<Lookup>,
    task: Option<Task<()>>,
}

impl DictionaryPage {
    fn look_up(&mut self, cx: &mut Context<Self>) {
        let word = match self.query.trim().is_empty() {
            true => self.selection.clone(),
            false => word_in(&self.query),
        };
        let Some(word) = word else {
            self.task = None;
            self.word = None;
            self.lookup = None;
            cx.notify();
            return;
        };
        let wait = !self.query.trim().is_empty();
        self.task = Some(cx.spawn(async move |this, cx| {
            if wait {
                cx.background_executor().timer(DEBOUNCE).await;
            }
            let lookup = {
                let word = word.clone();
                cx.background_spawn(async move { look_up(&word) }).await
            };
            this.update(cx, |page, cx| {
                page.word = Some(word);
                page.lookup = Some(lookup);
                page.task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

/// The entry as Markdown, for the side panel.
fn entry_markdown(word: &str, entry: &Entry) -> String {
    let mut text = format!("# {word}\n\n*{}*", entry.part_of_speech);
    if let Some(pronunciation) = entry.pronunciation() {
        text.push_str(&format!(" · {pronunciation}"));
    }
    text.push('\n');
    for (ix, sense) in entry.all_senses().iter().enumerate() {
        text.push_str(&format!("\n{}. {}\n", ix + 1, sense.definition));
        if let Some(example) = sense.examples.first() {
            text.push_str(&format!(
                "\n   *“{}”*\n",
                example.lines().next().unwrap_or("")
            ));
        }
    }
    if !entry.synonyms.is_empty() {
        text.push_str(&format!("\n**Synonyms:** {}\n", entry.synonyms.join(", ")));
    }
    if !entry.antonyms.is_empty() {
        text.push_str(&format!("\n**Antonyms:** {}\n", entry.antonyms.join(", ")));
    }
    text
}

fn shorten(text: &str, limit: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    match line.chars().count() > limit {
        true => format!(
            "{}…",
            line.chars().take(limit).collect::<String>().trim_end()
        ),
        false => line.to_owned(),
    }
}

/// A Chinese word's Mandarin reading with tone marks, such as `nǐ hǎo`;
/// Wiktionary gives only IPA for it.
fn pinyin_of(word: &str) -> Option<String> {
    use pinyin::ToPinyin as _;
    let syllables: Vec<&str> = word
        .to_pinyin()
        .flatten()
        .map(|pinyin| pinyin.with_tone())
        .collect();
    (!syllables.is_empty()).then(|| syllables.join(" "))
}

fn sections(word: &str, entries: &[Entry]) -> Vec<(String, Vec<Item>)> {
    let wiktionary = format!("https://en.wiktionary.org/wiki/{}", word.replace(' ', "_"));
    let pinyin = is_cjk(word).then(|| pinyin_of(word)).flatten();
    entries
        .iter()
        .enumerate()
        .map(|(entry_ix, entry)| {
            let markdown = entry_markdown(word, entry);
            let title = match pinyin.as_deref().or(entry.pronunciation()) {
                Some(pronunciation) => format!("{} · {pronunciation}", entry.part_of_speech),
                None => entry.part_of_speech.clone(),
            };
            let items = entry
                .all_senses()
                .into_iter()
                .enumerate()
                .map(|(ix, sense)| {
                    let mut item = Item::new(
                        ItemId::new(format!("definition/{entry_ix}/{ix}")),
                        sense.definition.clone(),
                    )
                    .with_image(Image::Icon("book-a".into()))
                    .with_detail(DetailModel::new(markdown.clone()))
                    .with_actions(
                        ActionPanel::new()
                            .with_action(
                                Action::new(
                                    "Copy Definition",
                                    Effect::Copy(sense.definition.clone().into()),
                                )
                                .with_image(Image::Icon("copy".into())),
                            )
                            .with_action(
                                Action::new("Copy Word", Effect::Copy(word.to_owned().into()))
                                    .with_image(Image::Icon("type".into()))
                                    .with_shortcut("secondary-shift-c"),
                            )
                            .with_action(
                                Action::new(
                                    "Open in Wiktionary",
                                    Effect::OpenUrl(wiktionary.clone().into()),
                                )
                                .with_image(Image::Icon("globe".into()))
                                .with_shortcut("secondary-o"),
                            ),
                    );
                    if let Some(example) = sense.examples.first() {
                        item = item.with_subtitle(format!("“{}”", shorten(example, EXAMPLE_CHARS)));
                    }
                    if !sense.synonyms.is_empty() {
                        item = item.with_accessory(Accessory::text(
                            sense
                                .synonyms
                                .iter()
                                .take(2)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join(", "),
                        ));
                    }
                    item
                })
                .collect();
            (title, items)
        })
        .collect()
}

impl Page for DictionaryPage {
    fn title(&self) -> SharedString {
        "Define Word".into()
    }

    fn model(&mut self, _: &mut Window, _: &mut Context<Self>) -> PageModel {
        let list = ListModel::new()
            .with_placeholder("Type a word…")
            .with_filtering(false)
            .with_loading(self.task.is_some());
        let word = self.word.clone().unwrap_or_default();
        match &self.lookup {
            Some(Lookup::Found(entries)) => sections(&word, entries).into_iter().fold(
                list.with_showing_detail(true),
                |list, (title, items)| {
                    list.with_section(Section::new().with_title(title).with_items(items))
                },
            ),
            Some(Lookup::NotFound) => list
                .with_empty_title(format!("No definitions of “{word}”"))
                .with_empty_description("Check the spelling, or try Translate."),
            Some(Lookup::Failed(error)) => list
                .with_empty_title("Couldn’t reach the dictionary")
                .with_empty_description(error.clone()),
            None => list
                .with_empty_title(match self.task {
                    Some(_) => "Looking up…",
                    None => "Type a word to define",
                })
                .with_empty_description("English and Chinese words, from Wiktionary."),
        }
        .into()
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.look_up(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_entries_and_picks_the_word() {
        let response: Response = serde_json::from_str(
            r#"{"word":"run","entries":[
                {"language":{"code":"en","name":"English"},"partOfSpeech":"verb",
                 "pronunciations":[{"type":"ipa","text":"/ɹʌn/","tags":[]}],
                 "senses":[{"definition":"To move swiftly.","tags":[],
                    "examples":["Run, and you might still catch the train!"],
                    "synonyms":["sprint"],
                    "subsenses":[{"definition":"To flee.","examples":[]}]}],
                 "synonyms":["dash"],"antonyms":["walk"]}],
              "source":{"url":"https://en.wiktionary.org"}}"#,
        )
        .unwrap();
        let sections = sections("run", &response.entries);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "verb · /ɹʌn/");
        assert_eq!(sections[0].1.len(), 2, "a subsense is a row of its own");
        assert_eq!(
            sections[0].1[0].subtitle().map(|s| s.as_ref()),
            Some("“Run, and you might still catch the train!”")
        );
        let markdown = entry_markdown("run", &response.entries[0]);
        assert!(markdown.contains("2. To flee."));
        assert!(markdown.contains("**Antonyms:** walk"));
        assert_eq!(word_in("  Serendipity!  ").as_deref(), Some("serendipity"));
        assert_eq!(word_in("a much longer sentence than a word"), None);
        assert!(is_cjk("你好"));
        assert_eq!(pinyin_of("你好").as_deref(), Some("nǐ hǎo"));
    }

    /// Asks the real service. Run by hand: needs the network.
    #[test]
    #[ignore = "needs the network"]
    fn test_looks_up_by_network() {
        for word in ["serendipity", "你好", "zzqqxx"] {
            match look_up(word) {
                Lookup::Found(entries) => println!("{word}: {}", sections(word, &entries)[0].0),
                other => println!("{word}: {other:?}"),
            }
        }
    }
}
