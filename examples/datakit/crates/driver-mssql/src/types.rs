//! SQL Server's types as DataKit sees them: a category, the name SQL Server
//! spells, and the [`Value`] a cell holds.

use std::fmt::Write as _;

use datakit_driver::{Row, TypeCategory, Value};
use tiberius::{
    ColumnData, ColumnType,
    numeric::Numeric,
    time::{Date, DateTime, DateTime2, DateTimeOffset, SmallDateTime, Time},
};

/// The category of a result column's type.
pub(crate) fn category(ty: ColumnType) -> TypeCategory {
    match ty {
        ColumnType::Bit | ColumnType::Bitn => TypeCategory::Boolean,
        ColumnType::Int1
        | ColumnType::Int2
        | ColumnType::Int4
        | ColumnType::Int8
        | ColumnType::Intn => TypeCategory::Integer,
        ColumnType::Float4 | ColumnType::Float8 | ColumnType::Floatn => TypeCategory::Float,
        ColumnType::Money | ColumnType::Money4 | ColumnType::Decimaln | ColumnType::Numericn => {
            TypeCategory::Decimal
        }
        ColumnType::Datetime4
        | ColumnType::Datetime
        | ColumnType::Datetimen
        | ColumnType::Daten
        | ColumnType::Timen
        | ColumnType::Datetime2
        | ColumnType::DatetimeOffsetn => TypeCategory::Temporal,
        ColumnType::Guid => TypeCategory::Uuid,
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image | ColumnType::Udt => {
            TypeCategory::Binary
        }
        ColumnType::BigVarChar
        | ColumnType::BigChar
        | ColumnType::NVarchar
        | ColumnType::NChar
        | ColumnType::Text
        | ColumnType::NText => TypeCategory::Text,
        ColumnType::Xml | ColumnType::SSVariant | ColumnType::Null => TypeCategory::Other,
    }
}

/// The type as SQL Server spells it, without length or precision: the
/// protocol describes a result column by its wire type, which does not say
/// them.
pub(crate) fn display_name(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::Null => "",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money => "money",
        ColumnType::Money4 => "smallmoney",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::Decimaln => "decimal",
        ColumnType::Numericn => "numeric",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::BigVarBin => "varbinary",
        ColumnType::BigVarChar => "varchar",
        ColumnType::BigBinary => "binary",
        ColumnType::BigChar => "char",
        ColumnType::NVarchar => "nvarchar",
        ColumnType::NChar => "nchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::Text => "text",
        ColumnType::Image => "image",
        ColumnType::NText => "ntext",
        ColumnType::SSVariant => "sql_variant",
    }
}

/// The values of `row`, one per column.
pub(crate) fn row_values(row: &tiberius::Row) -> Row {
    row.cells()
        .map(|(column, data)| value(data, column.column_type()))
        .collect()
}

/// One cell as a [`Value`].
///
/// `money` is the one type whose meaning the protocol loses: `tiberius`
/// decodes it as a float, so the column type says to print it back with the
/// four decimals it is stored with.
pub(crate) fn value(data: &ColumnData<'_>, ty: ColumnType) -> Value {
    match data {
        ColumnData::U8(value) => value.map_or(Value::Null, |value| Value::Int(value.into())),
        ColumnData::I16(value) => value.map_or(Value::Null, |value| Value::Int(value.into())),
        ColumnData::I32(value) => value.map_or(Value::Null, |value| Value::Int(value.into())),
        ColumnData::I64(value) => value.map_or(Value::Null, Value::Int),
        // A `real` keeps the digits it was written with, not those of its
        // nearest `f64`.
        ColumnData::F32(value) => value.map_or(Value::Null, |value| {
            Value::Float(value.to_string().parse().unwrap_or(value.into()))
        }),
        ColumnData::F64(value) => value.map_or(Value::Null, |value| match ty {
            ColumnType::Money | ColumnType::Money4 => Value::Text(format!("{value:.4}").into()),
            _ => Value::Float(value),
        }),
        ColumnData::Bit(value) => value.map_or(Value::Null, Value::Bool),
        ColumnData::String(text) => text
            .as_deref()
            .map_or(Value::Null, |text| Value::Text(text.into())),
        ColumnData::Guid(guid) => guid.map_or(Value::Null, |guid| {
            Value::Text(guid.to_string().to_uppercase().into())
        }),
        ColumnData::Binary(bytes) => bytes
            .as_deref()
            .map_or(Value::Null, |bytes| Value::Text(hex(bytes).into())),
        ColumnData::Numeric(number) => {
            number.map_or(Value::Null, |number| Value::Text(decimal(number).into()))
        }
        ColumnData::Xml(xml) => xml
            .as_deref()
            .map_or(Value::Null, |xml| Value::Text(xml.to_string().into())),
        ColumnData::DateTime(value) => {
            value.map_or(Value::Null, |value| Value::Text(datetime(value).into()))
        }
        ColumnData::SmallDateTime(value) => value.map_or(Value::Null, |value| {
            Value::Text(smalldatetime(value).into())
        }),
        ColumnData::Time(value) => {
            value.map_or(Value::Null, |value| Value::Text(time(value).into()))
        }
        ColumnData::Date(value) => {
            value.map_or(Value::Null, |value| Value::Text(date(value).into()))
        }
        ColumnData::DateTime2(value) => {
            value.map_or(Value::Null, |value| Value::Text(datetime2(value).into()))
        }
        ColumnData::DateTimeOffset(value) => value.map_or(Value::Null, |value| {
            Value::Text(datetimeoffset(value).into())
        }),
    }
}

/// Binary data as SQL Server writes a binary literal: `0x` and upper-case
/// hex digits.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(2 + bytes.len() * 2);
    text.push_str("0x");
    for byte in bytes {
        let _ = write!(text, "{byte:02X}");
    }
    text
}

/// An exact number with exactly its scale's digits after the point, and no
/// point at all for a scale of zero.
pub(crate) fn decimal(number: Numeric) -> String {
    let digits = number.value().unsigned_abs().to_string();
    let scale = number.scale() as usize;
    let sign = if number.value() < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let digits = format!("{digits:0>width$}", width = scale + 1);
    let (whole, fraction) = digits.split_at(digits.len() - scale);
    format!("{sign}{whole}.{fraction}")
}

/// Days from 0001-01-01 to 1900-01-01, the epoch of `datetime`.
const DAYS_TO_1900: i64 = 693_595;

/// `yyyy-mm-dd` of the day `days` after 0001-01-01.
fn civil(days: i64) -> String {
    // Howard Hinnant's `civil_from_days`, shifted from 1970-01-01 to
    // 0001-01-01.
    let z = days - 719_162 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// `hh:mm:ss` and, for a positive `scale`, that many fractional digits of
/// `increments` (units of 10^-scale seconds since midnight).
fn clock(increments: u64, scale: u8) -> String {
    let per_second = 10u64.pow(scale.into());
    let seconds = increments / per_second;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if scale > 0 {
        let _ = write!(
            text,
            ".{:0width$}",
            increments % per_second,
            width = scale.into()
        );
    }
    text
}

fn date(value: Date) -> String {
    civil(value.days().into())
}

fn time(value: Time) -> String {
    clock(value.increments(), value.scale())
}

fn datetime2(value: DateTime2) -> String {
    format!("{} {}", date(value.date()), time(value.time()))
}

/// `datetime` counts days from 1900 and three-hundredths of a second, which
/// SQL Server shows as milliseconds.
fn datetime(value: DateTime) -> String {
    let milliseconds = (u64::from(value.seconds_fragments()) * 10 + 1) / 3;
    format!(
        "{} {}",
        civil(DAYS_TO_1900 + i64::from(value.days())),
        clock(milliseconds, 3)
    )
}

/// `smalldatetime` counts days from 1900 and minutes.
fn smalldatetime(value: SmallDateTime) -> String {
    let seconds = u64::from(value.seconds_fragments()) * 60;
    format!(
        "{} {}",
        civil(DAYS_TO_1900 + i64::from(value.days())),
        clock(seconds, 0)
    )
}

/// The local time with its offset, as SQL Server shows it:
/// `2024-05-01 10:30:00.0000000 +02:00`. The protocol carries the time in
/// UTC and the offset in minutes.
fn datetimeoffset(value: DateTimeOffset) -> String {
    let utc = value.datetime2();
    let scale = utc.time().scale();
    let per_minute = 60 * 10i128.pow(scale.into());
    let per_day = 24 * 60 * per_minute;
    let total = i128::from(utc.date().days()) * per_day
        + i128::from(utc.time().increments())
        + i128::from(value.offset()) * per_minute;
    let days = total.div_euclid(per_day);
    let increments = total.rem_euclid(per_day);
    let offset = value.offset();
    format!(
        "{} {} {}{:02}:{:02}",
        civil(days as i64),
        clock(increments as u64, scale),
        if offset < 0 { '-' } else { '+' },
        offset.unsigned_abs() / 60,
        offset.unsigned_abs() % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_numbers_keep_their_scale() {
        assert_eq!(decimal(Numeric::new_with_scale(990, 2)), "9.90");
        assert_eq!(decimal(Numeric::new_with_scale(-5, 3)), "-0.005");
        assert_eq!(decimal(Numeric::new_with_scale(42, 0)), "42");
        assert_eq!(
            value(&ColumnData::F64(Some(12.5)), ColumnType::Money),
            Value::Text("12.5000".into())
        );
    }

    #[test]
    fn dates_and_times_print_as_iso_text() {
        // 2024-02-29 is day 738_944 after 0001-01-01.
        assert_eq!(civil(738_944), "2024-02-29");
        assert_eq!(civil(0), "0001-01-01");
        assert_eq!(
            datetime2(DateTime2::new(
                Date::new(738_944),
                Time::new(((13 * 60 + 5) * 60 + 9) * 10_000_000 + 1_234_567, 7)
            )),
            "2024-02-29 13:05:09.1234567"
        );
        // 1900-01-02 00:00:01.003
        assert_eq!(datetime(DateTime::new(1, 301)), "1900-01-02 00:00:01.003");
        assert_eq!(
            smalldatetime(SmallDateTime::new(0, 61)),
            "1900-01-01 01:01:00"
        );
    }

    #[test]
    fn an_offset_moves_the_utc_time_to_local() {
        // 2024-02-29 23:30 UTC at +02:00 is 01:30 on March 1st.
        let utc = DateTime2::new(Date::new(738_944), Time::new((23 * 60 + 30) * 60, 0));
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(utc, 120)),
            "2024-03-01 01:30:00 +02:00"
        );
        assert_eq!(
            datetimeoffset(DateTimeOffset::new(utc, -330)),
            "2024-02-29 18:00:00 -05:30"
        );
    }

    #[test]
    fn binary_is_a_hex_literal() {
        assert_eq!(hex(&[0xde, 0xad, 0x01]), "0xDEAD01");
        assert_eq!(
            value(&ColumnData::Binary(None), ColumnType::BigVarBin),
            Value::Null
        );
    }

    #[test]
    fn categories_follow_the_wire_type() {
        assert_eq!(category(ColumnType::Money), TypeCategory::Decimal);
        assert_eq!(category(ColumnType::Intn), TypeCategory::Integer);
        assert_eq!(category(ColumnType::Guid), TypeCategory::Uuid);
        assert_eq!(display_name(ColumnType::Datetime2), "datetime2");
        assert_eq!(display_name(ColumnType::NVarchar), "nvarchar");
    }
}
