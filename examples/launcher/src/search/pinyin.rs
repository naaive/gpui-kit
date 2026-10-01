//! Pinyin spellings of Chinese text, so `weixin` and `wx` both find 微信.
//!
//! A title is spelled once, with a space between syllables — "wei xin" — and
//! then scored by the ordinary matcher. The spaces make every syllable a word
//! start, so initials (`wx`, `xtsz`) and full pinyin (`weixin`) rank the same
//! way `vsc` and `code` rank "Visual Studio Code", with no second scorer.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use gpui_kit::SharedString;
use pinyin::{ToPinyin as _, ToPinyinMulti as _};

/// Spellings kept before the cache starts over. Titles are few (hundreds of
/// applications and commands), so this only bounds lists that churn, such as
/// an extension streaming search results.
const CACHE_CAPACITY: usize = 8192;
/// A title with several characters that have several readings gets one
/// spelling per alternative reading, up to this many in all.
const MAX_SPELLINGS: usize = 8;

/// The spellings of a text: the most common reading first.
pub type Spellings = Arc<[Box<str>]>;

/// Spelling a title takes a table lookup per character; typing re-scores every
/// item per keystroke, so each title is spelled once and kept, keyed by the
/// shared string itself (cloning a `SharedString` is a reference count).
fn cache() -> &'static Mutex<HashMap<SharedString, Spellings>> {
    static CACHE: OnceLock<Mutex<HashMap<SharedString, Spellings>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Whether `text` has any character pinyin can spell. Checked before the
/// cache, so Latin-only text never takes the lock.
fn has_han(text: &str) -> bool {
    text.chars().any(|character| {
        matches!(character,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'
            | '\u{20000}'..='\u{2ebef}')
    })
}

/// The cached pinyin spellings of `text`, or `None` when it has no Chinese.
///
/// Safe to call from a background thread, which is how a source warms the
/// cache for the items it just loaded.
pub fn spellings(text: &SharedString) -> Option<Spellings> {
    if !has_han(text) {
        return None;
    }
    let mut cache = cache().lock().unwrap_or_else(|poison| poison.into_inner());
    if let Some(spelled) = cache.get(text) {
        return Some(spelled.clone());
    }
    if cache.len() >= CACHE_CAPACITY {
        cache.clear();
    }
    let spelled: Spellings = spell(text).into_iter().map(Into::into).collect();
    cache.insert(text.clone(), spelled.clone());
    Some(spelled)
}

/// One character of a title: a Chinese character's plain readings (most
/// common first), or any other character as is.
enum Segment {
    Syllable(Vec<&'static str>),
    Other(char),
}

/// Spells every Chinese character and separates each syllable from its
/// neighbours; other text is kept as is, so "QQ音乐" becomes "QQ yin yue".
///
/// The first spelling uses each character's most common reading. A character
/// with several readings (乐 is `le` and `yue`) adds a spelling with each
/// other reading in its place, one character at a time: 音乐 is found by
/// `yinyue` without spelling every combination of every character.
pub fn spell(text: &str) -> Vec<String> {
    let segments: Vec<Segment> = text
        .chars()
        .map(|character| match character.to_pinyin_multi() {
            Some(readings) => {
                let default = character.to_pinyin().map(|pinyin| pinyin.plain());
                let mut plain: Vec<&'static str> = default.into_iter().collect();
                for reading in readings {
                    if !plain.contains(&reading.plain()) {
                        plain.push(reading.plain());
                    }
                }
                Segment::Syllable(plain)
            }
            None => Segment::Other(character),
        })
        .collect();

    let mut spellings = vec![render(&segments, None)];
    for (ix, segment) in segments.iter().enumerate() {
        if let Segment::Syllable(readings) = segment {
            for reading in readings.iter().skip(1) {
                if spellings.len() == MAX_SPELLINGS {
                    return spellings;
                }
                spellings.push(render(&segments, Some((ix, reading))));
            }
        }
    }
    spellings
}

/// Joins the segments, reading the character at `substitute.0` as
/// `substitute.1` instead of its most common reading.
fn render(segments: &[Segment], substitute: Option<(usize, &str)>) -> String {
    let mut spelled = String::new();
    let mut after_syllable = false;
    for (ix, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Syllable(readings) => {
                if !spelled.is_empty() && !spelled.ends_with(' ') {
                    spelled.push(' ');
                }
                let reading = match substitute {
                    Some((substitute_ix, reading)) if substitute_ix == ix => reading,
                    _ => readings.first().copied().unwrap_or_default(),
                };
                spelled.push_str(reading);
                after_syllable = true;
            }
            Segment::Other(character) => {
                if after_syllable && !character.is_whitespace() {
                    spelled.push(' ');
                }
                spelled.push(*character);
                after_syllable = false;
            }
        }
    }
    spelled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spells_syllables_apart_and_keeps_latin() {
        assert_eq!(spell("微信")[0], "wei xin");
        assert_eq!(spell("系统设置")[0], "xi tong she zhi");
        assert_eq!(spell("网易云 Beta")[0], "wang yi yun Beta");
        assert_eq!(spell("Visual Studio Code"), ["Visual Studio Code"]);
    }

    #[test]
    fn test_characters_with_several_readings_are_spelled_each_way() {
        let music = spell("QQ音乐");
        assert!(music.contains(&"QQ yin yue".to_owned()), "{music:?}");
        assert!(music.contains(&"QQ yin le".to_owned()), "{music:?}");
    }

    #[test]
    fn test_spellings_are_cached_only_for_chinese() {
        assert!(spellings(&"Terminal".into()).is_none());
        let first = spellings(&"钉钉".into()).unwrap();
        let second = spellings(&"钉钉".into()).unwrap();
        assert!(Arc::ptr_eq(&first, &second), "the second lookup is cached");
    }
}
