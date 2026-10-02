//! Matching a query against item text, and learning from what the user picks.
//! Pure Rust, no UI.
//!
//! The scorer is deliberately small: case-insensitive subsequence matching with
//! bonuses for a match at the start of the text, at the start of a word, and
//! for consecutive characters. That already ranks `vsc` → "Visual Studio Code"
//! and `code` → "Visual Studio Code" sensibly. Chinese text is also scored by
//! its pinyin spelling ([`pinyin`]), and the root search adds how often and how
//! recently an item was used ([`frecency`]). How loosely the root search
//! matches is the user's [`SearchSensitivity`].

mod frecency;
mod matcher;
pub mod pinyin;

pub use frecency::{UsageStore, now, write_snapshot};
pub use matcher::{Score, score};

use crate::{model::Item, shell::settings::SearchSensitivity};

/// Scores an item against a query: the best score among its title, subtitle
/// and keywords, or `None` when none of them matches.
pub fn score_item(query: &str, item: &Item) -> Option<Score> {
    std::iter::once(item.title())
        .chain(item.subtitle())
        .chain(item.keywords())
        .filter_map(|text| score_text(query, text))
        .max()
}

/// Scores an item as loosely as `sensitivity` allows.
///
/// - Low: the title or a keyword must contain every word typed.
/// - Medium: [`score_item`], the query's letters in order.
/// - High: also the words in any order, or with one letter wrong, scored
///   below every match Medium would make.
pub fn score_item_with(query: &str, item: &Item, sensitivity: SearchSensitivity) -> Option<Score> {
    match sensitivity {
        SearchSensitivity::Low => {
            let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
            std::iter::once(item.title())
                .chain(item.keywords())
                .filter(|text| contains_words(text, &words))
                // Words typed out of order still count, with the least score.
                .map(|text| score_text(query, text).unwrap_or(1))
                .max()
        }
        SearchSensitivity::Medium => score_item(query, item),
        SearchSensitivity::High => score_item(query, item).or_else(|| {
            std::iter::once(item.title())
                .chain(item.subtitle())
                .chain(item.keywords())
                .filter_map(|text| loose_score(query, text))
                .max()
        }),
    }
}

/// Whether `text`, or its pinyin, contains each of `words` (lowercase).
fn contains_words(text: &gpui_kit::SharedString, words: &[String]) -> bool {
    let contains = |text: &str| {
        let text = text.to_lowercase();
        words.iter().all(|word| text.contains(word.as_str()))
    };
    contains(text)
        || pinyin::spellings(text)
            .is_some_and(|spellings| spellings.iter().any(|spelling| contains(spelling)))
}

/// A match Medium misses: each word of the query found on its own, or the
/// query with one letter left out (a typo or a swap, from four letters on).
/// Halved, so it ranks below the strict matches.
fn loose_score(query: &str, text: &str) -> Option<Score> {
    let words: Vec<&str> = query.split_whitespace().collect();
    let any_order = (words.len() > 1)
        .then(|| {
            words
                .iter()
                .map(|word| score(word, text))
                .sum::<Option<Score>>()
        })
        .flatten();
    let letters: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let one_off = (letters.len() >= 4)
        .then(|| {
            (0..letters.len())
                .filter_map(|skipped| {
                    let rest: String = letters
                        .iter()
                        .enumerate()
                        .filter(|(ix, _)| *ix != skipped)
                        .map(|(_, c)| c)
                        .collect();
                    score(&rest, text)
                })
                .max()
        })
        .flatten();
    any_order.max(one_off).map(|score| score / 2)
}

/// Scores one text as written and, when it is Chinese, by its pinyin, so
/// `微信`, `weixin` and `wx` all find "微信".
pub fn score_text(query: &str, text: &gpui_kit::SharedString) -> Option<Score> {
    let direct = score(query, text);
    let spelled = pinyin::spellings(text).and_then(|spellings| {
        spellings
            .iter()
            .filter_map(|spelling| score(query, spelling))
            .max()
    });
    direct.max(spelled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemId;

    fn item(title: &str) -> Item {
        Item::new(ItemId::new(title.to_owned()), title.to_owned())
    }

    #[test]
    fn test_chinese_titles_match_by_pinyin_and_initials() {
        let wechat = item("微信");
        assert!(score_item("weixin", &wechat).is_some());
        assert!(score_item("wx", &wechat).is_some());
        assert!(score_item("微信", &wechat).is_some());
        assert!(score_item("wy", &wechat).is_none());

        let settings = item("系统设置");
        assert!(score_item("xtsz", &settings).is_some());
        assert!(score_item("xitong", &settings).is_some());
        assert!(score_item("shezhi", &settings).is_some());
    }

    #[test]
    fn test_pinyin_mixes_with_latin_text() {
        let music = item("QQ音乐");
        assert!(score_item("qqyy", &music).is_some());
        assert!(score_item("qq yinyue", &music).is_some());
        // Initials rank a pinyin title like initials rank an English one.
        assert!(score_item("wx", &item("微信")) > score_item("wx", &item("Wax")));
    }

    #[test]
    fn test_sensitivity_widens_or_narrows_matches() {
        let code = item("Visual Studio Code")
            .with_subtitle("Application")
            .with_keyword("editor");
        let score = |query, sensitivity| score_item_with(query, &code, sensitivity);
        let (low, medium, high) = (
            SearchSensitivity::Low,
            SearchSensitivity::Medium,
            SearchSensitivity::High,
        );

        // Words contained in the title or a keyword.
        assert!(score("studio", low).is_some());
        assert!(score("code studio", low).is_some());
        assert!(score("edit", low).is_some());
        assert!(score("vsc", low).is_none(), "initials are not words");
        assert!(score("application", low).is_none(), "nor is the subtitle");
        assert!(score("vsc", medium).is_some());

        // Today's subsequence match, then looser ones ranked below it.
        assert!(score("code studio", medium).is_none());
        assert!(score("code studio", high).is_some());
        assert!(score("studoi", medium).is_none());
        assert!(score("studoi", high).is_some(), "one letter off");
        assert!(score("stuxyi", high).is_none(), "two letters off");
        assert!(score("studio", high) > score("studoi", high));
        assert_eq!(score("vsc", high), score("vsc", medium));
    }

    #[test]
    fn test_keywords_in_chinese_match_by_pinyin() {
        let terminal = item("Terminal").with_keyword("终端");
        assert!(score_item("zhongduan", &terminal).is_some());
    }
}
