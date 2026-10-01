use std::sync::Arc;

use datakit_catalog::{Column, Index, Relation, RelationType, Routine};
use datakit_driver::{Dialect, PlanNode, RowChange, Value, ddl};

use crate::{plan, types};

/// ClickHouse's syntax.
///
/// A ClickHouse database is DataKit's schema. Identifiers are
/// case-sensitive and quoted with backticks; strings use backslash escapes.
///
/// Data definition follows what the catalog holds for ClickHouse (see
/// [`ClickHouseDriver`](crate::ClickHouseDriver)): a table's definition is
/// its engine clause, which `CREATE TABLE` repeats; a table without one — a
/// new table — gets `MergeTree` ordered by its primary key columns. A
/// generated column is written `MATERIALIZED`, so an `ALIAS` column is
/// recreated as a stored one.
///
/// Rows change through mutations, `ALTER TABLE … UPDATE` and
/// `ALTER TABLE … DELETE`, which work on every MergeTree table and every
/// supported version; they run with `mutations_sync = 2`, so the change is
/// visible once the statement returns. The key of a ClickHouse table is not
/// unique: a change keyed on it applies to every row with that key, and
/// columns of the sorting key cannot be updated at all.
///
/// ClickHouse's transactions are experimental and off by default, so the
/// standard `BEGIN`/`COMMIT`/`ROLLBACK` are kept but a manual-commit console
/// fails on them unless the server enables them; each statement otherwise
/// commits on its own.
pub struct ClickHouseDialect;

impl Dialect for ClickHouseDialect {
    // Transactions are experimental in ClickHouse and off by default.
    fn supports_transactions(&self) -> bool {
        false
    }

    fn reserved_words(&self) -> &'static [&'static str] {
        RESERVED
    }

    fn keywords(&self) -> &'static [&'static str] {
        KEYWORDS
    }

    fn functions(&self) -> &'static [&'static str] {
        FUNCTIONS
    }

    fn data_types(&self) -> &'static [&'static str] {
        DATA_TYPES
    }

    fn identifier_quote(&self) -> char {
        '`'
    }

    /// ClickHouse is case-sensitive: an unquoted name means exactly what it
    /// says.
    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        let plain = identifier
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        let reserved = RESERVED
            .iter()
            .any(|word| word.eq_ignore_ascii_case(identifier));
        if plain && !reserved {
            return identifier.to_string();
        }
        format!("`{}`", identifier.replace('\\', "\\\\").replace('`', "\\`"))
    }

    fn string_literal(&self, text: &str) -> String {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
    }

    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(value) => value.to_string(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) if value.is_nan() => "nan".into(),
            Value::Float(value) if value.is_infinite() && value.is_sign_negative() => "-inf".into(),
            Value::Float(value) if value.is_infinite() => "inf".into(),
            Value::Float(value) => value.to_string(),
            Value::Text(text) => self.string_literal(text),
        }
    }

    /// ClickHouse cannot run a statement to measure it through `EXPLAIN`, so
    /// `analyze` asks for the same estimate-free plan: which steps run and
    /// how much each index prunes.
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        let _ = analyze;
        Some(format!(
            "EXPLAIN json = 1, description = 1, indexes = 1 {}",
            sql.trim().trim_end_matches(';')
        ))
    }

    fn parse_plan(&self, rows: &[Vec<Value>]) -> anyhow::Result<PlanNode> {
        let text: Vec<String> = rows
            .iter()
            .filter_map(|row| row.first().and_then(Value::display))
            .map(|text| text.into_owned())
            .collect();
        plan::parse(&text.join("\n"))
    }

    fn row_change(&self, schema: &str, relation: &str, change: &RowChange) -> String {
        let table = self.qualified_name(schema, relation);
        match change {
            // ClickHouse has no `DEFAULT VALUES`; an empty object inserts a
            // row of defaults.
            RowChange::Insert { values } if values.is_empty() => {
                format!("INSERT INTO {table} FORMAT JSONEachRow {{}}")
            }
            RowChange::Insert { .. } => ddl::row_change(self, schema, relation, change),
            RowChange::Update { key, values } => {
                let assignments: Vec<String> = values
                    .iter()
                    .map(|(column, value)| {
                        format!(
                            "{} = {}",
                            self.quote_identifier(column),
                            self.literal(value)
                        )
                    })
                    .collect();
                format!(
                    "ALTER TABLE {table} UPDATE {} WHERE {} SETTINGS mutations_sync = 2",
                    assignments.join(", "),
                    self.key_condition(key)
                )
            }
            RowChange::Delete { key } => format!(
                "ALTER TABLE {table} DELETE WHERE {} SETTINGS mutations_sync = 2",
                self.key_condition(key)
            ),
        }
    }

    fn create_relation(&self, schema: &str, relation: &Relation) -> Vec<String> {
        let name = self.qualified_name(schema, &relation.name());
        let mut statements = Vec::new();
        match relation.relation_type() {
            RelationType::View => {
                let query = relation.definition().unwrap_or("SELECT 1");
                statements.push(format!("CREATE VIEW {name} AS\n{}", trim_statement(query)));
            }
            RelationType::MaterializedView => {
                // The definition is everything after the name: `TO target AS
                // SELECT …`, or an engine and the query. A bare query gets an
                // engine of its own.
                let definition = trim_statement(relation.definition().unwrap_or("SELECT 1"));
                if starts_query(definition) {
                    statements.push(format!(
                        "CREATE MATERIALIZED VIEW {name}\nENGINE = MergeTree ORDER BY tuple() AS\n{definition}"
                    ));
                } else {
                    statements.push(format!("CREATE MATERIALIZED VIEW {name} {definition}"));
                }
            }
            _ => {
                let mut lines: Vec<String> = relation
                    .columns()
                    .iter()
                    .map(|column| {
                        let mut line = format!("    {}", self.column_definition(column));
                        if let Some(comment) = column.comment() {
                            line.push_str(&format!(" COMMENT {}", self.string_literal(comment)));
                        }
                        line
                    })
                    .collect();
                lines.extend(
                    relation.constraints().iter().map(|constraint| {
                        format!("    {}", self.constraint_definition(constraint))
                    }),
                );
                lines.extend(
                    ddl::standalone_indexes(relation)
                        .map(|index| format!("    {}", self.index_clause(index))),
                );
                let mut sql = format!(
                    "CREATE TABLE {name} (\n{}\n)\nENGINE = {}",
                    lines.join(",\n"),
                    self.engine(relation)
                );
                if let Some(comment) = relation.comment() {
                    sql.push_str(&format!("\nCOMMENT {}", self.string_literal(comment)));
                }
                statements.push(sql);
                return statements;
            }
        }
        if relation.comment().is_some()
            && let Some(comment) = self.comment_on_relation(schema, relation)
        {
            statements.push(comment);
        }
        statements
    }

    /// The column as `CREATE TABLE` and `ALTER TABLE … ADD COLUMN` write it:
    /// name, type — `Nullable` when the column is — and its default. The
    /// comment is a statement of its own.
    fn column_definition(&self, column: &Column) -> String {
        let mut definition = format!(
            "{} {}",
            self.quote_identifier(&column.name()),
            types::with_nullability(column.data_type(), column.is_nullable())
        );
        if let Some(default) = column.default() {
            let kind = if column.is_generated() {
                "MATERIALIZED"
            } else {
                "DEFAULT"
            };
            definition.push_str(&format!(" {kind} {default}"));
        }
        definition
    }

    /// `ALTER TABLE … ADD INDEX`. Like every data skipping index, it covers
    /// parts written from now on; `MATERIALIZE INDEX` builds it for the rest.
    fn create_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        format!(
            "ALTER TABLE {} ADD {}",
            self.qualified_name(schema, relation),
            self.index_clause(index)
        )
    }

    fn drop_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        format!(
            "ALTER TABLE {} DROP INDEX {}",
            self.qualified_name(schema, relation),
            self.quote_identifier(&index.name())
        )
    }

    fn drop_relation(&self, schema: &str, relation: &Relation) -> String {
        let object = if relation.relation_type().is_view() {
            "VIEW"
        } else {
            "TABLE"
        };
        format!(
            "DROP {object} {}",
            self.qualified_name(schema, &relation.name())
        )
    }

    /// SQL functions are global in ClickHouse, not in a database.
    fn drop_routine(&self, schema: &str, routine: &Routine) -> String {
        let _ = schema;
        format!("DROP FUNCTION {}", self.quote_identifier(&routine.name()))
    }

    fn alter_column(
        &self,
        schema: &str,
        relation: &str,
        old: &Column,
        new: &Column,
    ) -> Vec<String> {
        let table = self.qualified_name(schema, relation);
        let column = self.quote_identifier(&new.name());
        let mut statements = Vec::new();
        let old_type = types::with_nullability(old.data_type(), old.is_nullable());
        let new_type = types::with_nullability(new.data_type(), new.is_nullable());
        let default_changed =
            old.default() != new.default() || old.is_generated() != new.is_generated();
        // `MODIFY COLUMN` restates the type, and the default with it.
        if old_type != new_type || (default_changed && new.default().is_some()) {
            statements.push(format!(
                "ALTER TABLE {table} MODIFY COLUMN {}",
                self.column_definition(new)
            ));
        }
        if default_changed && new.default().is_none() {
            let kind = if old.is_generated() {
                "MATERIALIZED"
            } else {
                "DEFAULT"
            };
            statements.push(format!(
                "ALTER TABLE {table} MODIFY COLUMN {column} REMOVE {kind}"
            ));
        }
        if old.comment() != new.comment() {
            statements.extend(self.comment_on_column(schema, relation, new));
        }
        statements
    }

    fn rename_relation(&self, schema: &str, relation: &Relation, new: &str) -> String {
        format!(
            "RENAME TABLE {} TO {}",
            self.qualified_name(schema, &relation.name()),
            self.qualified_name(schema, new)
        )
    }

    /// An empty comment is how ClickHouse clears one.
    fn comment_on_relation(&self, schema: &str, relation: &Relation) -> Option<String> {
        Some(format!(
            "ALTER TABLE {} MODIFY COMMENT {}",
            self.qualified_name(schema, &relation.name()),
            self.string_literal(relation.comment().unwrap_or_default())
        ))
    }

    fn comment_on_column(&self, schema: &str, relation: &str, column: &Column) -> Option<String> {
        Some(format!(
            "ALTER TABLE {} COMMENT COLUMN {} {}",
            self.qualified_name(schema, relation),
            self.quote_identifier(&column.name()),
            self.string_literal(column.comment().unwrap_or_default())
        ))
    }
}

impl ClickHouseDialect {
    /// The engine clause of `relation`: its own, or `MergeTree` ordered by
    /// its primary key.
    fn engine(&self, relation: &Relation) -> String {
        if let Some(engine) = relation
            .definition()
            .map(str::trim)
            .filter(|engine| !engine.is_empty())
        {
            return engine.to_string();
        }
        let key: Vec<String> = relation
            .primary_key()
            .iter()
            .map(|column| self.quote_identifier(&column.name()))
            .collect();
        let order_by = match key.as_slice() {
            [] => "tuple()".to_string(),
            [column] => column.clone(),
            columns => format!("({})", columns.join(", ")),
        };
        format!("MergeTree ORDER BY {order_by}")
    }

    /// The index as `CREATE TABLE` declares it:
    /// `INDEX name expr TYPE type GRANULARITY n`.
    fn index_clause(&self, index: &Index) -> String {
        if let Some(definition) = index
            .definition()
            .filter(|definition| definition.trim_start().starts_with("INDEX "))
        {
            return trim_statement(definition).to_string();
        }
        let columns: Vec<String> = index
            .columns()
            .iter()
            .map(|column| {
                // An expression is kept as written; a name is quoted.
                if column.contains(['(', ' ', '`']) {
                    column.to_string()
                } else {
                    self.quote_identifier(column)
                }
            })
            .collect();
        let expression = match columns.as_slice() {
            [column] => column.clone(),
            columns => format!("({})", columns.join(", ")),
        };
        format!(
            "INDEX {} {expression} TYPE {} GRANULARITY 1",
            self.quote_identifier(&index.name()),
            index.method().unwrap_or("minmax")
        )
    }

    /// The condition that finds a row by `key`.
    fn key_condition(&self, key: &[(Arc<str>, Value)]) -> String {
        key.iter()
            .map(|(column, value)| {
                let column = self.quote_identifier(column);
                if value.is_null() {
                    format!("{column} IS NULL")
                } else {
                    format!("{column} = {}", self.literal(value))
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    }
}

fn trim_statement(sql: &str) -> &str {
    sql.trim().trim_end_matches(';').trim_end()
}

/// Whether `sql` is a query rather than the clauses before one.
fn starts_query(sql: &str) -> bool {
    let first = sql
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default();
    first.eq_ignore_ascii_case("SELECT")
        || first.eq_ignore_ascii_case("WITH")
        || sql.starts_with('(')
}

/// Types offered when a column is created or changed, most common first.
const DATA_TYPES: &[&str] = &[
    "UInt64",
    "Int64",
    "UInt32",
    "Int32",
    "UInt8",
    "Float64",
    "Decimal(18, 2)",
    "String",
    "LowCardinality(String)",
    "FixedString(16)",
    "Bool",
    "Date",
    "DateTime",
    "DateTime64(3)",
    "UUID",
    "Nullable(String)",
    "Nullable(Int64)",
    "Array(String)",
    "Array(UInt64)",
    "Map(String, String)",
    "Tuple(String, UInt64)",
    "JSON",
    "IPv4",
    "IPv6",
    "Enum8('a' = 1, 'b' = 2)",
];

/// Words that cannot name a column or table unquoted everywhere in a
/// statement. ClickHouse reserves almost nothing outright, but these are
/// read as clause keywords where an alias or name may stand, so quoting them
/// is always safe and sometimes necessary.
const RESERVED: &[&str] = &[
    "ALL",
    "AND",
    "ANTI",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "ASOF",
    "BETWEEN",
    "BY",
    "CASE",
    "CAST",
    "CROSS",
    "DESC",
    "DISTINCT",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "FALSE",
    "FINAL",
    "FORMAT",
    "FROM",
    "FULL",
    "GLOBAL",
    "GROUP",
    "HAVING",
    "ILIKE",
    "IN",
    "INNER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PASTE",
    "PREWHERE",
    "QUALIFY",
    "RIGHT",
    "SAMPLE",
    "SELECT",
    "SEMI",
    "SETTINGS",
    "THEN",
    "TO",
    "TRUE",
    "UNION",
    "USING",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

/// Keywords offered by completion, most used first within each group.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "PREWHERE",
    "AND",
    "OR",
    "NOT",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "RIGHT JOIN",
    "FULL JOIN",
    "CROSS JOIN",
    "ANY JOIN",
    "ASOF JOIN",
    "SEMI JOIN",
    "ANTI JOIN",
    "GLOBAL JOIN",
    "ARRAY JOIN",
    "LEFT ARRAY JOIN",
    "ON",
    "USING",
    "GROUP BY",
    "WITH ROLLUP",
    "WITH CUBE",
    "WITH TOTALS",
    "ORDER BY",
    "WITH FILL",
    "HAVING",
    "QUALIFY",
    "LIMIT",
    "LIMIT BY",
    "OFFSET",
    "AS",
    "DISTINCT",
    "IN",
    "GLOBAL IN",
    "NOT IN",
    "IS NULL",
    "IS NOT NULL",
    "LIKE",
    "ILIKE",
    "BETWEEN",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "UNION ALL",
    "UNION DISTINCT",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "FINAL",
    "SAMPLE",
    "SETTINGS",
    "FORMAT",
    "INSERT INTO",
    "VALUES",
    "ALTER TABLE",
    "UPDATE",
    "DELETE",
    "DELETE FROM",
    "ADD COLUMN",
    "DROP COLUMN",
    "MODIFY COLUMN",
    "RENAME COLUMN",
    "COMMENT COLUMN",
    "MODIFY COMMENT",
    "ADD INDEX",
    "DROP INDEX",
    "MATERIALIZE INDEX",
    "ADD PROJECTION",
    "DROP PARTITION",
    "DETACH PARTITION",
    "ATTACH PARTITION",
    "CREATE TABLE",
    "CREATE DATABASE",
    "CREATE VIEW",
    "CREATE MATERIALIZED VIEW",
    "CREATE DICTIONARY",
    "CREATE FUNCTION",
    "CREATE OR REPLACE",
    "IF NOT EXISTS",
    "IF EXISTS",
    "ENGINE",
    "MergeTree",
    "ReplacingMergeTree",
    "SummingMergeTree",
    "AggregatingMergeTree",
    "CollapsingMergeTree",
    "ReplicatedMergeTree",
    "PARTITION BY",
    "PRIMARY KEY",
    "TTL",
    "DEFAULT",
    "MATERIALIZED",
    "ALIAS",
    "CODEC",
    "COMMENT",
    "TO",
    "POPULATE",
    "DROP TABLE",
    "DROP VIEW",
    "DROP DATABASE",
    "TRUNCATE TABLE",
    "RENAME TABLE",
    "EXCHANGE TABLES",
    "OPTIMIZE TABLE",
    "SHOW TABLES",
    "SHOW DATABASES",
    "SHOW CREATE TABLE",
    "DESCRIBE TABLE",
    "EXPLAIN",
    "EXPLAIN PIPELINE",
    "EXPLAIN SYNTAX",
    "SYSTEM",
    "KILL QUERY",
    "USE",
    "SET",
    "GRANT",
    "REVOKE",
    "NULL",
    "TRUE",
    "FALSE",
    "ASC",
    "DESC",
    "NULLS FIRST",
    "NULLS LAST",
    "OVER",
    "INTERVAL",
];

/// Built-in functions offered by completion.
const FUNCTIONS: &[&str] = &[
    "count",
    "countIf",
    "sum",
    "sumIf",
    "avg",
    "avgIf",
    "min",
    "max",
    "any",
    "anyLast",
    "argMin",
    "argMax",
    "uniq",
    "uniqExact",
    "uniqCombined",
    "groupArray",
    "groupUniqArray",
    "groupBitmap",
    "quantile",
    "quantiles",
    "quantileExact",
    "quantileTDigest",
    "median",
    "topK",
    "stddevPop",
    "varPop",
    "if",
    "multiIf",
    "coalesce",
    "ifNull",
    "nullIf",
    "assumeNotNull",
    "toTypeName",
    "toString",
    "toInt64",
    "toUInt64",
    "toFloat64",
    "toDecimal64",
    "toDate",
    "toDateTime",
    "toDateTime64",
    "parseDateTimeBestEffort",
    "now",
    "now64",
    "today",
    "yesterday",
    "toStartOfDay",
    "toStartOfHour",
    "toStartOfMinute",
    "toStartOfWeek",
    "toStartOfMonth",
    "toStartOfYear",
    "toStartOfInterval",
    "toYYYYMM",
    "toYYYYMMDD",
    "dateDiff",
    "dateAdd",
    "formatDateTime",
    "toYear",
    "toMonth",
    "toDayOfWeek",
    "length",
    "lower",
    "upper",
    "trim",
    "substring",
    "replaceAll",
    "replaceRegexpAll",
    "splitByChar",
    "concat",
    "position",
    "match",
    "extract",
    "like",
    "lowerUTF8",
    "upperUTF8",
    "arrayJoin",
    "arrayMap",
    "arrayFilter",
    "arrayExists",
    "arraySort",
    "arrayDistinct",
    "arrayElement",
    "has",
    "hasAny",
    "hasAll",
    "indexOf",
    "range",
    "tuple",
    "tupleElement",
    "map",
    "mapKeys",
    "mapValues",
    "JSONExtract",
    "JSONExtractString",
    "JSONExtractInt",
    "JSONExtractFloat",
    "JSONExtractRaw",
    "visitParamExtractString",
    "generateUUIDv4",
    "rand",
    "cityHash64",
    "sipHash64",
    "xxHash64",
    "round",
    "floor",
    "ceil",
    "abs",
    "intDiv",
    "modulo",
    "greatest",
    "least",
    "row_number",
    "rank",
    "dense_rank",
    "lagInFrame",
    "leadInFrame",
    "formatReadableSize",
    "formatReadableQuantity",
    "currentDatabase",
    "version",
];

#[cfg(test)]
mod tests {
    use datakit_catalog::{Constraint, ConstraintRule};

    use super::*;

    #[test]
    fn identifiers_keep_their_case_and_quote_with_backticks() {
        let dialect = ClickHouseDialect;
        assert_eq!(dialect.quote_identifier("UserId"), "UserId");
        assert_eq!(dialect.quote_identifier("order"), "`order`");
        assert_eq!(dialect.quote_identifier("line items"), "`line items`");
        assert_eq!(dialect.quote_identifier("a`b\\c"), "`a\\`b\\\\c`");
        assert_eq!(dialect.quote_identifier("2fa"), "`2fa`");
        assert_eq!(dialect.fold_identifier("Events"), "Events");
        assert_eq!(dialect.qualified_name("db", "Events"), "db.Events");
    }

    #[test]
    fn literals_escape_backslashes_and_quotes() {
        let dialect = ClickHouseDialect;
        assert_eq!(dialect.string_literal("it's C:\\"), "'it\\'s C:\\\\'");
        assert_eq!(dialect.literal(&Value::Bool(true)), "true");
        assert_eq!(dialect.literal(&Value::Float(f64::NAN)), "nan");
        assert_eq!(dialect.literal(&Value::Float(f64::NEG_INFINITY)), "-inf");
        assert_eq!(dialect.literal(&Value::Float(1.5)), "1.5");
        assert_eq!(dialect.literal(&Value::Null), "NULL");
    }

    fn events() -> Relation {
        Relation::new("events", RelationType::Table)
            .with_columns([
                Column::new("id", "UInt64").primary_key(true),
                Column::new("at", "DateTime").with_default("now()"),
                Column::new("day", "Date")
                    .generated(true)
                    .with_default("toDate(at)"),
                Column::new("note", "Nullable(String)")
                    .nullable(true)
                    .with_comment("Free text"),
            ])
            .with_constraints([Constraint::new(
                "positive",
                ConstraintRule::Check {
                    expression: "id > 0".into(),
                },
            )
            .with_definition("CHECK id > 0")])
            .with_indexes([
                Index::new("primary_key", ["id"]).primary(true),
                Index::new("note_bloom", ["note"])
                    .with_method("bloom_filter(0.01)")
                    .with_definition("INDEX note_bloom note TYPE bloom_filter(0.01) GRANULARITY 4"),
            ])
            .with_comment("Everything that happened")
    }

    #[test]
    fn a_new_table_is_a_merge_tree_ordered_by_its_key() {
        let statements = ClickHouseDialect.create_relation("shop", &events());
        assert_eq!(
            statements,
            ["CREATE TABLE shop.events (\n    \
              id UInt64,\n    \
              at DateTime DEFAULT now(),\n    \
              day Date MATERIALIZED toDate(at),\n    \
              note Nullable(String) COMMENT 'Free text',\n    \
              CONSTRAINT positive CHECK id > 0,\n    \
              INDEX note_bloom note TYPE bloom_filter(0.01) GRANULARITY 4\n\
              )\nENGINE = MergeTree ORDER BY id\nCOMMENT 'Everything that happened'"]
        );
    }

    #[test]
    fn an_introspected_table_keeps_its_engine() {
        let table = Relation::new("log", RelationType::Table)
            .with_columns([Column::new("line", "String")])
            .with_definition(
                "ReplacingMergeTree(version) ORDER BY line SETTINGS index_granularity = 8192",
            );
        assert_eq!(
            ClickHouseDialect.create_relation("db", &table),
            [
                "CREATE TABLE db.log (\n    line String\n)\nENGINE = ReplacingMergeTree(version) \
              ORDER BY line SETTINGS index_granularity = 8192"
            ]
        );
        let keyless = Relation::new("t", RelationType::Table)
            .with_columns([Column::new("a", "String").nullable(true)]);
        assert_eq!(
            ClickHouseDialect.create_relation("db", &keyless),
            ["CREATE TABLE db.t (\n    a Nullable(String)\n)\nENGINE = MergeTree ORDER BY tuple()"]
        );
    }

    #[test]
    fn views_keep_their_query_and_materialized_views_their_target() {
        let view = Relation::new("recent", RelationType::View)
            .with_definition("SELECT * FROM db.events WHERE at > now() - 60;");
        assert_eq!(
            ClickHouseDialect.create_relation("db", &view),
            ["CREATE VIEW db.recent AS\nSELECT * FROM db.events WHERE at > now() - 60"]
        );
        let materialized = Relation::new("daily", RelationType::MaterializedView)
            .with_definition("TO db.daily_counts (`day` Date, `n` UInt64) AS SELECT day, count() AS n FROM db.events GROUP BY day");
        assert_eq!(
            ClickHouseDialect.create_relation("db", &materialized),
            [
                "CREATE MATERIALIZED VIEW db.daily TO db.daily_counts (`day` Date, `n` UInt64) AS \
              SELECT day, count() AS n FROM db.events GROUP BY day"
            ]
        );
        assert_eq!(
            ClickHouseDialect.drop_relation("db", &materialized),
            "DROP VIEW db.daily"
        );
    }

    #[test]
    fn changing_a_column_restates_its_type_and_default() {
        let dialect = ClickHouseDialect;
        let old = Column::new("note", "String").with_default("''");
        let new = Column::new("note", "String")
            .nullable(true)
            .with_comment("Optional");
        assert_eq!(
            dialect.alter_column("db", "t", &old, &new),
            [
                "ALTER TABLE db.t MODIFY COLUMN note Nullable(String)",
                "ALTER TABLE db.t MODIFY COLUMN note REMOVE DEFAULT",
                "ALTER TABLE db.t COMMENT COLUMN note 'Optional'",
            ]
        );
        let relation = Relation::new("t", RelationType::Table);
        assert_eq!(
            dialect.comment_on_relation("db", &relation).unwrap(),
            "ALTER TABLE db.t MODIFY COMMENT ''"
        );
        assert_eq!(
            dialect.create_index("db", "t", &Index::new("by_day", ["toDate(at)"])),
            "ALTER TABLE db.t ADD INDEX by_day toDate(at) TYPE minmax GRANULARITY 1"
        );
        assert_eq!(
            dialect.drop_index("db", "t", &Index::new("by_day", ["x"])),
            "ALTER TABLE db.t DROP INDEX by_day"
        );
    }

    #[test]
    fn row_changes_are_mutations() {
        let dialect = ClickHouseDialect;
        let key = vec![
            (Arc::from("id"), Value::Int(7)),
            (Arc::from("tag"), Value::Null),
        ];
        assert_eq!(
            dialect.row_change(
                "db",
                "events",
                &RowChange::Update {
                    key: key.clone(),
                    values: vec![("note".into(), Value::Text("O'Hara".into()))],
                }
            ),
            "ALTER TABLE db.events UPDATE note = 'O\\'Hara' WHERE id = 7 AND tag IS NULL \
             SETTINGS mutations_sync = 2"
        );
        assert_eq!(
            dialect.row_change("db", "events", &RowChange::Delete { key }),
            "ALTER TABLE db.events DELETE WHERE id = 7 AND tag IS NULL SETTINGS mutations_sync = 2"
        );
        assert_eq!(
            dialect.row_change("db", "events", &RowChange::Insert { values: vec![] }),
            "INSERT INTO db.events FORMAT JSONEachRow {}"
        );
        assert_eq!(
            dialect.row_change(
                "db",
                "events",
                &RowChange::Insert {
                    values: vec![("id".into(), Value::Int(1))]
                }
            ),
            "INSERT INTO db.events (id) VALUES (1)"
        );
    }

    #[test]
    fn explaining_asks_for_a_json_plan() {
        assert_eq!(
            ClickHouseDialect.explain("SELECT 1;", false).unwrap(),
            "EXPLAIN json = 1, description = 1, indexes = 1 SELECT 1"
        );
    }
}
