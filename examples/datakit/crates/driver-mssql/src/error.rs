use datakit_driver::DatabaseError;
use tiberius::error::{Error, TokenError};

/// `error` as a [`DatabaseError`] when the server reported it, so the console
/// can point at it; otherwise as it is.
///
/// SQL Server reports the line of the batch an error is on, not a column.
/// The position is the first character of that line in `sql`, which is the
/// whole statement the driver was given; `batch` is where in `sql` the text
/// the server ran begins, since a plan request runs only part of it. An error
/// raised inside a procedure or trigger counts lines of that module, not of
/// the batch, so it gets no position. The detail is what Management Studio
/// prints above the message: `Msg 208, Level 16, State 1, Line 3`.
pub(crate) fn convert(error: Error, sql: &str, batch: usize) -> anyhow::Error {
    let Error::Server(token) = error else {
        return anyhow::Error::new(error);
    };
    anyhow::Error::new(database_error(&token, sql, batch))
}

pub(crate) fn database_error(token: &TokenError, sql: &str, batch: usize) -> DatabaseError {
    let mut detail = format!(
        "Msg {}, Level {}, State {}",
        token.code(),
        token.class(),
        token.state()
    );
    if !token.procedure().is_empty() {
        detail.push_str(&format!(", Procedure {}", token.procedure()));
    }
    if token.line() > 0 {
        detail.push_str(&format!(", Line {}", token.line()));
    }
    let mut converted = DatabaseError::new(token.message())
        .with_code(token.code().to_string())
        .with_detail(detail);
    if token.procedure().is_empty()
        && let Some(position) = line_position(&sql[batch.min(sql.len())..], token.line())
    {
        converted = converted.with_position(batch + position);
    }
    converted
}

/// The byte offset of the first non-blank character of line `line`
/// (counting from 1) of `text`.
fn line_position(text: &str, line: u32) -> Option<usize> {
    let skip = (line as usize).checked_sub(1)?;
    let start = if skip == 0 {
        0
    } else {
        text.match_indices('\n').nth(skip - 1)?.0 + 1
    };
    let indent = text[start..]
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(0);
    let position = start + indent;
    // A blank line points at its own start rather than the next line.
    if text[position..].starts_with(['\r', '\n']) {
        Some(start)
    } else {
        Some(position)
    }
}

/// Whether `error` leaves the session unusable: the network failed, or the
/// conversation with the server lost its place.
pub(crate) fn is_fatal(error: &Error) -> bool {
    matches!(error, Error::Io { .. } | Error::Protocol(_) | Error::Tls(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_number_points_at_the_text_on_that_line() {
        let sql = "SELECT 1\n  FROM nope\nWHERE x";
        assert_eq!(line_position(sql, 1), Some(0));
        assert_eq!(&sql[line_position(sql, 2).unwrap()..][..4], "FROM");
        assert_eq!(&sql[line_position(sql, 3).unwrap()..][..5], "WHERE");
        assert_eq!(line_position(sql, 4), None);
        assert_eq!(line_position(sql, 0), None);
        assert_eq!(line_position("a\n\nb", 2), Some(2));
    }
}
