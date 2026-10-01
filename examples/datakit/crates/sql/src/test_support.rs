//! Helpers for this crate's tests.

use datakit_driver::Dialect;

/// A dialect with PostgreSQL's quoting and a few reserved words.
pub(crate) struct Plain;

impl Dialect for Plain {
    fn reserved_words(&self) -> &'static [&'static str] {
        &[
            "SELECT", "FROM", "WHERE", "AS", "ON", "JOIN", "USER", "ORDER", "AND", "OR", "NOT",
            "NULL",
        ]
    }

    fn keywords(&self) -> &'static [&'static str] {
        &["SELECT", "FROM", "WHERE", "ORDER BY"]
    }

    fn functions(&self) -> &'static [&'static str] {
        &["count", "coalesce", "now"]
    }
}

/// The text with `|` removed, and where it was.
pub(crate) fn caret(text: &str) -> (String, usize) {
    let offset = text.find('|').expect("a caret");
    (text.replacen('|', "", 1), offset)
}
