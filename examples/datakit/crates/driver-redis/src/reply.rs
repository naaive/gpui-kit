//! Replies as rows.

use std::sync::Arc;

use datakit_driver::{
    ColumnInfo, CommandSummary, Row, RowStream, StatementOutcome, TypeCategory, Value,
};
use futures::StreamExt as _;
use redis::Value as Reply;

/// What the console shows for `reply` to `words`, the command line.
pub(crate) fn outcome(words: &[String], reply: Reply) -> anyhow::Result<StatementOutcome> {
    let command = words
        .first()
        .map(|word| word.to_uppercase())
        .unwrap_or_default();
    let has = |flag: &str| words.iter().any(|word| word.eq_ignore_ascii_case(flag));
    Ok(match reply {
        Reply::Okay => StatementOutcome::Command(CommandSummary::new("OK", None)),
        // What a writing command counts — fields set, keys deleted — is a
        // report on the command, not a result to look through.
        Reply::Int(count) if !crate::command::READING.contains(&command.as_str()) => {
            StatementOutcome::Command(CommandSummary::new(format!("{command} {count}"), None))
        }
        Reply::SimpleString(status) => StatementOutcome::Command(CommandSummary::new(status, None)),
        Reply::ServerError(error) => anyhow::bail!("{}", error.details().unwrap_or(error.code())),
        Reply::Map(pairs) => pairs_table(
            "key",
            "value",
            pairs
                .into_iter()
                .map(|(key, value)| (scalar(key), scalar(value))),
        ),
        Reply::Array(items) | Reply::Set(items) => match command.as_str() {
            "SCAN" | "SSCAN" | "HSCAN" | "ZSCAN" => {
                let mut items = items.into_iter();
                let cursor = items
                    .next()
                    .map(scalar)
                    .and_then(|value| value.display().map(|c| c.into_owned()));
                let found = match items.next() {
                    Some(Reply::Array(found)) => found,
                    _ => Vec::new(),
                };
                let heading = format!("next cursor {}", cursor.unwrap_or_default());
                match command.as_str() {
                    "HSCAN" => pairs_table("field", &heading, pairs(found)),
                    "ZSCAN" => pairs_table("member", &heading, pairs(found)),
                    _ => single_column(&heading, found.into_iter().map(scalar)),
                }
            }
            "HGETALL" | "CONFIG" => pairs_table("field", "value", pairs(items)),
            "ZRANGE" | "ZREVRANGE" | "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZPOPMIN"
            | "ZPOPMAX" | "ZRANDMEMBER"
                if has("WITHSCORES") || command.starts_with("ZPOP") =>
            {
                pairs_table("member", "score", pairs(items))
            }
            "XRANGE" | "XREVRANGE" => entries_table(items),
            "LRANGE" => {
                let start: i64 = words.get(2).and_then(|word| word.parse().ok()).unwrap_or(0);
                let start = start.max(0);
                indexed(start, items)
            }
            _ => single_column("value", items.into_iter().map(scalar)),
        },
        other => single_column("value", std::iter::once(scalar(other))),
    })
}

/// The rows of a table: each row a value per column.
pub(crate) fn table(columns: &[(&str, TypeCategory)], rows: Vec<Row>) -> StatementOutcome {
    let columns: Vec<ColumnInfo> = columns
        .iter()
        .map(|(name, category)| ColumnInfo::new(*name, type_name(*category), *category))
        .collect();
    StatementOutcome::Rows(RowStream::new(
        columns,
        futures::stream::iter(rows.into_iter().map(Ok)).boxed(),
    ))
}

fn type_name(category: TypeCategory) -> &'static str {
    match category {
        TypeCategory::Integer => "integer",
        TypeCategory::Float => "double",
        TypeCategory::Boolean => "boolean",
        _ => "string",
    }
}

/// The category of a column of `values`: numeric when every value is.
fn category<'a>(values: impl Iterator<Item = &'a Value>) -> TypeCategory {
    let mut category = None;
    for value in values {
        let this = match value {
            Value::Null => continue,
            Value::Int(_) => TypeCategory::Integer,
            Value::Float(_) => TypeCategory::Float,
            Value::Bool(_) => TypeCategory::Boolean,
            Value::Text(_) => return TypeCategory::Text,
        };
        match category {
            None => category = Some(this),
            Some(seen) if seen == this => {}
            Some(_) => return TypeCategory::Text,
        }
    }
    category.unwrap_or(TypeCategory::Text)
}

fn single_column(name: &str, values: impl Iterator<Item = Value>) -> StatementOutcome {
    let rows: Vec<Row> = values.map(|value| vec![value].into()).collect();
    let category = category(rows.iter().map(|row| &row[0]));
    table(&[(name, category)], rows)
}

fn indexed(start: i64, items: Vec<Reply>) -> StatementOutcome {
    let rows: Vec<Row> = items
        .into_iter()
        .enumerate()
        .map(|(ix, item)| vec![Value::Int(start + ix as i64), scalar(item)].into())
        .collect();
    let category = category(rows.iter().map(|row| &row[1]));
    table(
        &[("index", TypeCategory::Integer), ("value", category)],
        rows,
    )
}

fn pairs_table(
    first: &str,
    second: &str,
    pairs: impl Iterator<Item = (Value, Value)>,
) -> StatementOutcome {
    let rows: Vec<Row> = pairs.map(|(a, b)| vec![a, b].into()).collect();
    let categories = (
        category(rows.iter().map(|row| &row[0])),
        category(rows.iter().map(|row| &row[1])),
    );
    table(&[(first, categories.0), (second, categories.1)], rows)
}

/// A flat `[a, b, a, b, …]` reply as pairs.
fn pairs(items: Vec<Reply>) -> impl Iterator<Item = (Value, Value)> {
    let mut items = items.into_iter();
    std::iter::from_fn(move || Some((scalar(items.next()?), scalar(items.next()?))))
}

/// A stream's entries: their id and their fields, `field=value` a pair.
fn entries_table(items: Vec<Reply>) -> StatementOutcome {
    let rows: Vec<Row> = items
        .into_iter()
        .map(|entry| {
            let mut parts = match entry {
                Reply::Array(parts) => parts.into_iter(),
                other => vec![other].into_iter(),
            };
            let id = parts.next().map(scalar).unwrap_or(Value::Null);
            let fields = match parts.next() {
                Some(Reply::Array(fields)) => pairs(fields)
                    .map(|(field, value)| {
                        format!(
                            "{}={}",
                            field.display().unwrap_or_default(),
                            value.display().unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => String::new(),
            };
            vec![id, Value::Text(fields.into())].into()
        })
        .collect();
    table(
        &[("id", TypeCategory::Text), ("fields", TypeCategory::Text)],
        rows,
    )
}

/// One value of a reply. Nested replies become their text.
pub(crate) fn scalar(reply: Reply) -> Value {
    match reply {
        Reply::Nil => Value::Null,
        Reply::Int(value) => Value::Int(value),
        Reply::Double(value) => Value::Float(value),
        Reply::Boolean(value) => Value::Bool(value),
        Reply::BulkString(bytes) => Value::Text(bytes_text(&bytes).into()),
        Reply::SimpleString(text) => Value::Text(text.into()),
        Reply::Okay => Value::Text("OK".into()),
        Reply::VerbatimString { text, .. } => Value::Text(text.into()),
        Reply::BigNumber(number) => Value::Text(number.to_string().into()),
        Reply::Array(items) | Reply::Set(items) => Value::Text(
            format!(
                "[{}]",
                items
                    .into_iter()
                    .map(|item| scalar(item).display().unwrap_or("nil".into()).into_owned())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .into(),
        ),
        other => Value::Text(format!("{other:?}").into()),
    }
}

/// Bytes as text: UTF-8 when they are, otherwise `redis-cli`'s escapes.
pub(crate) fn bytes_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes
            .iter()
            .map(|byte| match byte {
                0x20..=0x7e if *byte != b'\\' => (*byte as char).to_string(),
                _ => format!("\\x{byte:02x}"),
            })
            .collect(),
    }
}

/// A column, for the explorer's description of a key's value.
pub(crate) fn column(name: &str, type_name: &str) -> datakit_catalog::Column {
    datakit_catalog::Column::new(Arc::<str>::from(name), Arc::<str>::from(type_name))
}
