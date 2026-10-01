use datakit_catalog::{
    Column, Constraint, ConstraintRule, Index, Relation, RelationType, Routine, Trigger,
};
use datakit_driver::{Dialect, PlanNode, RowChange, Value, ddl};

use crate::plan;

/// MySQL's syntax, which MariaDB shares.
///
/// A MySQL database is a schema: names are qualified with it, and the
/// explorer shows the server's databases as its schemas.
pub struct MySqlDialect;

impl Dialect for MySqlDialect {
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

    /// MySQL keeps an identifier's case. Whether `Orders` and `orders` name
    /// the same table depends on the server's `lower_case_table_names`, so
    /// DataKit never changes the case itself.
    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    /// `identifier`, quoted only when it has to be: when it is not a plain
    /// word or is reserved. Upper case is plain, since MySQL does not fold
    /// it; a leading digit is quoted, since `1e5` would read as a number.
    fn quote_identifier(&self, identifier: &str) -> String {
        let plain = identifier
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
        let reserved = RESERVED
            .iter()
            .any(|word| word.eq_ignore_ascii_case(identifier));
        if plain && !reserved {
            return identifier.to_string();
        }
        format!("`{}`", identifier.replace('`', "``"))
    }

    /// `text` as a string literal. MySQL reads a backslash in a literal as
    /// an escape, so it is doubled along with the quote. A session that sets
    /// `NO_BACKSLASH_ESCAPES` would read the doubled backslash as two; the
    /// literal is written for the default mode.
    fn string_literal(&self, text: &str) -> String {
        format!("'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
    }

    fn row_change(&self, schema: &str, relation: &str, change: &RowChange) -> String {
        match change {
            // MySQL has no `DEFAULT VALUES`.
            RowChange::Insert { values } if values.is_empty() => format!(
                "INSERT INTO {} () VALUES ()",
                self.qualified_name(schema, relation)
            ),
            change => ddl::row_change(self, schema, relation, change),
        }
    }

    fn begin_transaction(&self) -> &'static str {
        "START TRANSACTION"
    }

    /// `EXPLAIN FORMAT=JSON`, or with `analyze`, `EXPLAIN ANALYZE`, which
    /// runs the statement and answers with an indented text tree (MySQL
    /// 8.0.18 and later; MariaDB spells it `ANALYZE FORMAT=JSON` and is not
    /// asked).
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        Some(if analyze {
            format!("EXPLAIN ANALYZE {sql}")
        } else {
            format!("EXPLAIN FORMAT=JSON {sql}")
        })
    }

    fn parse_plan(&self, rows: &[Vec<Value>]) -> anyhow::Result<PlanNode> {
        let text: Vec<String> = rows
            .iter()
            .filter_map(|row| row.first().and_then(Value::display))
            .map(|text| text.into_owned())
            .collect();
        plan::parse(&text.join("\n"))
    }

    /// The column as MySQL writes it: name, type, nullability, default or
    /// generation, `AUTO_INCREMENT`, and the comment, which MySQL keeps with
    /// the column rather than in a statement of its own.
    fn column_definition(&self, column: &Column) -> String {
        let mut definition = format!(
            "{} {}",
            self.quote_identifier(&column.name()),
            column.data_type()
        );
        if column.is_generated()
            && let Some(expression) = column.default()
        {
            // MySQL's default kind of generated column, `VIRTUAL`.
            definition.push_str(&format!(" GENERATED ALWAYS AS ({expression})"));
        }
        if !column.is_nullable() {
            definition.push_str(" NOT NULL");
        }
        if !column.is_generated()
            && let Some(default) = column.default()
        {
            definition.push_str(&format!(" DEFAULT {default}"));
        }
        if column.is_auto_increment() {
            definition.push_str(" AUTO_INCREMENT");
        }
        if let Some(comment) = column.comment() {
            definition.push_str(&format!(" COMMENT {}", self.string_literal(comment)));
        }
        definition
    }

    /// A primary key is always called `PRIMARY` in MySQL, so it is written
    /// without a name.
    fn constraint_definition(&self, constraint: &Constraint) -> String {
        match constraint.rule() {
            ConstraintRule::PrimaryKey { columns } if constraint.definition().is_none() => {
                let columns: Vec<String> = columns
                    .iter()
                    .map(|column| self.quote_identifier(column))
                    .collect();
                format!("PRIMARY KEY ({})", columns.join(", "))
            }
            _ => ddl::constraint_definition(self, constraint),
        }
    }

    /// `CREATE INDEX`, with `FULLTEXT` and `SPATIAL` written where MySQL
    /// wants them: as the kind of index, not as its method.
    fn create_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        if let Some(definition) = index.definition() {
            return definition.trim().trim_end_matches(';').to_string();
        }
        let method = index.method().map(str::to_ascii_uppercase);
        let prefix = match method.as_deref() {
            Some(kind @ ("FULLTEXT" | "SPATIAL")) => format!("{kind} "),
            _ if index.is_unique() => "UNIQUE ".into(),
            _ => String::new(),
        };
        let columns: Vec<String> = index
            .columns()
            .iter()
            .map(|column| {
                // A prefix (`name(10)`), an order (`total DESC`) or an
                // expression is kept as written; a name is quoted.
                if column.contains(['(', ' ', '`']) {
                    column.to_string()
                } else {
                    self.quote_identifier(column)
                }
            })
            .collect();
        let mut sql = format!(
            "CREATE {prefix}INDEX {} ON {} ({})",
            self.quote_identifier(&index.name()),
            self.qualified_name(schema, relation),
            columns.join(", ")
        );
        if method.as_deref() == Some("HASH") {
            sql.push_str(" USING HASH");
        }
        sql
    }

    fn drop_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        format!(
            "DROP INDEX {} ON {}",
            self.quote_identifier(&index.name()),
            self.qualified_name(schema, relation)
        )
    }

    /// MySQL names a routine without its arguments: there is no
    /// overloading.
    fn drop_routine(&self, schema: &str, routine: &Routine) -> String {
        let object = match routine.routine_type() {
            datakit_catalog::RoutineType::Procedure => "PROCEDURE",
            _ => "FUNCTION",
        };
        format!(
            "DROP {object} {}",
            self.qualified_name(schema, &routine.name())
        )
    }

    /// A trigger belongs to its schema, not to its table, in MySQL.
    fn drop_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> String {
        let _ = relation;
        format!(
            "DROP TRIGGER {}",
            self.qualified_name(schema, &trigger.name())
        )
    }

    /// MySQL cannot change a column's type, nullability or default one at a
    /// time; `MODIFY COLUMN` restates the whole column, comment included.
    fn alter_column(
        &self,
        schema: &str,
        relation: &str,
        old: &Column,
        new: &Column,
    ) -> Vec<String> {
        if old == new {
            return Vec::new();
        }
        vec![format!(
            "ALTER TABLE {} MODIFY COLUMN {}",
            self.qualified_name(schema, relation),
            self.column_definition(new)
        )]
    }

    /// `RENAME TABLE`, which renames a view as well and keeps the schema.
    fn rename_relation(&self, schema: &str, relation: &Relation, new: &str) -> String {
        format!(
            "RENAME TABLE {} TO {}",
            self.qualified_name(schema, &relation.name()),
            self.qualified_name(schema, new)
        )
    }

    /// `ALTER TABLE … COMMENT`; a view has no comment in MySQL.
    fn comment_on_relation(&self, schema: &str, relation: &Relation) -> Option<String> {
        if relation.relation_type() == RelationType::View {
            return None;
        }
        Some(format!(
            "ALTER TABLE {} COMMENT = {}",
            self.qualified_name(schema, &relation.name()),
            self.string_literal(relation.comment().unwrap_or(""))
        ))
    }

    /// Always `None`: a column's comment is part of its definition, written
    /// by [`Self::column_definition`] wherever the column is created or
    /// modified.
    fn comment_on_column(&self, schema: &str, relation: &str, column: &Column) -> Option<String> {
        let _ = (schema, relation, column);
        None
    }
}

/// Types offered when a column is created or changed, most common first.
const DATA_TYPES: &[&str] = &[
    "int",
    "bigint",
    "smallint",
    "tinyint",
    "int unsigned",
    "bigint unsigned",
    "decimal(10,2)",
    "double",
    "float",
    "tinyint(1)",
    "varchar(255)",
    "char(1)",
    "text",
    "mediumtext",
    "longtext",
    "date",
    "time",
    "datetime",
    "timestamp",
    "year",
    "json",
    "binary(16)",
    "varbinary(255)",
    "blob",
    "longblob",
    "enum('a','b')",
    "set('a','b')",
    "bit(1)",
];

/// Words MySQL 8 reserves; an identifier spelled like one must be quoted.
/// From the "Keywords and Reserved Words" section of the MySQL 8.4 manual,
/// the words marked (R).
const RESERVED: &[&str] = &[
    "ACCESSIBLE",
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "AS",
    "ASC",
    "ASENSITIVE",
    "BEFORE",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BLOB",
    "BOTH",
    "BY",
    "CALL",
    "CASCADE",
    "CASE",
    "CHANGE",
    "CHAR",
    "CHARACTER",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "CONDITION",
    "CONSTRAINT",
    "CONTINUE",
    "CONVERT",
    "CREATE",
    "CROSS",
    "CUBE",
    "CUME_DIST",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "DATABASE",
    "DATABASES",
    "DAY_HOUR",
    "DAY_MICROSECOND",
    "DAY_MINUTE",
    "DAY_SECOND",
    "DEC",
    "DECIMAL",
    "DECLARE",
    "DEFAULT",
    "DELAYED",
    "DELETE",
    "DENSE_RANK",
    "DESC",
    "DESCRIBE",
    "DETERMINISTIC",
    "DISTINCT",
    "DISTINCTROW",
    "DIV",
    "DOUBLE",
    "DROP",
    "DUAL",
    "EACH",
    "ELSE",
    "ELSEIF",
    "EMPTY",
    "ENCLOSED",
    "ESCAPED",
    "EXCEPT",
    "EXISTS",
    "EXIT",
    "EXPLAIN",
    "FALSE",
    "FETCH",
    "FIRST_VALUE",
    "FLOAT",
    "FLOAT4",
    "FLOAT8",
    "FOR",
    "FORCE",
    "FOREIGN",
    "FROM",
    "FULLTEXT",
    "FUNCTION",
    "GENERATED",
    "GET",
    "GRANT",
    "GROUP",
    "GROUPING",
    "GROUPS",
    "HAVING",
    "HIGH_PRIORITY",
    "HOUR_MICROSECOND",
    "HOUR_MINUTE",
    "HOUR_SECOND",
    "IF",
    "IGNORE",
    "IN",
    "INDEX",
    "INFILE",
    "INNER",
    "INOUT",
    "INSENSITIVE",
    "INSERT",
    "INT",
    "INT1",
    "INT2",
    "INT3",
    "INT4",
    "INT8",
    "INTEGER",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IO_AFTER_GTIDS",
    "IO_BEFORE_GTIDS",
    "IS",
    "ITERATE",
    "JOIN",
    "JSON_TABLE",
    "KEY",
    "KEYS",
    "KILL",
    "LAG",
    "LAST_VALUE",
    "LATERAL",
    "LEAD",
    "LEADING",
    "LEAVE",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LINEAR",
    "LINES",
    "LOAD",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "LOCK",
    "LONG",
    "LONGBLOB",
    "LONGTEXT",
    "LOOP",
    "LOW_PRIORITY",
    "MANUAL",
    "MASTER_BIND",
    "MASTER_SSL_VERIFY_SERVER_CERT",
    "MATCH",
    "MAXVALUE",
    "MEDIUMBLOB",
    "MEDIUMINT",
    "MEDIUMTEXT",
    "MIDDLEINT",
    "MINUTE_MICROSECOND",
    "MINUTE_SECOND",
    "MOD",
    "MODIFIES",
    "NATURAL",
    "NOT",
    "NO_WRITE_TO_BINLOG",
    "NTH_VALUE",
    "NTILE",
    "NULL",
    "NUMERIC",
    "OF",
    "ON",
    "OPTIMIZE",
    "OPTIMIZER_COSTS",
    "OPTION",
    "OPTIONALLY",
    "OR",
    "ORDER",
    "OUT",
    "OUTER",
    "OUTFILE",
    "OVER",
    "PARALLEL",
    "PARTITION",
    "PERCENT_RANK",
    "PRECISION",
    "PRIMARY",
    "PROCEDURE",
    "PURGE",
    "QUALIFY",
    "RANGE",
    "RANK",
    "READ",
    "READS",
    "READ_WRITE",
    "REAL",
    "RECURSIVE",
    "REFERENCES",
    "REGEXP",
    "RELEASE",
    "RENAME",
    "REPEAT",
    "REPLACE",
    "REQUIRE",
    "RESIGNAL",
    "RESTRICT",
    "RETURN",
    "REVOKE",
    "RIGHT",
    "RLIKE",
    "ROW",
    "ROWS",
    "ROW_NUMBER",
    "SCHEMA",
    "SCHEMAS",
    "SECOND_MICROSECOND",
    "SELECT",
    "SENSITIVE",
    "SEPARATOR",
    "SET",
    "SHOW",
    "SIGNAL",
    "SMALLINT",
    "SPATIAL",
    "SPECIFIC",
    "SQL",
    "SQLEXCEPTION",
    "SQLSTATE",
    "SQLWARNING",
    "SQL_BIG_RESULT",
    "SQL_CALC_FOUND_ROWS",
    "SQL_SMALL_RESULT",
    "SSL",
    "STARTING",
    "STORED",
    "STRAIGHT_JOIN",
    "SYSTEM",
    "TABLE",
    "TABLESAMPLE",
    "TERMINATED",
    "THEN",
    "TINYBLOB",
    "TINYINT",
    "TINYTEXT",
    "TO",
    "TRAILING",
    "TRIGGER",
    "TRUE",
    "UNDO",
    "UNION",
    "UNIQUE",
    "UNLOCK",
    "UNSIGNED",
    "UPDATE",
    "USAGE",
    "USE",
    "USING",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "VARCHARACTER",
    "VARYING",
    "VIRTUAL",
    "WHEN",
    "WHERE",
    "WHILE",
    "WINDOW",
    "WITH",
    "WRITE",
    "XOR",
    "YEAR_MONTH",
    "ZEROFILL",
];

/// Keywords offered by completion, most used first within each group.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "RIGHT JOIN",
    "CROSS JOIN",
    "STRAIGHT_JOIN",
    "ON",
    "USING",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "AS",
    "DISTINCT",
    "IN",
    "IS NULL",
    "IS NOT NULL",
    "LIKE",
    "REGEXP",
    "BETWEEN",
    "EXISTS",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "UNION",
    "UNION ALL",
    "INTERSECT",
    "EXCEPT",
    "WITH",
    "RECURSIVE",
    "INSERT INTO",
    "INSERT IGNORE INTO",
    "REPLACE INTO",
    "VALUES",
    "ON DUPLICATE KEY UPDATE",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "CREATE TABLE",
    "CREATE VIEW",
    "CREATE INDEX",
    "CREATE DATABASE",
    "CREATE FUNCTION",
    "CREATE PROCEDURE",
    "CREATE TRIGGER",
    "CREATE OR REPLACE VIEW",
    "ALTER TABLE",
    "ADD COLUMN",
    "MODIFY COLUMN",
    "CHANGE COLUMN",
    "RENAME COLUMN",
    "DROP COLUMN",
    "DROP TABLE",
    "DROP VIEW",
    "DROP INDEX",
    "RENAME TABLE",
    "TRUNCATE TABLE",
    "PRIMARY KEY",
    "FOREIGN KEY",
    "REFERENCES",
    "UNIQUE",
    "CHECK",
    "DEFAULT",
    "NOT NULL",
    "AUTO_INCREMENT",
    "COMMENT",
    "ENGINE",
    "CHARACTER SET",
    "COLLATE",
    "START TRANSACTION",
    "BEGIN",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "LOCK TABLES",
    "UNLOCK TABLES",
    "EXPLAIN",
    "EXPLAIN ANALYZE",
    "EXPLAIN FORMAT=JSON",
    "ANALYZE TABLE",
    "OPTIMIZE TABLE",
    "SHOW DATABASES",
    "SHOW TABLES",
    "SHOW COLUMNS FROM",
    "SHOW CREATE TABLE",
    "SHOW INDEX FROM",
    "SHOW PROCESSLIST",
    "SHOW VARIABLES",
    "SHOW STATUS",
    "DESCRIBE",
    "USE",
    "CALL",
    "GRANT",
    "REVOKE",
    "NULL",
    "TRUE",
    "FALSE",
    "ASC",
    "DESC",
    "OVER",
    "PARTITION BY",
    "WINDOW",
    "LATERAL",
    "INTERVAL",
    "FOR UPDATE",
];

/// Built-in functions offered by completion.
const FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "coalesce",
    "ifnull",
    "nullif",
    "if",
    "greatest",
    "least",
    "now",
    "curdate",
    "curtime",
    "current_timestamp",
    "date_format",
    "str_to_date",
    "date_add",
    "date_sub",
    "datediff",
    "timestampdiff",
    "unix_timestamp",
    "from_unixtime",
    "year",
    "month",
    "day",
    "extract",
    "length",
    "char_length",
    "lower",
    "upper",
    "trim",
    "substring",
    "substring_index",
    "replace",
    "concat",
    "concat_ws",
    "group_concat",
    "lpad",
    "rpad",
    "locate",
    "format",
    "cast",
    "convert",
    "json_extract",
    "json_unquote",
    "json_object",
    "json_array",
    "json_arrayagg",
    "json_objectagg",
    "json_contains",
    "json_table",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "round",
    "abs",
    "ceil",
    "floor",
    "rand",
    "uuid",
    "md5",
    "sha2",
    "hex",
    "unhex",
    "regexp_replace",
    "regexp_like",
    "last_insert_id",
    "found_rows",
    "database",
    "version",
];

#[cfg(test)]
mod tests {
    use datakit_catalog::{ForeignKey, ReferentialAction, Schema, diff_schemas};

    use super::*;

    fn orders() -> Relation {
        Relation::new("orders", RelationType::Table)
            .with_columns([
                Column::new("id", "bigint unsigned")
                    .primary_key(true)
                    .auto_increment(true),
                Column::new("customer_id", "int"),
                Column::new("note", "varchar(80)")
                    .nullable(true)
                    .with_default("'none'")
                    .with_comment("Free text"),
            ])
            .with_constraints([
                Constraint::new(
                    "PRIMARY",
                    ConstraintRule::PrimaryKey {
                        columns: vec!["id".into()].into(),
                    },
                ),
                Constraint::new(
                    "orders_customer_fk",
                    ConstraintRule::ForeignKey(
                        ForeignKey::new(["customer_id"], "shop", "customers", ["id"])
                            .with_on_delete(ReferentialAction::Cascade),
                    ),
                ),
            ])
            .with_indexes([
                Index::new("PRIMARY", ["id"]).unique(true).primary(true),
                Index::new("orders_customer_fk", ["customer_id"]),
                Index::new("orders_note", ["`note`(10)", "`customer_id` DESC"]),
            ])
            .with_comment("Every order")
    }

    #[test]
    fn identifiers_keep_their_case_and_quote_with_backticks() {
        assert_eq!(MySqlDialect.quote_identifier("orders"), "orders");
        assert_eq!(MySqlDialect.quote_identifier("OrderLines"), "OrderLines");
        assert_eq!(MySqlDialect.quote_identifier("order"), "`order`");
        assert_eq!(MySqlDialect.quote_identifier("line items"), "`line items`");
        assert_eq!(MySqlDialect.quote_identifier("say`hi"), "`say``hi`");
        assert_eq!(MySqlDialect.quote_identifier("2fa"), "`2fa`");
        assert_eq!(
            MySqlDialect.select_rows("shop", "Key", Some(10)),
            "SELECT * FROM shop.`Key` LIMIT 10"
        );
    }

    #[test]
    fn literals_escape_backslashes_and_quotes() {
        assert_eq!(
            MySqlDialect.literal(&Value::Text(r"C:\temp\it's".into())),
            r"'C:\\temp\\it''s'"
        );
        assert_eq!(MySqlDialect.literal(&Value::Bool(true)), "TRUE");
    }

    #[test]
    fn a_table_is_created_with_inline_comments() {
        let statements = MySqlDialect.create_relation("shop", &orders());
        assert_eq!(
            statements[0],
            "CREATE TABLE shop.orders (\n    \
             id bigint unsigned NOT NULL AUTO_INCREMENT,\n    \
             customer_id int NOT NULL,\n    \
             note varchar(80) DEFAULT 'none' COMMENT 'Free text',\n    \
             PRIMARY KEY (id),\n    \
             CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id) REFERENCES shop.customers (id) ON DELETE CASCADE\n)"
        );
        assert_eq!(
            statements[1],
            "ALTER TABLE shop.orders COMMENT = 'Every order'"
        );
        assert_eq!(
            statements[2],
            "CREATE INDEX orders_note ON shop.orders (`note`(10), `customer_id` DESC)"
        );
        assert_eq!(statements.len(), 3, "the key indexes are not repeated");
    }

    #[test]
    fn a_changed_column_is_restated_with_modify_column() {
        let target = Schema::new("shop").with_relations([orders()]);
        let mut columns = orders().columns().to_vec();
        columns[1] = columns[1].clone().with_data_type("bigint").nullable(true);
        columns[2] = columns[2].clone().with_comment("Said by the customer");
        columns.push(
            Column::new("placed_at", "datetime")
                .with_default("CURRENT_TIMESTAMP")
                .with_comment("When"),
        );
        let source = Schema::new("shop").with_relations([orders().with_columns(columns)]);
        let statements = MySqlDialect.migrate("shop", &diff_schemas(&target, &source));
        assert_eq!(
            statements,
            vec![
                "ALTER TABLE shop.orders ADD COLUMN placed_at datetime NOT NULL DEFAULT CURRENT_TIMESTAMP COMMENT 'When'",
                "ALTER TABLE shop.orders MODIFY COLUMN customer_id bigint",
                "ALTER TABLE shop.orders MODIFY COLUMN note varchar(80) DEFAULT 'none' COMMENT 'Said by the customer'",
            ]
        );
    }

    #[test]
    fn generated_columns_are_computed_not_defaulted() {
        let total = Column::new("total", "decimal(12,2)")
            .nullable(true)
            .generated(true)
            .with_default("(`price` * `quantity`)");
        assert_eq!(
            MySqlDialect.column_definition(&total),
            "total decimal(12,2) GENERATED ALWAYS AS ((`price` * `quantity`))"
        );
    }

    #[test]
    fn drops_name_objects_the_mysql_way() {
        let index = Index::new("orders_note", ["note"]);
        assert_eq!(
            MySqlDialect.drop_index("shop", "orders", &index),
            "DROP INDEX orders_note ON shop.orders"
        );
        let trigger = Trigger::new("orders_touch", "BEFORE INSERT FOR EACH ROW");
        assert_eq!(
            MySqlDialect.drop_trigger("shop", "orders", &trigger),
            "DROP TRIGGER shop.orders_touch"
        );
        let routine = Routine::new(
            "total",
            datakit_catalog::RoutineType::Function,
            "order_id bigint",
        );
        assert_eq!(
            MySqlDialect.drop_routine("shop", &routine),
            "DROP FUNCTION shop.total"
        );
        assert_eq!(
            MySqlDialect.rename_column("shop", "orders", "note", "remark"),
            "ALTER TABLE shop.orders RENAME COLUMN note TO remark"
        );
        assert_eq!(
            MySqlDialect.rename_relation("shop", &orders(), "purchases"),
            "RENAME TABLE shop.orders TO shop.purchases"
        );
        assert_eq!(
            MySqlDialect.row_change("shop", "orders", &RowChange::Insert { values: vec![] }),
            "INSERT INTO shop.orders () VALUES ()"
        );
    }
}
