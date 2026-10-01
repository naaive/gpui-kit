use std::{borrow::Cow, cmp::Ordering, sync::Arc};

/// One value in a result row.
///
/// Values keep the database's own text for every type DataKit does not
/// compute with: a `numeric` stays exact, a `timestamptz` keeps the server's
/// time zone, and a type DataKit has never heard of still displays. Only
/// booleans and numbers that fit a machine type are parsed, because sorting
/// and alignment need them.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(Arc<str>),
}

impl Value {
    /// The value for the server's text `text` of a column in `category`.
    pub fn from_text(text: &str, category: TypeCategory) -> Self {
        match category {
            TypeCategory::Boolean => match text {
                "t" | "true" => Value::Bool(true),
                "f" | "false" => Value::Bool(false),
                _ => Value::Text(text.into()),
            },
            TypeCategory::Integer => text
                .parse()
                .map(Value::Int)
                .unwrap_or_else(|_| Value::Text(text.into())),
            TypeCategory::Float => text
                .parse()
                .map(Value::Float)
                .unwrap_or_else(|_| Value::Text(text.into())),
            _ => Value::Text(text.into()),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The value as the grid and clipboard show it. `NULL` is not text: the
    /// caller decides how to show its absence, so this returns `None`.
    pub fn display(&self) -> Option<Cow<'_, str>> {
        match self {
            Value::Null => None,
            Value::Bool(true) => Some(Cow::Borrowed("true")),
            Value::Bool(false) => Some(Cow::Borrowed("false")),
            Value::Int(value) => Some(Cow::Owned(value.to_string())),
            Value::Float(value) => Some(Cow::Owned(value.to_string())),
            Value::Text(text) => Some(Cow::Borrowed(text)),
        }
    }

    /// A total order for sorting a result locally: `NULL` last, numbers by
    /// value, everything else by text.
    pub fn sort_cmp(&self, other: &Value) -> Ordering {
        match (self, other) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Greater,
            (_, Value::Null) => Ordering::Less,
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => a.total_cmp(b),
            (Value::Int(a), Value::Float(b)) => (*a as f64).total_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.total_cmp(&(*b as f64)),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (a, b) => a.display().cmp(&b.display()),
        }
    }
}

/// The family a column's type belongs to, which decides how its values are
/// parsed, aligned and edited. The name follows PostgreSQL's own
/// `pg_type.typcategory`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TypeCategory {
    Boolean,
    Integer,
    Float,
    /// Exact numbers (`numeric`, `money`), kept as text.
    Decimal,
    #[default]
    Text,
    Binary,
    /// Dates, times, timestamps and intervals.
    Temporal,
    Json,
    Uuid,
    Array,
    Network,
    Other,
}

impl TypeCategory {
    /// Whether values compare as numbers and align to the trailing edge.
    pub fn is_numeric(self) -> bool {
        matches!(self, Self::Integer | Self::Float | Self::Decimal)
    }
}

/// A column of a result, as the server described it.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnInfo {
    name: Arc<str>,
    type_name: Arc<str>,
    category: TypeCategory,
}

impl ColumnInfo {
    pub fn new(
        name: impl Into<Arc<str>>,
        type_name: impl Into<Arc<str>>,
        category: TypeCategory,
    ) -> Self {
        Self {
            name: name.into(),
            type_name: type_name.into(),
            category,
        }
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn type_name(&self) -> &str {
        &self.type_name
    }

    pub fn category(&self) -> TypeCategory {
        self.category
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_parsed_only_for_types_that_compute() {
        assert_eq!(
            Value::from_text("t", TypeCategory::Boolean),
            Value::Bool(true)
        );
        assert_eq!(
            Value::from_text("42", TypeCategory::Integer),
            Value::Int(42)
        );
        assert_eq!(
            Value::from_text("1.50", TypeCategory::Decimal),
            Value::Text("1.50".into())
        );
        assert_eq!(
            Value::from_text("NaN", TypeCategory::Float)
                .display()
                .unwrap(),
            "NaN"
        );
    }

    #[test]
    fn a_number_too_large_for_i64_keeps_its_text() {
        let value = Value::from_text("99999999999999999999", TypeCategory::Integer);
        assert_eq!(value, Value::Text("99999999999999999999".into()));
    }

    #[test]
    fn nulls_sort_last_and_numbers_by_value() {
        let mut values = vec![
            Value::Null,
            Value::Int(10),
            Value::Float(2.5),
            Value::Int(3),
        ];
        values.sort_by(Value::sort_cmp);
        assert_eq!(
            values,
            vec![
                Value::Float(2.5),
                Value::Int(3),
                Value::Int(10),
                Value::Null
            ]
        );
    }
}
