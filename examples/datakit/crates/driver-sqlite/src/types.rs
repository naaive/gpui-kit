use std::fmt::Write as _;

use datakit_driver::{TypeCategory, Value};
use rusqlite::types::ValueRef;

/// The category of a column declared as `declared`, from the type affinity
/// SQLite derives from it (the "Determination Of Column Affinity" rules),
/// refined by the names people use for dates, booleans, JSON and UUIDs,
/// which SQLite stores without a type of their own.
pub(crate) fn category(declared: &str) -> TypeCategory {
    let declared = declared.to_ascii_uppercase();
    let has = |part: &str| declared.contains(part);
    if has("BOOL") {
        TypeCategory::Boolean
    } else if has("INT") {
        TypeCategory::Integer
    } else if has("CHAR") || has("CLOB") || has("TEXT") {
        TypeCategory::Text
    } else if has("BLOB") || declared.trim().is_empty() {
        TypeCategory::Binary
    } else if has("REAL") || has("FLOA") || has("DOUB") {
        TypeCategory::Float
    } else if has("DATE") || has("TIME") {
        TypeCategory::Temporal
    } else if has("JSON") {
        TypeCategory::Json
    } else if has("UUID") || has("GUID") {
        TypeCategory::Uuid
    } else {
        TypeCategory::Decimal
    }
}

/// The category for a value of a column with no declared type: whatever
/// SQLite stored it as.
pub(crate) fn storage_category(value: ValueRef<'_>) -> TypeCategory {
    match value {
        ValueRef::Integer(_) => TypeCategory::Integer,
        ValueRef::Real(_) => TypeCategory::Float,
        ValueRef::Blob(_) => TypeCategory::Binary,
        ValueRef::Null | ValueRef::Text(_) => TypeCategory::Text,
    }
}

/// The name a column with no declared type is shown with: the storage class
/// of its first value.
pub(crate) fn storage_name(value: ValueRef<'_>) -> &'static str {
    match value {
        ValueRef::Null => "",
        ValueRef::Integer(_) => "integer",
        ValueRef::Real(_) => "real",
        ValueRef::Text(_) => "text",
        ValueRef::Blob(_) => "blob",
    }
}

/// `value`, read from a column in `category`.
///
/// A column declared boolean stores `0` and `1`; those become booleans.
/// A blob becomes its literal, `X'0A0B'`.
pub(crate) fn value(value: ValueRef<'_>, category: TypeCategory) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(0) if category == TypeCategory::Boolean => Value::Bool(false),
        ValueRef::Integer(1) if category == TypeCategory::Boolean => Value::Bool(true),
        ValueRef::Integer(value) => Value::Int(value),
        ValueRef::Real(value) => Value::Float(value),
        ValueRef::Text(text) => Value::Text(String::from_utf8_lossy(text).into()),
        ValueRef::Blob(bytes) => Value::Text(blob_literal(bytes).into()),
    }
}

/// `bytes` as SQLite's blob literal, `X'0A0B'`.
pub(crate) fn blob_literal(bytes: &[u8]) -> String {
    let mut literal = String::with_capacity(bytes.len() * 2 + 3);
    literal.push_str("X'");
    for byte in bytes {
        let _ = write!(literal, "{byte:02X}");
    }
    literal.push('\'');
    literal
}

/// Whether `text` is a blob literal as [`blob_literal`] writes it.
pub(crate) fn is_blob_literal(text: &str) -> bool {
    text.strip_prefix("X'")
        .and_then(|rest| rest.strip_suffix('\''))
        .is_some_and(|hex| {
            hex.len() % 2 == 0
                && hex
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_follow_sqlite_affinity() {
        assert_eq!(category("INTEGER"), TypeCategory::Integer);
        assert_eq!(category("unsigned big int"), TypeCategory::Integer);
        assert_eq!(category("VARCHAR(80)"), TypeCategory::Text);
        assert_eq!(category("BLOB"), TypeCategory::Binary);
        assert_eq!(category(""), TypeCategory::Binary);
        assert_eq!(category("DOUBLE PRECISION"), TypeCategory::Float);
        assert_eq!(category("NUMERIC(10,2)"), TypeCategory::Decimal);
        assert_eq!(category("DATETIME"), TypeCategory::Temporal);
        assert_eq!(category("BOOLEAN"), TypeCategory::Boolean);
    }

    #[test]
    fn blobs_read_as_literals_that_round_trip() {
        let literal = blob_literal(&[0x0a, 0x0b, 0xff]);
        assert_eq!(literal, "X'0A0BFF'");
        assert!(is_blob_literal(&literal));
        assert!(is_blob_literal("X''"));
        assert!(!is_blob_literal("X'0A0'"));
        assert!(!is_blob_literal("x'0a'"));
        assert!(!is_blob_literal("X'0G'"));
    }

    #[test]
    fn booleans_read_as_booleans_only_when_declared() {
        assert_eq!(
            value(ValueRef::Integer(1), TypeCategory::Boolean),
            Value::Bool(true)
        );
        assert_eq!(
            value(ValueRef::Integer(2), TypeCategory::Boolean),
            Value::Int(2)
        );
        assert_eq!(
            value(ValueRef::Integer(1), TypeCategory::Integer),
            Value::Int(1)
        );
    }
}
