//! ClickHouse's `TabSeparated` text, the format every result is read in.
//!
//! One row is one line and its values are separated by tabs; a tab, a line
//! break or a backslash inside a value is written as a backslash escape, so
//! splitting on the raw bytes is always right. `\N` alone is `NULL`.
//! `TabSeparatedWithNamesAndTypes` starts with two more lines of the same
//! shape: the column names, then their ClickHouse types.

use std::{collections::VecDeque, sync::Arc};

use anyhow::Result;
use bytes::Bytes;
use futures::{StreamExt as _, stream::BoxStream};

use crate::error;

/// The format statements are run in, unless they name another.
pub(crate) const FORMAT: &str = "TabSeparatedWithNamesAndTypes";

/// The values of one line, unescaped; `None` for `NULL`.
pub(crate) fn fields(line: &[u8]) -> Vec<Option<String>> {
    line.split(|byte| *byte == b'\t').map(field).collect()
}

/// One value, unescaped; `None` for `NULL`.
pub(crate) fn field(raw: &[u8]) -> Option<String> {
    (raw != b"\\N").then(|| unescape(raw))
}

/// `raw` with ClickHouse's backslash escapes resolved. Bytes that are not
/// UTF-8 — a `String` may hold any — are replaced rather than lost.
pub(crate) fn unescape(raw: &[u8]) -> String {
    if !raw.contains(&b'\\') {
        return String::from_utf8_lossy(raw).into_owned();
    }
    let mut bytes = Vec::with_capacity(raw.len());
    let mut ix = 0;
    while ix < raw.len() {
        let byte = raw[ix];
        let Some(&next) = raw.get(ix + 1).filter(|_| byte == b'\\') else {
            bytes.push(byte);
            ix += 1;
            continue;
        };
        ix += 2;
        bytes.push(match next {
            b'b' => 0x08,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'0' => 0,
            b'a' => 0x07,
            b'v' => 0x0b,
            b'e' => 0x1b,
            b'x' => match raw
                .get(ix..ix + 2)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(value) => {
                    ix += 2;
                    value
                }
                None => b'x',
            },
            // `\\`, `\'` and any other escaped character stand for
            // themselves.
            other => other,
        });
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A response body read line by line as it arrives.
///
/// Once the server has started sending rows it can no longer answer with an
/// error status, so an error that happens later — a cancelled statement, a
/// limit reached while reading — is written into the body after the rows
/// already sent. The body reports it as an error instead of as a line.
pub(crate) struct Body {
    chunks: BoxStream<'static, Result<Bytes>>,
    lines: VecDeque<Vec<u8>>,
    /// The start of a line whose end has not arrived yet.
    partial: Vec<u8>,
    ended: bool,
    /// The statement, to place an error's position in.
    sql: Arc<str>,
}

impl Body {
    pub(crate) fn new(chunks: BoxStream<'static, Result<Bytes>>, sql: Arc<str>) -> Self {
        Self {
            chunks,
            lines: VecDeque::new(),
            partial: Vec::new(),
            ended: false,
            sql,
        }
    }

    /// The statement whose result this is.
    pub(crate) fn sql(&self) -> &str {
        &self.sql
    }

    /// The next line without its line break; `None` at the end of the body.
    pub(crate) async fn next_line(&mut self) -> Result<Option<Vec<u8>>> {
        let line = match self.next_raw_line().await? {
            Some(line) => line,
            None => return Ok(None),
        };
        if !error::starts_exception(&line) {
            return Ok(Some(line));
        }
        let mut text = String::from_utf8_lossy(&line).into_owned();
        while let Some(line) = self.next_raw_line().await? {
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&line));
        }
        Err(error::exception(&text, None, &self.sql).into())
    }

    async fn next_raw_line(&mut self) -> Result<Option<Vec<u8>>> {
        loop {
            if let Some(line) = self.lines.pop_front() {
                return Ok(Some(line));
            }
            if self.ended {
                // A body that does not end with a line break still ends a
                // line.
                return Ok((!self.partial.is_empty()).then(|| std::mem::take(&mut self.partial)));
            }
            match self.chunks.next().await {
                Some(chunk) => self.push(&chunk?),
                None => self.ended = true,
            }
        }
    }

    fn push(&mut self, mut chunk: &[u8]) {
        while let Some(end) = chunk.iter().position(|byte| *byte == b'\n') {
            let mut line = std::mem::take(&mut self.partial);
            line.extend_from_slice(&chunk[..end]);
            self.lines.push_back(line);
            chunk = &chunk[end + 1..];
        }
        self.partial.extend_from_slice(chunk);
    }
}

#[cfg(test)]
mod tests {
    use datakit_driver::DatabaseError;
    use futures::{FutureExt as _, stream};

    use super::*;

    fn body(chunks: &[&str]) -> Body {
        let chunks: Vec<Result<Bytes>> = chunks
            .iter()
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk.as_bytes())))
            .collect();
        Body::new(stream::iter(chunks).boxed(), "SELECT 1".into())
    }

    fn lines(body: &mut Body) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        while let Some(line) = body.next_line().now_or_never().unwrap()? {
            lines.push(String::from_utf8(line).unwrap());
        }
        Ok(lines)
    }

    #[test]
    fn escapes_are_resolved_and_null_is_none() {
        assert_eq!(
            fields(b"a\\tb\t\\N\tline\\none\\\\two\tit\\'s\t\\x41\t"),
            [
                Some("a\tb".into()),
                None,
                Some("line\none\\two".into()),
                Some("it's".into()),
                Some("A".into()),
                Some(String::new()),
            ]
        );
        // `\N` is NULL only as the whole value.
        assert_eq!(field(b"\\\\N"), Some("\\N".into()));
    }

    #[test]
    fn lines_may_be_split_across_chunks() {
        let mut body = body(&["na", "me\nStr", "ing\n\nlast"]);
        assert_eq!(lines(&mut body).unwrap(), ["name", "String", "", "last"]);
    }

    #[test]
    fn an_exception_after_rows_is_an_error() {
        let mut body = body(&[
            "n\nUInt64\n1\n2\n",
            "Code: 394. DB::Exception: Query was cancelled. (QUERY_WAS_CANCELLED) ",
            "(version 24.3.1.1 (official build))\n",
        ]);
        let mut seen = Vec::new();
        let error = loop {
            match body.next_line().now_or_never().unwrap() {
                Ok(Some(line)) => seen.push(String::from_utf8(line).unwrap()),
                Ok(None) => panic!("the body must fail"),
                Err(error) => break error,
            }
        };
        assert_eq!(seen, ["n", "UInt64", "1", "2"]);
        let error = error.downcast_ref::<DatabaseError>().unwrap();
        assert_eq!(error.code(), Some("394"));
        assert_eq!(error.message(), "Query was cancelled.");
    }
}
