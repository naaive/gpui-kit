//! Matching a query against item text. Pure Rust, no UI.
//!
//! The scorer is deliberately small: case-insensitive subsequence matching with
//! bonuses for a match at the start of the text, at the start of a word, and
//! for consecutive characters. That already ranks `vsc` → "Visual Studio Code"
//! and `code` → "Visual Studio Code" sensibly. Pinyin and frecency layer on top
//! of this in a later milestone.

mod matcher;

pub use matcher::{Score, score};

use crate::model::Item;

/// Scores an item against a query: the best score among its title, subtitle
/// and keywords, or `None` when none of them matches.
pub fn score_item(query: &str, item: &Item) -> Option<Score> {
    std::iter::once(item.title())
        .chain(item.subtitle())
        .chain(item.keywords())
        .filter_map(|text| score(query, text))
        .max()
}
