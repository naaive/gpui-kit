//! What a result column holds, from the type and flags the server sends
//! with it, and its values as DataKit's [`Value`]s.

use std::fmt::Write as _;

use datakit_driver::{ColumnInfo, TypeCategory, Value};
use mysql_async::{
    Column,
    consts::{ColumnFlags, ColumnType},
};

/// The character set number MySQL gives bytes that are not text: `BINARY`,
/// `VARBINARY` and `BLOB` columns, and also numbers and times, whose metadata
/// says "binary" because they have no collation.
const BINARY_CHARSET: u16 = 63;

/// The column as DataKit describes a result column.
pub(crate) fn column_info(column: &Column) -> ColumnInfo {
    ColumnInfo::new(
        column.name_str().into_owned(),
        display_name(column),
        category(column),
    )
}

/// The category of a result column.
pub(crate) fn category(column: &Column) -> TypeCategory {
    use ColumnType::*;

    let flags = column.flags();
    match column.column_type() {
        MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_INT24 | MYSQL_TYPE_LONG
        | MYSQL_TYPE_LONGLONG => TypeCategory::Integer,
        MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE => TypeCategory::Float,
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => TypeCategory::Decimal,
        MYSQL_TYPE_DATE
        | MYSQL_TYPE_NEWDATE
        | MYSQL_TYPE_TIME
        | MYSQL_TYPE_TIME2
        | MYSQL_TYPE_DATETIME
        | MYSQL_TYPE_DATETIME2
        | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2
        | MYSQL_TYPE_YEAR => TypeCategory::Temporal,
        MYSQL_TYPE_JSON => TypeCategory::Json,
        // A bit field and a geometry arrive as raw bytes.
        MYSQL_TYPE_BIT | MYSQL_TYPE_GEOMETRY | MYSQL_TYPE_VECTOR => TypeCategory::Binary,
        MYSQL_TYPE_STRING | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR
            if flags.intersects(ColumnFlags::ENUM_FLAG | ColumnFlags::SET_FLAG) =>
        {
            TypeCategory::Text
        }
        MYSQL_TYPE_STRING
        | MYSQL_TYPE_VAR_STRING
        | MYSQL_TYPE_VARCHAR
        | MYSQL_TYPE_TINY_BLOB
        | MYSQL_TYPE_MEDIUM_BLOB
        | MYSQL_TYPE_LONG_BLOB
        | MYSQL_TYPE_BLOB => {
            if column.character_set() == BINARY_CHARSET {
                TypeCategory::Binary
            } else {
                TypeCategory::Text
            }
        }
        MYSQL_TYPE_ENUM | MYSQL_TYPE_SET => TypeCategory::Text,
        _ => TypeCategory::Other,
    }
}

/// The type as MySQL spells it in `CREATE TABLE`: `int unsigned`,
/// `varchar`, `datetime`. Lengths are left out; the server reports them in
/// bytes of its own encoding rather than as the column was declared.
pub(crate) fn display_name(column: &Column) -> String {
    use ColumnType::*;

    let flags = column.flags();
    let binary = column.character_set() == BINARY_CHARSET;
    let name = match column.column_type() {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_INT24 => "mediumint",
        MYSQL_TYPE_LONG => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 => "datetime",
        MYSQL_TYPE_TIMESTAMP | MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        MYSQL_TYPE_YEAR => "year",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_BIT => "bit",
        MYSQL_TYPE_GEOMETRY => "geometry",
        MYSQL_TYPE_VECTOR => "vector",
        MYSQL_TYPE_NULL => "null",
        MYSQL_TYPE_ENUM => "enum",
        MYSQL_TYPE_SET => "set",
        _ if flags.contains(ColumnFlags::ENUM_FLAG) => "enum",
        _ if flags.contains(ColumnFlags::SET_FLAG) => "set",
        MYSQL_TYPE_STRING if binary => "binary",
        MYSQL_TYPE_STRING => "char",
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR if binary => "varbinary",
        MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_VARCHAR => "varchar",
        MYSQL_TYPE_TINY_BLOB if binary => "tinyblob",
        MYSQL_TYPE_TINY_BLOB => "tinytext",
        MYSQL_TYPE_MEDIUM_BLOB if binary => "mediumblob",
        MYSQL_TYPE_MEDIUM_BLOB => "mediumtext",
        MYSQL_TYPE_LONG_BLOB if binary => "longblob",
        MYSQL_TYPE_LONG_BLOB => "longtext",
        MYSQL_TYPE_BLOB if binary => "blob",
        MYSQL_TYPE_BLOB => "text",
        _ => "",
    };
    let numeric = matches!(
        category(column),
        TypeCategory::Integer | TypeCategory::Float | TypeCategory::Decimal
    );
    if numeric && flags.contains(ColumnFlags::UNSIGNED_FLAG) {
        format!("{name} unsigned")
    } else {
        name.to_string()
    }
}

/// One value of a text-protocol row, read as a column of `category`.
///
/// The text protocol sends every value as the server's text, so only
/// `NULL` and bytes arrive. Bytes that are not text — a `BLOB`, a
/// `VARBINARY`, a `BIT` — are shown as hexadecimal, `0x` followed by two
/// upper-case digits per byte, which is also how MySQL writes a binary
/// literal; decoding them as text would show replacement characters, and
/// would lose bytes when the cell was copied or edited.
pub(crate) fn value(value: Option<&mysql_async::Value>, category: TypeCategory) -> Value {
    match value {
        None | Some(mysql_async::Value::NULL) => Value::Null,
        Some(mysql_async::Value::Bytes(bytes)) if category == TypeCategory::Binary => {
            Value::Text(hex(bytes).into())
        }
        Some(mysql_async::Value::Bytes(bytes)) => {
            Value::from_text(&String::from_utf8_lossy(bytes), category)
        }
        Some(other) => text(other).map_or(Value::Null, |text| Value::from_text(&text, category)),
    }
}

/// A value the binary protocol sent, as text; `None` for `NULL`.
pub(crate) fn text(value: &mysql_async::Value) -> Option<String> {
    match value {
        mysql_async::Value::NULL => None,
        mysql_async::Value::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        mysql_async::Value::Int(value) => Some(value.to_string()),
        mysql_async::Value::UInt(value) => Some(value.to_string()),
        mysql_async::Value::Float(value) => Some(value.to_string()),
        mysql_async::Value::Double(value) => Some(value.to_string()),
        // Dates and times print as quoted literals.
        other => Some(other.as_sql(true).trim_matches('\'').to_string()),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        let _ = write!(text, "{byte:02X}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(column_type: ColumnType, flags: ColumnFlags, character_set: u16) -> Column {
        Column::new(column_type)
            .with_flags(flags)
            .with_character_set(character_set)
    }

    #[test]
    fn bytes_in_the_binary_character_set_are_binary() {
        let varbinary = column(
            ColumnType::MYSQL_TYPE_VAR_STRING,
            ColumnFlags::BINARY_FLAG,
            BINARY_CHARSET,
        );
        assert_eq!(category(&varbinary), TypeCategory::Binary);
        assert_eq!(display_name(&varbinary), "varbinary");
        let text = column(ColumnType::MYSQL_TYPE_BLOB, ColumnFlags::empty(), 255);
        assert_eq!(category(&text), TypeCategory::Text);
        assert_eq!(display_name(&text), "text");
    }

    #[test]
    fn numbers_and_times_are_not_binary_despite_their_character_set() {
        let unsigned = column(
            ColumnType::MYSQL_TYPE_LONGLONG,
            ColumnFlags::UNSIGNED_FLAG,
            BINARY_CHARSET,
        );
        assert_eq!(category(&unsigned), TypeCategory::Integer);
        assert_eq!(display_name(&unsigned), "bigint unsigned");
        let datetime = column(
            ColumnType::MYSQL_TYPE_DATETIME,
            ColumnFlags::empty(),
            BINARY_CHARSET,
        );
        assert_eq!(category(&datetime), TypeCategory::Temporal);
        assert_eq!(display_name(&datetime), "datetime");
        let decimal = column(
            ColumnType::MYSQL_TYPE_NEWDECIMAL,
            ColumnFlags::empty(),
            BINARY_CHARSET,
        );
        assert_eq!(category(&decimal), TypeCategory::Decimal);
    }

    #[test]
    fn enums_are_text() {
        let status = column(ColumnType::MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG, 255);
        assert_eq!(category(&status), TypeCategory::Text);
        assert_eq!(display_name(&status), "enum");
    }

    #[test]
    fn binary_values_are_hexadecimal() {
        let bytes = mysql_async::Value::Bytes(vec![0x00, 0xAB, 0x10]);
        assert_eq!(
            value(Some(&bytes), TypeCategory::Binary),
            Value::Text("0x00AB10".into())
        );
        let number = mysql_async::Value::Bytes(b"42".to_vec());
        assert_eq!(value(Some(&number), TypeCategory::Integer), Value::Int(42));
        assert_eq!(
            value(Some(&mysql_async::Value::NULL), TypeCategory::Text),
            Value::Null
        );
    }
}
