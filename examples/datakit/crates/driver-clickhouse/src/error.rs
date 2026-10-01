//! ClickHouse's exceptions as [`DatabaseError`]s.
//!
//! The HTTP interface reports an exception as text:
//!
//! ```text
//! Code: 62. DB::Exception: Syntax error: failed at position 8 ('FRM') (line 1, col 8): FRM t.
//! Expected one of: … (SYNTAX_ERROR) (version 24.3.1.2672 (official build))
//! ```
//!
//! with the code also in the `X-ClickHouse-Exception-Code` header when the
//! exception happened before the response started. The message keeps what a
//! person needs: the version suffix and the error's constant name are
//! dropped, the code stays as the number, and a syntax error's list of
//! expected tokens becomes the detail.

use datakit_driver::DatabaseError;

/// Codes whose message names, in quotes, the identifier the statement got
/// wrong, which is then found in the statement to place the error.
const NAMES_AN_IDENTIFIER: &[&str] = &[
    "16", // NO_SUCH_COLUMN_IN_TABLE
    "46", // UNKNOWN_FUNCTION
    "47", // UNKNOWN_IDENTIFIER
    "60", // UNKNOWN_TABLE
    "81", // UNKNOWN_DATABASE
];

/// Whether `line` of a response body starts an exception, which the server
/// writes after the rows it had already sent.
///
/// Servers from 25.x frame it between `__exception__` markers; older ones
/// write the bare `Code: …` text. A value that happens to read like an
/// exception, in a single-column result, is mistaken for one.
pub(crate) fn starts_exception(line: &[u8]) -> bool {
    if line.starts_with(b"__exception__") {
        return true;
    }
    let Some(rest) = line.strip_prefix(b"Code: ") else {
        return false;
    };
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    digits > 0 && rest[digits..].starts_with(b". DB::Exception")
}

/// The error a response with status `status` reports in `body`. `code` is the
/// `X-ClickHouse-Exception-Code` header, when there is one.
pub(crate) fn from_response(
    status: u16,
    code: Option<&str>,
    body: &str,
    sql: &str,
) -> anyhow::Error {
    if body.contains("Code: ") || code.is_some() {
        return exception(body, code, sql).into();
    }
    match body.trim() {
        "" => anyhow::anyhow!("ClickHouse answered with HTTP status {status}"),
        body => anyhow::anyhow!("ClickHouse answered with HTTP status {status}: {body}"),
    }
}

/// The exception in `text`, placed in `sql` when the message says where.
pub(crate) fn exception(text: &str, code: Option<&str>, sql: &str) -> DatabaseError {
    let mut text = &text[text.find("Code: ").unwrap_or(0)..];
    // A framed exception ends with its length and a tag on a line of their
    // own, then the closing marker.
    if let Some(end) = text.find("__exception__") {
        text = &text[..end];
    }
    text = text.trim_end();
    if let Some((message, last)) = text.rsplit_once('\n')
        && is_frame_trailer(last)
    {
        text = message.trim_end();
    }

    let code = code
        .map(str::to_string)
        .or_else(|| number_after(text, "Code: ").map(|code| code.to_string()));
    let message = match text.split_once("DB::Exception: ") {
        Some((_, message)) => message,
        None => text
            .strip_prefix("Code: ")
            .and_then(|rest| rest.split_once(". "))
            .map_or(text, |(_, message)| message),
    };
    let message = match message.rfind(" (version ") {
        Some(end) => &message[..end],
        None => message,
    };
    let message = strip_constant_name(message.trim_end());
    let position = position(message, code.as_deref(), sql);

    let (message, detail) = match message.split_once(". Expected ") {
        Some((message, expected)) => (message, Some(format!("Expected {expected}"))),
        None => (message, None),
    };
    let mut error = DatabaseError::new(message.trim());
    if let Some(code) = code {
        error = error.with_code(code);
    }
    if let Some(detail) = detail {
        error = error.with_detail(detail);
    }
    if let Some(position) = position {
        error = error.with_position(position);
    }
    error
}

/// `message` without the `(SYNTAX_ERROR)` that names the code's constant.
fn strip_constant_name(message: &str) -> &str {
    let Some(open) = message.strip_suffix(')').and_then(|rest| rest.rfind(" (")) else {
        return message;
    };
    let name = &message[open + 2..message.len() - 1];
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        &message[..open]
    } else {
        message
    }
}

/// Whether `line` is the `<length> <tag>` line that ends a framed exception.
fn is_frame_trailer(line: &str) -> bool {
    let mut words = line.split_whitespace();
    matches!(
        (words.next(), words.next(), words.next()),
        (Some(length), Some(_), None) if length.parse::<usize>().is_ok()
    )
}

/// The byte offset in `sql` the message points at.
///
/// A syntax error says `failed at position N`, counting bytes from 1, and
/// newer servers add `(line L, col C)`. An unknown name is found by searching
/// the statement for it. Input-data errors that say `(at row R)` count rows
/// of inserted data, not of the statement, and are not placed.
fn position(message: &str, code: Option<&str>, sql: &str) -> Option<usize> {
    if let Some(position) = number_after(message, "failed at position ") {
        return Some(char_boundary(sql, position.saturating_sub(1)));
    }
    if let Some(line) = number_after(message, "(line ")
        && let Some(column) = number_after(message, &format!("(line {line}, col "))
    {
        let start: usize = sql
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum();
        return Some(char_boundary(sql, start + column.saturating_sub(1)));
    }
    if code.is_some_and(|code| NAMES_AN_IDENTIFIER.contains(&code)) {
        let name = quoted_name(message).or_else(|| {
            let rest = &message[message.find("Table ")? + "Table ".len()..];
            rest.strip_suffix(" does not exist")
                .or_else(|| rest.split_once(" does not exist").map(|(name, _)| name))
        })?;
        return find_word(sql, name)
            .or_else(|| find_word(sql, name.rsplit('.').next().unwrap_or(name)));
    }
    None
}

/// The first name in backticks or single quotes in `message`.
fn quoted_name(message: &str) -> Option<&str> {
    let start = message.find(['`', '\''])?;
    let quote = message[start..].chars().next()?;
    let rest = &message[start + 1..];
    let end = rest.find(quote)?;
    Some(&rest[..end]).filter(|name| !name.is_empty())
}

/// The byte offset of the first occurrence of `word` in `sql` that is not
/// part of a longer word.
fn find_word(sql: &str, word: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    sql.match_indices(word).map(|(ix, _)| ix).find(|&ix| {
        !sql[..ix].chars().next_back().is_some_and(is_word)
            && !sql[ix + word.len()..].chars().next().is_some_and(is_word)
    })
}

fn number_after(text: &str, prefix: &str) -> Option<usize> {
    let rest = &text[text.find(prefix)? + prefix.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// `offset`, within `sql` and on a character boundary.
fn char_boundary(sql: &str, offset: usize) -> usize {
    let mut offset = offset.min(sql.len());
    while !sql.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_syntax_error_is_placed_and_its_expectations_become_the_detail() {
        let sql = "SELECT * FRM t";
        let error = exception(
            "Code: 62. DB::Exception: Syntax error: failed at position 10 ('FRM') \
             (line 1, col 10): FRM t. Expected one of: FROM, WHERE. (SYNTAX_ERROR) \
             (version 24.3.1.2672 (official build))\n",
            Some("62"),
            sql,
        );
        assert_eq!(error.code(), Some("62"));
        assert_eq!(
            error.message(),
            "Syntax error: failed at position 10 ('FRM') (line 1, col 10): FRM t"
        );
        assert_eq!(error.detail(), Some("Expected one of: FROM, WHERE."));
        assert_eq!(&sql[error.position().unwrap()..], "FRM t");
    }

    #[test]
    fn line_and_column_count_from_the_start_of_the_line() {
        let sql = "SELECT 1\nFROM t\nWHERE ?";
        let error = exception(
            "Code: 62. DB::Exception: Syntax error (line 3, col 7): ?. (SYNTAX_ERROR)",
            None,
            sql,
        );
        assert_eq!(&sql[error.position().unwrap()..], "?");
    }

    #[test]
    fn an_unknown_name_is_found_in_the_statement() {
        let sql = "SELECT nope_too, nope FROM system.one";
        let error = exception(
            "Code: 47. DB::Exception: Unknown expression identifier `nope` in scope \
             SELECT nope_too, nope FROM system.one. (UNKNOWN_IDENTIFIER) (version 24.8.1.1)",
            Some("47"),
            sql,
        );
        assert_eq!(&sql[error.position().unwrap()..][..4], "nope");
        assert_eq!(error.position(), Some(17));

        let sql = "SELECT * FROM shop.nope";
        let error = exception(
            "Code: 60. DB::Exception: Table shop.nope does not exist. (UNKNOWN_TABLE)",
            None,
            sql,
        );
        assert_eq!(error.code(), Some("60"));
        assert_eq!(error.message(), "Table shop.nope does not exist.");
        assert_eq!(&sql[error.position().unwrap()..], "shop.nope");
    }

    #[test]
    fn a_framed_exception_loses_its_frame() {
        let text = "__exception__\r\nk3j4\r\nCode: 395. DB::Exception: Value passed to \
                    'throwIf' function is non-zero. (FUNCTION_THROW_IF_VALUE_IS_NON_ZERO) \
                    (version 25.6.1.1)\r\n123 k3j4\r\n__exception__\r\n";
        assert!(starts_exception(b"__exception__\r"));
        let error = exception(text, None, "");
        assert_eq!(error.code(), Some("395"));
        assert_eq!(
            error.message(),
            "Value passed to 'throwIf' function is non-zero."
        );
        assert_eq!(error.position(), None);
    }

    #[test]
    fn only_exception_text_starts_an_exception() {
        assert!(starts_exception(b"Code: 241. DB::Exception: Memory limit"));
        assert!(!starts_exception(b"Code: none"));
        assert!(!starts_exception(b"1\tCode: 1. DB::Exception"));
    }

    #[test]
    fn a_response_without_an_exception_reports_its_status() {
        let error = from_response(502, None, "Bad Gateway", "SELECT 1");
        assert!(error.downcast_ref::<DatabaseError>().is_none());
        assert_eq!(
            error.to_string(),
            "ClickHouse answered with HTTP status 502: Bad Gateway"
        );
        let error = from_response(
            516,
            Some("516"),
            "Code: 516. DB::Exception: default: Authentication failed. (AUTHENTICATION_FAILED)",
            "",
        );
        let error = error.downcast_ref::<DatabaseError>().unwrap();
        assert_eq!(error.message(), "default: Authentication failed.");
    }
}
