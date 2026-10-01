use datakit_driver::TypeCategory;
use tokio_postgres::types::{Kind, Type};

/// The category of a PostgreSQL type, looking through domains.
pub(crate) fn category(ty: &Type) -> TypeCategory {
    match ty.kind() {
        Kind::Array(_) => return TypeCategory::Array,
        Kind::Domain(inner) => return category(inner),
        Kind::Enum(_) => return TypeCategory::Text,
        _ => {}
    }
    match *ty {
        Type::BOOL => TypeCategory::Boolean,
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => TypeCategory::Integer,
        Type::FLOAT4 | Type::FLOAT8 => TypeCategory::Float,
        Type::NUMERIC | Type::MONEY => TypeCategory::Decimal,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::CHAR | Type::UNKNOWN => {
            TypeCategory::Text
        }
        Type::BYTEA => TypeCategory::Binary,
        Type::DATE
        | Type::TIME
        | Type::TIMETZ
        | Type::TIMESTAMP
        | Type::TIMESTAMPTZ
        | Type::INTERVAL => TypeCategory::Temporal,
        Type::JSON | Type::JSONB => TypeCategory::Json,
        Type::UUID => TypeCategory::Uuid,
        Type::INET | Type::CIDR | Type::MACADDR | Type::MACADDR8 => TypeCategory::Network,
        _ => TypeCategory::Other,
    }
}

/// The SQL-standard spelling for the common types, the catalog name
/// otherwise.
pub(crate) fn display_name(ty: &Type) -> String {
    let name = match *ty {
        Type::BOOL => "boolean",
        Type::INT2 => "smallint",
        Type::INT4 => "integer",
        Type::INT8 => "bigint",
        Type::FLOAT4 => "real",
        Type::FLOAT8 => "double precision",
        Type::VARCHAR => "varchar",
        Type::BPCHAR => "char",
        Type::TIMESTAMPTZ => "timestamptz",
        Type::TIMETZ => "timetz",
        _ => {
            if let Kind::Array(element) = ty.kind() {
                return format!("{}[]", display_name(element));
            }
            ty.name()
        }
    };
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrays_are_arrays_whatever_they_hold() {
        assert_eq!(category(&Type::INT4_ARRAY), TypeCategory::Array);
        assert_eq!(display_name(&Type::INT4_ARRAY), "integer[]");
    }

    #[test]
    fn exact_numbers_are_decimal_not_float() {
        assert_eq!(category(&Type::NUMERIC), TypeCategory::Decimal);
        assert_eq!(category(&Type::FLOAT8), TypeCategory::Float);
    }
}
