//! Reading the SQL text ClickHouse keeps for its objects: the
//! `create_table_query` of a table, a key expression, a function's
//! `create_query`.
//!
//! Only enough of the grammar to find the parts DataKit needs: quotes,
//! brackets and whole words. Text inside a quoted string or identifier, or
//! inside brackets, is never mistaken for a separator or a keyword.

/// The parts of `text` between the occurrences of `separator` that are
/// outside quotes and brackets, trimmed. Empty text has no parts.
pub(crate) fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (ix, c) in top_level_chars(text) {
        if c == separator {
            parts.push(text[start..ix].trim());
            start = ix + c.len_utf8();
        }
    }
    let last = text[start..].trim();
    if !last.is_empty() || !parts.is_empty() {
        parts.push(last);
    }
    parts
}

/// The byte offset of the first whole-word, case-insensitive occurrence of
/// `keyword` outside quotes and brackets.
pub(crate) fn find_keyword(text: &str, keyword: &str) -> Option<usize> {
    top_level_chars(text).map(|(ix, _)| ix).find(|&ix| {
        text.get(ix..ix + keyword.len())
            .is_some_and(|word| word.eq_ignore_ascii_case(keyword))
            && !text[..ix].chars().next_back().is_some_and(is_word_char)
            && !text[ix + keyword.len()..]
                .chars()
                .next()
                .is_some_and(is_word_char)
    })
}

/// The contents of the bracketed group `text` starts with, and the text
/// after its closing bracket; `None` when `text` does not start with `(`
/// or the group is not closed.
pub(crate) fn bracketed(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if !text.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    for (ix, c) in chars_outside_quotes(text) {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&text[1..ix], &text[ix + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

/// `text` after the identifier it starts with, which may be quoted and may be
/// qualified (`db`.`table`).
pub(crate) fn skip_name(text: &str) -> &str {
    let mut rest = text.trim_start();
    loop {
        rest = match rest.chars().next() {
            Some(quote @ ('`' | '"')) => {
                let mut end = rest.len();
                let mut escaped = false;
                for (ix, c) in rest.char_indices().skip(1) {
                    if escaped {
                        escaped = false;
                    } else if c == '\\' {
                        escaped = true;
                    } else if c == quote {
                        end = ix + 1;
                        break;
                    }
                }
                &rest[end..]
            }
            _ => {
                let end = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
                &rest[end..]
            }
        };
        match rest.strip_prefix('.') {
            Some(after) => rest = after,
            None => return rest,
        }
    }
}

/// `text` without the backticks or double quotes around it, with the
/// escapes inside them resolved; plain text as it is.
pub(crate) fn unquote(text: &str) -> String {
    let text = text.trim();
    let quoted = text.len() >= 2
        && ((text.starts_with('`') && text.ends_with('`'))
            || (text.starts_with('"') && text.ends_with('"')));
    if !quoted {
        return text.to_string();
    }
    let quote = text.as_bytes()[0] as char;
    let inner = &text[1..text.len() - 1];
    let mut unquoted = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            unquoted.extend(chars.next());
        } else {
            unquoted.push(c);
            // A doubled quote stands for one.
            if c == quote && chars.as_str().starts_with(quote) {
                chars.next();
            }
        }
    }
    unquoted
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every character of `text` outside quotes and brackets, with its offset.
fn top_level_chars(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut depth = 0usize;
    chars_outside_quotes(text).filter(move |&(_, c)| match c {
        '(' | '[' | '{' => {
            depth += 1;
            false
        }
        ')' | ']' | '}' => {
            depth = depth.saturating_sub(1);
            false
        }
        _ => depth == 0,
    })
}

/// Every character of `text` that is not inside a quoted string or
/// identifier, with its offset. The quotes themselves are skipped.
fn chars_outside_quotes(text: &str) -> impl Iterator<Item = (usize, char)> + '_ {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    text.char_indices().filter(move |&(_, c)| {
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == open {
                quote = None;
            }
            return false;
        }
        if matches!(c, '\'' | '"' | '`') {
            quote = Some(c);
            return false;
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_ignores_separators_inside_brackets_and_quotes() {
        assert_eq!(
            split_top_level("id, toDate(ts, 'a,b'), `x,y`", ','),
            ["id", "toDate(ts, 'a,b')", "`x,y`"]
        );
        assert!(split_top_level("  ", ',').is_empty());
    }

    #[test]
    fn keywords_are_found_as_whole_words_at_the_top_level() {
        let query = "CREATE VIEW db.v (`AS` UInt8, alias String) AS SELECT 1 AS a";
        let ix = find_keyword(query, "AS").unwrap();
        assert_eq!(&query[ix..], "AS SELECT 1 AS a");
        assert_eq!(find_keyword("SELECT 'AS'", "as"), None);
    }

    #[test]
    fn a_bracketed_group_ends_at_its_matching_bracket() {
        assert_eq!(
            bracketed(" (a Array(String), b String DEFAULT ')') ENGINE = Log"),
            Some(("a Array(String), b String DEFAULT ')'", " ENGINE = Log"))
        );
        assert_eq!(bracketed("x"), None);
    }

    #[test]
    fn names_are_skipped_whole_and_unquoted() {
        assert_eq!(skip_name("db.`my \\` table` TO x"), " TO x");
        assert_eq!(skip_name("plain (a UInt8)"), " (a UInt8)");
        assert_eq!(unquote("`my \\` table`"), "my ` table");
        assert_eq!(unquote("plain"), "plain");
    }
}
