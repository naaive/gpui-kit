//! Reformatting SQL text.

use sqlformat::{FormatOptions, Indent, QueryParams};

/// How formatted SQL looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormatStyle {
    uppercase_keywords: bool,
    indent: u8,
}

impl Default for FormatStyle {
    fn default() -> Self {
        Self {
            uppercase_keywords: true,
            indent: 4,
        }
    }
}

impl FormatStyle {
    pub fn uppercase_keywords(mut self, uppercase: bool) -> Self {
        self.uppercase_keywords = uppercase;
        self
    }

    pub fn with_indent(mut self, spaces: u8) -> Self {
        self.indent = spaces;
        self
    }

    pub fn is_uppercase_keywords(&self) -> bool {
        self.uppercase_keywords
    }

    pub fn indent(&self) -> u8 {
        self.indent
    }
}

/// `text` laid out one clause per line, with keywords in one case.
/// Statements stay apart by a blank line; comments and strings are kept.
pub fn format_sql(text: &str, style: FormatStyle) -> String {
    let options = FormatOptions {
        indent: Indent::Spaces(style.indent),
        uppercase: Some(style.uppercase_keywords),
        lines_between_queries: 2,
        ..FormatOptions::default()
    };
    let formatted = sqlformat::format(text, &QueryParams::None, &options);
    let mut formatted = formatted.trim_end().to_string();
    if text.ends_with('\n') {
        formatted.push('\n');
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clauses_go_on_their_own_lines() {
        let formatted = format_sql(
            "select id, name from customers where id = 1 order by name",
            FormatStyle::default(),
        );
        assert!(formatted.starts_with("SELECT"));
        assert!(formatted.contains("\nFROM"));
        assert!(formatted.contains("\nWHERE"));
        assert!(formatted.contains("customers"));
    }

    #[test]
    fn strings_and_comments_survive() {
        let formatted = format_sql(
            "-- keep me\nselect 'It''s a $1' as x;",
            FormatStyle::default().uppercase_keywords(false),
        );
        assert!(formatted.contains("-- keep me"));
        assert!(formatted.contains("'It''s a $1'"));
        assert!(formatted.contains("select"));
    }
}
