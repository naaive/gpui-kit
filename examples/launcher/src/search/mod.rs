//! Matching a query against item text, and learning from what the user picks.
//! Pure Rust, no UI.
//!
//! The scorer is deliberately small: case-insensitive subsequence matching with
//! bonuses for a match at the start of the text, at the start of a word, and
//! for consecutive characters. That already ranks `vsc` → "Visual Studio Code"
//! and `code` → "Visual Studio Code" sensibly. Chinese text is also scored by
//! its pinyin spelling ([`pinyin`]), and the root search adds how often and how
//! recently an item was used ([`frecency`]).

mod frecency;
mod matcher;
pub mod pinyin;

pub use frecency::{UsageStore, now, write_snapshot};
pub use matcher::{Score, score};

use crate::model::Item;

/// Scores an item against a query: the best score among its title, subtitle
/// and keywords, or `None` when none of them matches.
pub fn score_item(query: &str, item: &Item) -> Option<Score> {
    std::iter::once(item.title())
        .chain(item.subtitle())
        .chain(item.keywords())
        .filter_map(|text| score_text(query, text))
        .max()
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
    fn test_keywords_in_chinese_match_by_pinyin() {
        let terminal = item("Terminal").with_keyword("终端");
        assert!(score_item("zhongduan", &terminal).is_some());
    }
}
