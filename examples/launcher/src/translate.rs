//! Translate: the text typed, or the text selected when the launcher was
//! summoned, in another language. Google Translate's public endpoint is
//! asked first and MyMemory when it cannot be reached.
//!
//! The target language is the one picked in the dropdown, or by default
//! English for Chinese, Japanese or Korean text and Chinese for anything
//! else.

use std::{sync::Mutex, time::Duration};

use anyhow::{Context as _, Result};
use gpui_kit::{App, AppContext as _, Context, SharedString, Task, Window};

use crate::model::{
    Accessory, Action, ActionPanel, Choice, DetailModel, Dropdown, Effect, Image, Item, ItemId,
    ListModel, PageModel, Section, TextHandler,
};
use crate::pages::{self, Page, PageHandle};

const AUTOMATIC: &str = "auto";
/// Typing faster than this does not translate on each key.
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Longer translations are shown in the side panel too.
const LONG: usize = 80;

/// Languages offered, by Google's code.
const LANGUAGES: [(&str, &str); 16] = [
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("en", "English"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
    ("pt", "Portuguese"),
    ("it", "Italian"),
    ("ru", "Russian"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("id", "Indonesian"),
];

/// The target picked last, kept while the launcher runs.
static TARGET: Mutex<Option<String>> = Mutex::new(None);

fn language_name(code: &str) -> String {
    let base = code.split('-').next().unwrap_or(code);
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .or_else(|| LANGUAGES.iter().find(|(known, _)| known.starts_with(base)))
        .map_or_else(|| code.to_owned(), |(_, name)| (*name).to_owned())
}

fn is_cjk_char(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF)
}

/// Whether `text` is mostly Chinese, Japanese or Korean: a character of
/// those weighs as much as a word of other scripts.
fn is_cjk(text: &str) -> bool {
    let cjk = text.chars().filter(|c| is_cjk_char(*c)).count();
    let words = text
        .split(|c: char| !c.is_alphanumeric() || is_cjk_char(c))
        .filter(|word| !word.is_empty())
        .count();
    cjk > 0 && cjk >= words
}

/// The language `text` goes to when none is picked.
fn automatic_target(text: &str) -> &'static str {
    match is_cjk(text) {
        true => "en",
        false => "zh-CN",
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Translation {
    source: String,
    text: String,
    /// The detected source language's code.
    from: String,
    to: String,
}

/// Reads the Chrome dictionary endpoint's answer: `[["translated", "en"]]`.
fn parse_chrome(body: &serde_json::Value) -> Option<(String, String)> {
    let first = body.get(0)?;
    let (text, from) = match first.as_array() {
        Some(pair) => (
            pair.first()?.as_str()?,
            pair.get(1).and_then(|from| from.as_str()),
        ),
        None => (first.as_str()?, None),
    };
    (!text.is_empty()).then(|| (text.to_owned(), from.unwrap_or("").to_owned()))
}

/// Reads Google's answer: `[[["translated", "source", …], …], …, "en", …]`.
fn parse_google(body: &serde_json::Value) -> Option<(String, String)> {
    let text: String = body
        .get(0)?
        .as_array()?
        .iter()
        .filter_map(|part| part.get(0)?.as_str())
        .collect();
    let from = body.get(2).and_then(|from| from.as_str()).unwrap_or("");
    (!text.is_empty()).then(|| (text, from.to_owned()))
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?)
}

/// The endpoint Chrome's dictionary extension uses; it is limited less
/// than the one below.
fn google_chrome(text: &str, to: &str) -> Result<(String, String)> {
    let body: serde_json::Value = client()?
        .get("https://clients5.google.com/translate_a/t")
        .query(&[
            ("client", "dict-chrome-ex"),
            ("sl", "auto"),
            ("tl", to),
            ("q", text),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    parse_chrome(&body).context("Google Translate gave no translation")
}

fn google(text: &str, to: &str) -> Result<(String, String)> {
    let body: serde_json::Value = client()?
        .get("https://translate.googleapis.com/translate_a/single")
        .query(&[
            ("client", "gtx"),
            ("sl", "auto"),
            ("tl", to),
            ("dt", "t"),
            ("q", text),
        ])
        .send()?
        .error_for_status()?
        .json()?;
    parse_google(&body).context("Google Translate gave no translation")
}

fn my_memory(text: &str, to: &str) -> Result<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Response {
        #[serde(rename = "responseData")]
        data: Data,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        #[serde(rename = "translatedText")]
        text: String,
    }
    // MyMemory cannot detect the language; it is guessed from the script.
    let from = match is_cjk(text) {
        true => "zh-CN",
        false => "en",
    };
    let response: Response = client()?
        .get("https://api.mymemory.translated.net/get")
        .query(&[("q", text), ("langpair", &format!("{from}|{to}"))])
        .send()?
        .error_for_status()?
        .json()?;
    Ok((response.data.text, from.to_owned()))
}

/// Translates `text` to `to`, or to the automatic target. Blocking.
fn translate(text: &str, to: Option<&str>) -> Result<Translation> {
    let to = to.unwrap_or_else(|| automatic_target(text)).to_owned();
    let (translated, from) = google_chrome(text, &to)
        .or_else(|error| {
            tracing::warn!("Google Translate failed, trying again: {error:#}");
            google(text, &to)
        })
        .or_else(|error| {
            tracing::warn!("Google Translate failed, trying MyMemory: {error:#}");
            my_memory(text, &to)
        })?;
    Ok(Translation {
        source: text.to_owned(),
        text: translated,
        from,
        to,
    })
}

pub fn translate_page(_: &mut Window, cx: &mut App) -> Result<PageHandle> {
    let selection = crate::selection::latest().map(|text| text.trim().to_owned());
    Ok(pages::handle(cx.new(|cx| {
        let mut page = TranslatePage {
            query: String::new(),
            selection,
            target: TARGET.lock().ok().and_then(|target| target.clone()),
            result: None,
            loading: false,
            task: None,
        };
        page.translate(cx);
        page
    })))
}

struct TranslatePage {
    query: String,
    /// What was selected when the launcher was summoned, translated while
    /// nothing is typed.
    selection: Option<String>,
    /// The language picked, or `None` for automatic.
    target: Option<String>,
    result: Option<Result<Translation, String>>,
    loading: bool,
    task: Option<Task<()>>,
}

impl TranslatePage {
    fn text(&self) -> Option<String> {
        let query = self.query.trim();
        match query.is_empty() {
            true => self.selection.clone().filter(|text| !text.is_empty()),
            false => Some(query.to_owned()),
        }
    }

    fn translate(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.text() else {
            self.task = None;
            self.result = None;
            self.loading = false;
            cx.notify();
            return;
        };
        let target = self.target.clone();
        let wait = !self.query.trim().is_empty();
        self.loading = true;
        cx.notify();
        self.task = Some(cx.spawn(async move |this, cx| {
            if wait {
                cx.background_executor().timer(DEBOUNCE).await;
            }
            let result = cx
                .background_spawn(async move {
                    translate(&text, target.as_deref()).map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |page, cx| {
                page.result = Some(result);
                page.loading = false;
                cx.notify();
            })
            .ok();
        }));
    }
}

fn translation_item(translation: &Translation) -> Item {
    let text: SharedString = translation.text.clone().into();
    let url = format!(
        "https://translate.google.com/?sl=auto&tl={}&text={}&op=translate",
        translation.to,
        percent_encoding::utf8_percent_encode(
            &translation.source,
            percent_encoding::NON_ALPHANUMERIC
        )
    );
    let item = Item::new(ItemId::new("translation"), translation.text.clone())
        .with_subtitle(format!(
            "{} → {}",
            language_name(&translation.from),
            language_name(&translation.to)
        ))
        .with_image(Image::Icon("languages".into()))
        .with_actions(
            ActionPanel::new()
                .with_action(
                    Action::new("Copy Translation", Effect::Copy(text.clone()))
                        .with_image(Image::Icon("copy".into())),
                )
                .with_action(
                    Action::new("Paste Translation", Effect::Paste(text))
                        .with_image(Image::Icon("clipboard-paste".into())),
                )
                .with_action(
                    Action::new(
                        "Copy Original Text",
                        Effect::Copy(translation.source.clone().into()),
                    )
                    .with_image(Image::Icon("text-quote".into()))
                    .with_shortcut("secondary-shift-c"),
                )
                .with_action(
                    Action::new("Open in Google Translate", Effect::OpenUrl(url.into()))
                        .with_image(Image::Icon("globe".into()))
                        .with_shortcut("secondary-o"),
                ),
        );
    match translation.text.chars().count() > LONG || translation.text.contains('\n') {
        true => item.with_detail(DetailModel::new(format!(
            "{}\n\n---\n\n{}",
            translation.text, translation.source
        ))),
        false => item,
    }
}

impl Page for TranslatePage {
    fn title(&self) -> SharedString {
        "Translate".into()
    }

    fn model(&mut self, _: &mut Window, cx: &mut Context<Self>) -> PageModel {
        let page = cx.entity().downgrade();
        let dropdown = LANGUAGES.iter().fold(
            Dropdown::new("Translate To")
                .with_choice(Choice::new(AUTOMATIC, "Automatic"))
                .with_value(self.target.clone().unwrap_or_else(|| AUTOMATIC.into()))
                .with_on_change(TextHandler::new(move |value, _, cx| {
                    page.update(cx, |page, cx| {
                        page.target = (value != AUTOMATIC).then(|| value.to_string());
                        if let Ok(mut target) = TARGET.lock() {
                            *target = page.target.clone();
                        }
                        page.translate(cx);
                    })
                    .ok();
                })),
            |dropdown, (code, name)| dropdown.with_choice(Choice::new(*code, *name)),
        );
        let list = ListModel::new()
            .with_placeholder("Enter text to translate…")
            .with_filtering(false)
            .with_loading(self.loading)
            .with_dropdown(dropdown);
        let showing_selection = self.query.trim().is_empty() && self.selection.is_some();
        match &self.result {
            Some(Ok(translation)) => {
                let item = translation_item(translation);
                let long = item.detail().is_some();
                list.with_showing_detail(long).with_section(
                    Section::new()
                        .with_title(match showing_selection {
                            true => "Selected Text",
                            false => "Translation",
                        })
                        .with_item(match showing_selection {
                            true => item.with_accessory(Accessory::text("Selection")),
                            false => item,
                        }),
                )
            }
            Some(Err(error)) => list
                .with_empty_title("Couldn’t translate")
                .with_empty_description(error.clone()),
            None => list
                .with_empty_title(match self.loading {
                    true => "Translating…",
                    false => "Type the text to translate",
                })
                .with_empty_description(
                    "Chinese, Japanese and Korean go to English, anything else to Chinese, unless you pick a language.",
                ),
        }
        .into()
    }

    fn set_query(&mut self, query: &str, _: &mut Window, cx: &mut Context<Self>) {
        if self.query != query {
            self.query = query.to_owned();
            self.translate(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_google_answers_and_picks_a_target() {
        let body = serde_json::json!([
            [
                ["你好，", "Hello, ", null, null],
                ["世界", "world", null, null]
            ],
            null,
            "en"
        ]);
        assert_eq!(
            parse_google(&body),
            Some(("你好，世界".to_owned(), "en".to_owned()))
        );
        assert_eq!(
            parse_chrome(&serde_json::json!([["早上好", "en"]])),
            Some(("早上好".to_owned(), "en".to_owned()))
        );
        assert_eq!(
            parse_chrome(&serde_json::json!(["早上好"])),
            Some(("早上好".to_owned(), String::new()))
        );
        assert_eq!(automatic_target("hello world"), "zh-CN");
        assert_eq!(automatic_target("the 中 word list"), "zh-CN");
        assert_eq!(automatic_target("你好 world"), "en");
        assert_eq!(automatic_target("こんにちは"), "en");
        assert_eq!(language_name("zh-CN"), "Chinese (Simplified)");
        assert_eq!(language_name("en-GB"), "English");
    }

    /// Asks the real services. Run by hand: needs the network.
    #[test]
    #[ignore = "needs the network"]
    fn test_translates_by_network() {
        println!("{:?}", google_chrome("Good morning", "zh-CN"));
        println!("{:?}", google_chrome("今天天气很好", "en"));
        println!("{:?}", google("Good morning", "zh-CN"));
        println!("{:?}", my_memory("Good morning", "zh-CN"));
    }
}
