//! ClickHouse's types, which the server names in text: `Nullable(UInt64)`,
//! `LowCardinality(String)`, `DateTime64(3, 'UTC')`.
//!
//! Nullability is part of a ClickHouse type rather than a property of the
//! column, so it is read from and written into the type name.

use datakit_driver::TypeCategory;

use crate::text;

/// The category of a ClickHouse type, looking through the wrappers that
/// change how values are stored but not what they are.
pub(crate) fn category(type_name: &str) -> TypeCategory {
    let ty = unwrap(type_name);
    let base = ty.split('(').next().unwrap_or(ty).trim();
    match base {
        "Bool" | "Boolean" => TypeCategory::Boolean,
        _ if base.starts_with("Interval") => TypeCategory::Temporal,
        _ if base.starts_with("Int") || base.starts_with("UInt") => TypeCategory::Integer,
        "Float32" | "Float64" | "BFloat16" => TypeCategory::Float,
        _ if base.starts_with("Decimal") => TypeCategory::Decimal,
        "Date" | "Date32" | "DateTime" | "DateTime32" | "DateTime64" | "Time" | "Time64" => {
            TypeCategory::Temporal
        }
        "UUID" => TypeCategory::Uuid,
        "Array" => TypeCategory::Array,
        "Map" | "Tuple" | "Nested" | "JSON" | "Object" | "Variant" | "Dynamic" => {
            TypeCategory::Json
        }
        "IPv4" | "IPv6" => TypeCategory::Network,
        "String" | "FixedString" | "Enum" | "Enum8" | "Enum16" => TypeCategory::Text,
        _ => TypeCategory::Other,
    }
}

/// `type_name` without `Nullable(…)`, `LowCardinality(…)` and the type
/// argument of `SimpleAggregateFunction(…)`, however they nest.
fn unwrap(type_name: &str) -> &str {
    let mut ty = type_name.trim();
    loop {
        if let Some(inner) = wrapped(ty, "Nullable").or_else(|| wrapped(ty, "LowCardinality")) {
            ty = inner.trim();
        } else if let Some(arguments) = wrapped(ty, "SimpleAggregateFunction")
            && let [_, inner] = text::split_top_level(arguments, ',')[..]
        {
            ty = inner;
        } else {
            return ty;
        }
    }
}

/// The arguments of `ty` when it is `name(…)`.
fn wrapped<'a>(ty: &'a str, name: &str) -> Option<&'a str> {
    ty.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')
}

/// Whether a column of `type_name` may hold `NULL`.
pub(crate) fn is_nullable(type_name: &str) -> bool {
    let ty = type_name.trim();
    let ty = wrapped(ty, "LowCardinality").unwrap_or(ty);
    ty.starts_with("Nullable(")
}

/// `type_name`, wrapped in or unwrapped from `Nullable` so that it holds
/// `NULL` exactly when `nullable` is set. A `LowCardinality` type keeps it
/// outermost, where ClickHouse requires it.
pub(crate) fn with_nullability(type_name: &str, nullable: bool) -> String {
    let ty = type_name.trim();
    if is_nullable(ty) == nullable {
        return ty.to_string();
    }
    match (wrapped(ty, "LowCardinality"), nullable) {
        (Some(inner), true) => format!("LowCardinality(Nullable({}))", inner.trim()),
        (Some(inner), false) => format!(
            "LowCardinality({})",
            wrapped(inner.trim(), "Nullable").unwrap_or(inner)
        ),
        (None, true) => format!("Nullable({ty})"),
        (None, false) => wrapped(ty, "Nullable").unwrap_or(ty).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrappers_do_not_change_the_category() {
        assert_eq!(category("Nullable(UInt64)"), TypeCategory::Integer);
        assert_eq!(
            category("LowCardinality(Nullable(String))"),
            TypeCategory::Text
        );
        assert_eq!(
            category("SimpleAggregateFunction(sum, Float64)"),
            TypeCategory::Float
        );
    }

    #[test]
    fn every_family_has_its_category() {
        for (ty, expected) in [
            ("Int8", TypeCategory::Integer),
            ("UInt256", TypeCategory::Integer),
            ("Float32", TypeCategory::Float),
            ("Decimal(18, 2)", TypeCategory::Decimal),
            ("Decimal128(4)", TypeCategory::Decimal),
            ("Bool", TypeCategory::Boolean),
            ("Date32", TypeCategory::Temporal),
            ("DateTime64(3, 'UTC')", TypeCategory::Temporal),
            ("IntervalDay", TypeCategory::Temporal),
            ("UUID", TypeCategory::Uuid),
            ("Array(Nullable(String))", TypeCategory::Array),
            ("Map(String, UInt64)", TypeCategory::Json),
            ("Tuple(a UInt8, b String)", TypeCategory::Json),
            ("JSON", TypeCategory::Json),
            ("IPv6", TypeCategory::Network),
            ("FixedString(16)", TypeCategory::Text),
            ("Enum8('a' = 1, 'b' = 2)", TypeCategory::Text),
            ("AggregateFunction(uniq, UInt64)", TypeCategory::Other),
        ] {
            assert_eq!(category(ty), expected, "{ty}");
        }
    }

    #[test]
    fn nullability_is_written_into_the_type() {
        assert!(is_nullable("Nullable(String)"));
        assert!(is_nullable("LowCardinality(Nullable(String))"));
        assert!(!is_nullable("Array(Nullable(String))"));
        assert_eq!(with_nullability("String", true), "Nullable(String)");
        assert_eq!(with_nullability("Nullable(String)", false), "String");
        assert_eq!(
            with_nullability("LowCardinality(String)", true),
            "LowCardinality(Nullable(String))"
        );
        assert_eq!(
            with_nullability("LowCardinality(Nullable(String))", false),
            "LowCardinality(String)"
        );
        assert_eq!(with_nullability("UInt8", false), "UInt8");
    }
}
