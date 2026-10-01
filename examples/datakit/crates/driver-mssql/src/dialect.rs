use datakit_catalog::{
    Column, Constraint, ConstraintRule, Index, ReferentialAction, Relation, RelationChange,
    RelationType, Routine, RoutineType, Sequence, Trigger,
};
use datakit_driver::{Dialect, PlanNode, Value, ddl};

use crate::{connection::PlanSetting, plan};

/// SQL Server's syntax, Transact-SQL.
///
/// Names are quoted with brackets. An unquoted name means what it says: a
/// default SQL Server collation compares names without regard to case, so
/// nothing folds them, and `Orders` needs no quotes.
///
/// Data definition follows what SQL Server accepts, with limits worth
/// knowing:
///
/// - A column's default is a constraint with a name. A default the
///   dialect adds is called `DF_<table>_<column>`; one it removes or
///   replaces is found by the column, whatever its name, by a statement
///   that looks it up in `sys.default_constraints` first.
/// - Changing a column's type or nullability restates both, as
///   `ALTER COLUMN` requires.
/// - Identity and computed columns cannot change in place; a change to
///   either is not written.
/// - Comments are `MS_Description` extended properties, which is where
///   Management Studio keeps them.
pub struct SqlServerDialect;

impl Dialect for SqlServerDialect {
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
        '['
    }

    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    /// `identifier` in brackets when it is not a regular identifier: one
    /// that starts with a letter or `_` and goes on with letters, digits,
    /// `_`, `@`, `#` or `$`, and is not reserved.
    fn quote_identifier(&self, identifier: &str) -> String {
        let regular = identifier
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && identifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '@' | '#' | '$'));
        let reserved = RESERVED
            .iter()
            .any(|word| word.eq_ignore_ascii_case(identifier));
        if regular && !reserved {
            identifier.to_string()
        } else {
            format!("[{}]", identifier.replace(']', "]]"))
        }
    }

    /// `N'…'` for text beyond ASCII, which a plain literal would read in
    /// the database's code page.
    fn string_literal(&self, text: &str) -> String {
        let prefix = if text.is_ascii() { "" } else { "N" };
        format!("{prefix}'{}'", text.replace('\'', "''"))
    }

    /// Booleans are `bit`s: `1` and `0`.
    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(true) => "1".into(),
            Value::Bool(false) => "0".into(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) if value.is_finite() => value.to_string(),
            Value::Float(value) => self.string_literal(&value.to_string()),
            Value::Text(text) => self.string_literal(text),
        }
    }

    fn select_rows_where(
        &self,
        schema: &str,
        relation: &str,
        condition: &str,
        order_by: &str,
        limit: Option<u64>,
    ) -> String {
        let mut sql = match limit {
            Some(limit) => format!("SELECT TOP ({limit}) *"),
            None => "SELECT *".to_string(),
        };
        sql.push_str(&format!(" FROM {}", self.qualified_name(schema, relation)));
        if !condition.trim().is_empty() {
            sql.push_str(&format!(" WHERE {}", condition.trim()));
        }
        if !order_by.trim().is_empty() {
            sql.push_str(&format!(" ORDER BY {}", order_by.trim()));
        }
        sql
    }

    /// `OFFSET … FETCH` pages, which SQL Server allows only after an
    /// `ORDER BY`; without one the rows keep the order the server reads
    /// them in.
    fn select_page(
        &self,
        schema: &str,
        relation: &str,
        condition: &str,
        order_by: &str,
        limit: u64,
        offset: u64,
    ) -> String {
        let order_by = if order_by.trim().is_empty() {
            "(SELECT NULL)"
        } else {
            order_by.trim()
        };
        format!(
            "{} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY",
            self.select_rows_where(schema, relation, condition, order_by, None)
        )
    }

    fn begin_transaction(&self) -> &'static str {
        "BEGIN TRANSACTION"
    }

    /// The setting that asks for a plan, then the statement. SQL Server
    /// wants the setting in a batch of its own; the connection sends it so,
    /// answers with the plan documents, and turns the setting off again.
    /// The estimated plan does not run the statement; the actual one does.
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        let setting = if analyze {
            PlanSetting::Actual
        } else {
            PlanSetting::Estimated
        };
        Some(format!("{}{}", setting.prefix(), sql.trim()))
    }

    fn parse_plan(&self, rows: &[Vec<Value>]) -> anyhow::Result<PlanNode> {
        let documents: Vec<String> = rows
            .iter()
            .filter_map(|row| row.first().and_then(Value::display))
            .map(|text| text.into_owned())
            .collect();
        plan::parse(&documents)
    }

    /// A view whose definition is the whole `CREATE VIEW` statement, as
    /// SQL Server keeps it, is created by that statement.
    fn create_relation(&self, schema: &str, relation: &Relation) -> Vec<String> {
        let mut statements = ddl::create_relation(self, schema, relation);
        if relation.relation_type().is_view()
            && let Some(definition) = relation.definition()
            && starts_with_create(definition)
        {
            statements[0] = definition.trim().trim_end_matches(';').to_string();
        }
        statements
    }

    fn column_definition(&self, column: &Column) -> String {
        let name = self.quote_identifier(&column.name());
        if column.is_generated()
            && let Some(expression) = column.default()
        {
            return format!("{name} AS ({expression})");
        }
        let mut definition = format!("{name} {}", column.data_type());
        if column.is_auto_increment() {
            definition.push_str(" IDENTITY(1,1)");
        } else if let Some(default) = column.default() {
            definition.push_str(&format!(" DEFAULT {default}"));
        }
        definition.push_str(if column.is_nullable() {
            " NULL"
        } else {
            " NOT NULL"
        });
        definition
    }

    /// The standard definition, except that SQL Server has no `RESTRICT`
    /// action: `NO ACTION` is what it means there.
    fn constraint_definition(&self, constraint: &Constraint) -> String {
        if constraint.definition().is_none()
            && let ConstraintRule::ForeignKey(key) = constraint.rule()
        {
            let restricted = |action: ReferentialAction| {
                if action == ReferentialAction::Restrict {
                    ReferentialAction::NoAction
                } else {
                    action
                }
            };
            let key = key
                .clone()
                .with_on_update(restricted(key.on_update()))
                .with_on_delete(restricted(key.on_delete()));
            let constraint = Constraint::new(constraint.name(), ConstraintRule::ForeignKey(key));
            return ddl::constraint_definition(self, &constraint);
        }
        ddl::constraint_definition(self, constraint)
    }

    /// `CLUSTERED` or `NONCLUSTERED` when the index's method says which;
    /// SQL Server has no `USING`.
    fn create_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        if index.definition().is_some() {
            return ddl::create_index(self, schema, relation, index);
        }
        let clustering = match index.method().map(str::to_ascii_lowercase).as_deref() {
            Some("clustered") => "CLUSTERED ",
            Some("nonclustered") => "NONCLUSTERED ",
            _ => "",
        };
        // The same index without its method, which the standard statement
        // would write as `USING`.
        let mut plain = Index::new(index.name(), index.columns().iter().cloned())
            .unique(index.is_unique())
            .primary(index.is_primary());
        if let Some(predicate) = index.predicate() {
            plain = plain.with_predicate(predicate);
        }
        let sql = ddl::create_index(self, schema, relation, &plain);
        sql.replacen("INDEX ", &format!("{clustering}INDEX "), 1)
    }

    fn create_sequence(&self, schema: &str, sequence: &Sequence) -> String {
        let mut sql = format!(
            "CREATE SEQUENCE {} AS {} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {}",
            self.qualified_name(schema, &sequence.name()),
            sequence.data_type(),
            sequence.start(),
            sequence.increment(),
            sequence.min_value(),
            sequence.max_value()
        );
        sql.push_str(if sequence.is_cycle() {
            " CYCLE"
        } else {
            " NO CYCLE"
        });
        sql
    }

    fn drop_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        format!(
            "DROP INDEX {} ON {}",
            self.quote_identifier(&index.name()),
            self.qualified_name(schema, relation)
        )
    }

    /// A routine is dropped by its name; SQL Server does not overload.
    fn drop_routine(&self, schema: &str, routine: &Routine) -> String {
        let object = match routine.routine_type() {
            RoutineType::Procedure => "PROCEDURE",
            RoutineType::Aggregate => "AGGREGATE",
            _ => "FUNCTION",
        };
        format!(
            "DROP {object} {}",
            self.qualified_name(schema, &routine.name())
        )
    }

    /// A trigger on a table belongs to the table's schema.
    fn drop_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> String {
        let _ = relation;
        format!(
            "DROP TRIGGER {}",
            self.qualified_name(schema, &trigger.name())
        )
    }

    fn add_column(&self, schema: &str, relation: &str, column: &Column) -> String {
        format!(
            "ALTER TABLE {} ADD {}",
            self.qualified_name(schema, relation),
            self.column_definition(column)
        )
    }

    fn alter_relation(&self, schema: &str, change: &RelationChange) -> Vec<String> {
        let relation = change.source();
        if relation.relation_type().is_view() {
            return ddl::alter_relation(self, schema, change);
        }
        let name = relation.name();
        let table = self.qualified_name(schema, &name);
        let mut statements = Vec::new();
        for trigger in change.removed_triggers() {
            statements.push(self.drop_trigger(schema, &name, trigger));
        }
        for index in change.removed_indexes() {
            if !change
                .removed_constraints()
                .iter()
                .any(|constraint| constraint.name() == index.name())
                && !index.is_primary()
            {
                statements.push(self.drop_index(schema, &name, index));
            }
        }
        for constraint in change.removed_constraints() {
            statements.push(format!(
                "ALTER TABLE {table} DROP CONSTRAINT {}",
                self.quote_identifier(&constraint.name())
            ));
        }
        for column in change.added_columns() {
            // `ADD`, without `COLUMN`.
            statements.push(format!(
                "ALTER TABLE {table} ADD {}",
                self.column_definition(column)
            ));
            if column.comment().is_some() {
                statements.extend(self.comment_on_column(schema, &name, column));
            }
        }
        for (old, new) in change.changed_columns() {
            statements.extend(self.alter_column(schema, &name, old, new));
        }
        for constraint in change.added_constraints() {
            statements.push(format!(
                "ALTER TABLE {table} ADD {}",
                self.constraint_definition(constraint)
            ));
        }
        for index in change.added_indexes() {
            if !change
                .added_constraints()
                .iter()
                .any(|constraint| constraint.name() == index.name())
                && !index.is_primary()
            {
                statements.push(self.create_index(schema, &name, index));
            }
        }
        for trigger in change.added_triggers() {
            statements.extend(self.create_trigger(schema, &name, trigger));
        }
        for column in change.removed_columns() {
            // A column with a default cannot be dropped before the default.
            if column.default().is_some() && !column.is_generated() {
                statements.push(self.drop_default(schema, &name, &column.name()));
            }
            statements.push(format!(
                "ALTER TABLE {table} DROP COLUMN {}",
                self.quote_identifier(&column.name())
            ));
        }
        if change.is_comment_changed() {
            statements.extend(self.comment_on_relation(schema, relation));
        }
        statements
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
        let computed = old.is_generated() || new.is_generated();
        let default_changed = !computed && old.default() != new.default();
        if default_changed && old.default().is_some() {
            statements.push(self.drop_default(schema, relation, &new.name()));
        }
        if !computed
            && (old.data_type() != new.data_type() || old.is_nullable() != new.is_nullable())
        {
            statements.push(format!(
                "ALTER TABLE {table} ALTER COLUMN {column} {} {}",
                new.data_type(),
                if new.is_nullable() {
                    "NULL"
                } else {
                    "NOT NULL"
                }
            ));
        }
        if default_changed && let Some(default) = new.default() {
            statements.push(format!(
                "ALTER TABLE {table} ADD CONSTRAINT {} DEFAULT {default} FOR {column}",
                self.quote_identifier(&format!("DF_{relation}_{}", new.name()))
            ));
        }
        if old.comment() != new.comment() {
            statements.extend(self.comment_on_column(schema, relation, new));
        }
        statements
    }

    fn rename_column(&self, schema: &str, relation: &str, old: &str, new: &str) -> String {
        format!(
            "EXEC sp_rename {}, {}, 'COLUMN'",
            self.string_literal(&format!(
                "{}.{}",
                self.qualified_name(schema, relation),
                self.quote_identifier(old)
            )),
            self.string_literal(new)
        )
    }

    fn rename_relation(&self, schema: &str, relation: &Relation, new: &str) -> String {
        format!(
            "EXEC sp_rename {}, {}",
            self.string_literal(&self.qualified_name(schema, &relation.name())),
            self.string_literal(new)
        )
    }

    fn comment_on_relation(&self, schema: &str, relation: &Relation) -> Option<String> {
        let level = match relation.relation_type() {
            RelationType::View | RelationType::MaterializedView => "VIEW",
            _ => "TABLE",
        };
        Some(self.description(schema, level, &relation.name(), None, relation.comment()))
    }

    fn comment_on_column(&self, schema: &str, relation: &str, column: &Column) -> Option<String> {
        Some(self.description(
            schema,
            "TABLE",
            relation,
            Some(&column.name()),
            column.comment(),
        ))
    }
}

impl SqlServerDialect {
    /// The statement that drops the default of `column`, whatever the
    /// constraint is called. One line without semicolons, so a script
    /// splitter keeps it whole.
    fn drop_default(&self, schema: &str, relation: &str, column: &str) -> String {
        let table = self.qualified_name(schema, relation);
        let object = self.string_literal(&table);
        format!(
            "DECLARE @default sysname = (SELECT name FROM sys.default_constraints \
             WHERE parent_object_id = OBJECT_ID({object}) AND parent_column_id = \
             COLUMNPROPERTY(OBJECT_ID({object}), {}, 'ColumnId')) \
             IF @default IS NOT NULL EXEC ({} + QUOTENAME(@default))",
            self.string_literal(column),
            self.string_literal(&format!("ALTER TABLE {table} DROP CONSTRAINT "))
        )
    }

    /// The statement that sets, replaces or removes the `MS_Description` of
    /// a table or view, or of one of its columns.
    fn description(
        &self,
        schema: &str,
        level: &str,
        relation: &str,
        column: Option<&str>,
        comment: Option<&str>,
    ) -> String {
        let object = self.string_literal(&self.qualified_name(schema, relation));
        let minor = match column {
            Some(column) => format!(
                "COLUMNPROPERTY(OBJECT_ID({object}), {}, 'ColumnId')",
                self.string_literal(column)
            ),
            None => "0".to_string(),
        };
        let exists = format!(
            "IF EXISTS (SELECT 1 FROM sys.extended_properties WHERE class = 1 \
             AND major_id = OBJECT_ID({object}) AND minor_id = {minor} \
             AND name = N'MS_Description')"
        );
        let mut levels = format!(
            "@level0type = N'SCHEMA', @level0name = {}, @level1type = N'{level}', \
             @level1name = {}",
            self.string_literal(schema),
            self.string_literal(relation)
        );
        if let Some(column) = column {
            levels.push_str(&format!(
                ", @level2type = N'COLUMN', @level2name = {}",
                self.string_literal(column)
            ));
        }
        match comment {
            Some(comment) => {
                let value = format!("@value = N'{}'", comment.replace('\'', "''"));
                format!(
                    "{exists} EXEC sys.sp_updateextendedproperty @name = N'MS_Description', \
                     {value}, {levels} ELSE EXEC sys.sp_addextendedproperty \
                     @name = N'MS_Description', {value}, {levels}"
                )
            }
            None => format!(
                "{exists} EXEC sys.sp_dropextendedproperty @name = N'MS_Description', {levels}"
            ),
        }
    }
}

/// Whether `definition` is a whole statement rather than a query.
fn starts_with_create(definition: &str) -> bool {
    let mut rest = definition;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail);
        } else {
            break;
        }
    }
    rest.get(..6)
        .is_some_and(|word| word.eq_ignore_ascii_case("CREATE"))
}

/// Types offered when a column is created or changed, most common first.
const DATA_TYPES: &[&str] = &[
    "int",
    "bigint",
    "smallint",
    "tinyint",
    "bit",
    "decimal(18,2)",
    "numeric(18,0)",
    "money",
    "float",
    "real",
    "nvarchar(255)",
    "nvarchar(max)",
    "varchar(255)",
    "varchar(max)",
    "nchar(10)",
    "char(10)",
    "date",
    "time",
    "datetime2",
    "datetimeoffset",
    "datetime",
    "smalldatetime",
    "uniqueidentifier",
    "varbinary(max)",
    "binary(16)",
    "xml",
    "sql_variant",
    "rowversion",
    "hierarchyid",
    "geography",
    "geometry",
];

/// Transact-SQL's reserved keywords, from "Reserved Keywords (Transact-SQL)"
/// in the SQL Server documentation.
const RESERVED: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "AUTHORIZATION",
    "BACKUP",
    "BEGIN",
    "BETWEEN",
    "BREAK",
    "BROWSE",
    "BULK",
    "BY",
    "CASCADE",
    "CASE",
    "CHECK",
    "CHECKPOINT",
    "CLOSE",
    "CLUSTERED",
    "COALESCE",
    "COLLATE",
    "COLUMN",
    "COMMIT",
    "COMPUTE",
    "CONSTRAINT",
    "CONTAINS",
    "CONTAINSTABLE",
    "CONTINUE",
    "CONVERT",
    "CREATE",
    "CROSS",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "CURSOR",
    "DATABASE",
    "DBCC",
    "DEALLOCATE",
    "DECLARE",
    "DEFAULT",
    "DELETE",
    "DENY",
    "DESC",
    "DISK",
    "DISTINCT",
    "DISTRIBUTED",
    "DOUBLE",
    "DROP",
    "DUMP",
    "ELSE",
    "END",
    "ERRLVL",
    "ESCAPE",
    "EXCEPT",
    "EXEC",
    "EXECUTE",
    "EXISTS",
    "EXIT",
    "EXTERNAL",
    "FETCH",
    "FILE",
    "FILLFACTOR",
    "FOR",
    "FOREIGN",
    "FREETEXT",
    "FREETEXTTABLE",
    "FROM",
    "FULL",
    "FUNCTION",
    "GOTO",
    "GRANT",
    "GROUP",
    "HAVING",
    "HOLDLOCK",
    "IDENTITY",
    "IDENTITY_INSERT",
    "IDENTITYCOL",
    "IF",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "KILL",
    "LEFT",
    "LIKE",
    "LINENO",
    "LOAD",
    "MERGE",
    "NATIONAL",
    "NOCHECK",
    "NONCLUSTERED",
    "NOT",
    "NULL",
    "NULLIF",
    "OF",
    "OFF",
    "OFFSETS",
    "ON",
    "OPEN",
    "OPENDATASOURCE",
    "OPENQUERY",
    "OPENROWSET",
    "OPENXML",
    "OPTION",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PERCENT",
    "PIVOT",
    "PLAN",
    "PRECISION",
    "PRIMARY",
    "PRINT",
    "PROC",
    "PROCEDURE",
    "PUBLIC",
    "RAISERROR",
    "READ",
    "READTEXT",
    "RECONFIGURE",
    "REFERENCES",
    "REPLICATION",
    "RESTORE",
    "RESTRICT",
    "RETURN",
    "REVERT",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROWCOUNT",
    "ROWGUIDCOL",
    "RULE",
    "SAVE",
    "SCHEMA",
    "SECURITYAUDIT",
    "SELECT",
    "SEMANTICKEYPHRASETABLE",
    "SEMANTICSIMILARITYDETAILSTABLE",
    "SEMANTICSIMILARITYTABLE",
    "SESSION_USER",
    "SET",
    "SETUSER",
    "SHUTDOWN",
    "SOME",
    "STATISTICS",
    "SYSTEM_USER",
    "TABLE",
    "TABLESAMPLE",
    "TEXTSIZE",
    "THEN",
    "TO",
    "TOP",
    "TRAN",
    "TRANSACTION",
    "TRIGGER",
    "TRUNCATE",
    "TRY_CONVERT",
    "TSEQUAL",
    "UNION",
    "UNIQUE",
    "UNPIVOT",
    "UPDATE",
    "UPDATETEXT",
    "USE",
    "USER",
    "VALUES",
    "VARYING",
    "VIEW",
    "WAITFOR",
    "WHEN",
    "WHERE",
    "WHILE",
    "WITH",
    "WRITETEXT",
];

/// Keywords offered by completion, most used first within each group.
const KEYWORDS: &[&str] = &[
    "SELECT",
    "TOP",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "RIGHT JOIN",
    "FULL JOIN",
    "CROSS JOIN",
    "CROSS APPLY",
    "OUTER APPLY",
    "ON",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "OFFSET",
    "FETCH NEXT",
    "ROWS ONLY",
    "AS",
    "DISTINCT",
    "IN",
    "IS NULL",
    "IS NOT NULL",
    "LIKE",
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
    "INSERT INTO",
    "VALUES",
    "OUTPUT",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "MERGE INTO",
    "WHEN MATCHED",
    "WHEN NOT MATCHED",
    "CREATE TABLE",
    "CREATE VIEW",
    "CREATE INDEX",
    "CREATE CLUSTERED INDEX",
    "CREATE NONCLUSTERED INDEX",
    "CREATE SCHEMA",
    "CREATE PROCEDURE",
    "CREATE FUNCTION",
    "CREATE TRIGGER",
    "CREATE OR ALTER",
    "ALTER TABLE",
    "ADD",
    "ALTER COLUMN",
    "DROP COLUMN",
    "DROP TABLE",
    "DROP VIEW",
    "DROP INDEX",
    "TRUNCATE TABLE",
    "PRIMARY KEY",
    "FOREIGN KEY",
    "REFERENCES",
    "UNIQUE",
    "CHECK",
    "DEFAULT",
    "IDENTITY",
    "NOT NULL",
    "BEGIN TRANSACTION",
    "COMMIT",
    "ROLLBACK",
    "SAVE TRANSACTION",
    "BEGIN TRY",
    "END TRY",
    "BEGIN CATCH",
    "END CATCH",
    "DECLARE",
    "EXEC",
    "PRINT",
    "RAISERROR",
    "THROW",
    "WAITFOR DELAY",
    "USE",
    "GRANT",
    "REVOKE",
    "NULL",
    "ASC",
    "DESC",
    "OVER",
    "PARTITION BY",
    "PIVOT",
    "UNPIVOT",
    "COLLATE",
    "WITH (NOLOCK)",
    "OPTION (RECOMPILE)",
];

/// Built-in functions offered by completion.
const FUNCTIONS: &[&str] = &[
    "COUNT",
    "COUNT_BIG",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "STRING_AGG",
    "COALESCE",
    "ISNULL",
    "NULLIF",
    "IIF",
    "CHOOSE",
    "CAST",
    "CONVERT",
    "TRY_CAST",
    "TRY_CONVERT",
    "PARSE",
    "TRY_PARSE",
    "FORMAT",
    "GETDATE",
    "GETUTCDATE",
    "SYSDATETIME",
    "SYSUTCDATETIME",
    "SYSDATETIMEOFFSET",
    "CURRENT_TIMESTAMP",
    "DATEADD",
    "DATEDIFF",
    "DATEDIFF_BIG",
    "DATEPART",
    "DATENAME",
    "DATEFROMPARTS",
    "EOMONTH",
    "YEAR",
    "MONTH",
    "DAY",
    "LEN",
    "DATALENGTH",
    "LOWER",
    "UPPER",
    "LTRIM",
    "RTRIM",
    "TRIM",
    "SUBSTRING",
    "LEFT",
    "RIGHT",
    "REPLACE",
    "CHARINDEX",
    "PATINDEX",
    "CONCAT",
    "CONCAT_WS",
    "STUFF",
    "REPLICATE",
    "REVERSE",
    "STRING_SPLIT",
    "QUOTENAME",
    "JSON_VALUE",
    "JSON_QUERY",
    "JSON_MODIFY",
    "OPENJSON",
    "ISJSON",
    "ROW_NUMBER",
    "RANK",
    "DENSE_RANK",
    "NTILE",
    "LAG",
    "LEAD",
    "FIRST_VALUE",
    "LAST_VALUE",
    "NEWID",
    "NEWSEQUENTIALID",
    "ROUND",
    "ABS",
    "CEILING",
    "FLOOR",
    "POWER",
    "SQRT",
    "RAND",
    "OBJECT_ID",
    "OBJECT_NAME",
    "SCHEMA_NAME",
    "DB_NAME",
    "SCOPE_IDENTITY",
    "@@ROWCOUNT",
    "@@IDENTITY",
    "@@VERSION",
];

#[cfg(test)]
mod tests {
    use datakit_catalog::{ForeignKey, Schema, diff_schemas};

    use super::*;

    fn orders() -> Relation {
        Relation::new("orders", RelationType::Table)
            .with_columns([
                Column::new("id", "int").auto_increment(true),
                Column::new("total", "decimal(12,2)").with_default("0"),
                Column::new("note", "nvarchar(80)").nullable(true),
            ])
            .with_constraints([Constraint::new(
                "PK_orders",
                ConstraintRule::PrimaryKey {
                    columns: vec!["id".into()].into(),
                },
            )])
    }

    #[test]
    fn identifiers_are_bracketed_only_when_needed() {
        let dialect = SqlServerDialect;
        assert_eq!(dialect.quote_identifier("Orders"), "Orders");
        assert_eq!(dialect.quote_identifier("order"), "[order]");
        assert_eq!(dialect.quote_identifier("line items"), "[line items]");
        assert_eq!(dialect.quote_identifier("odd]name"), "[odd]]name]");
        assert_eq!(dialect.quote_identifier("2fa"), "[2fa]");
        assert_eq!(dialect.fold_identifier("Orders"), "Orders");
        assert_eq!(dialect.qualified_name("dbo", "user"), "dbo.[user]");
    }

    #[test]
    fn rows_are_limited_with_top() {
        let dialect = SqlServerDialect;
        assert_eq!(
            dialect.select_rows("dbo", "orders", Some(500)),
            "SELECT TOP (500) * FROM dbo.orders"
        );
        assert_eq!(
            dialect.select_rows_where("dbo", "orders", " total > 1 ", "id DESC", Some(10)),
            "SELECT TOP (10) * FROM dbo.orders WHERE total > 1 ORDER BY id DESC"
        );
        assert_eq!(
            dialect.select_rows("dbo", "orders", None),
            "SELECT * FROM dbo.orders"
        );
    }

    #[test]
    fn literals_are_transact_sql() {
        let dialect = SqlServerDialect;
        assert_eq!(dialect.literal(&Value::Bool(true)), "1");
        assert_eq!(dialect.literal(&Value::Text("it's".into())), "'it''s'");
        assert_eq!(dialect.literal(&Value::Text("Grüße".into())), "N'Grüße'");
        assert_eq!(dialect.literal(&Value::Null), "NULL");
    }

    #[test]
    fn a_table_is_created_with_identity_and_explicit_nullability() {
        let statements = SqlServerDialect.create_relation("dbo", &orders());
        assert_eq!(
            statements[0],
            "CREATE TABLE dbo.orders (\n    id int IDENTITY(1,1) NOT NULL,\n    \
             total decimal(12,2) DEFAULT 0 NOT NULL,\n    note nvarchar(80) NULL,\n    \
             CONSTRAINT PK_orders PRIMARY KEY (id)\n)"
        );
    }

    #[test]
    fn a_view_with_its_whole_statement_is_created_by_it() {
        let view = Relation::new("big", RelationType::View)
            .with_definition("CREATE VIEW dbo.big AS SELECT * FROM dbo.orders WHERE total > 100");
        assert_eq!(
            SqlServerDialect.create_relation("dbo", &view)[0],
            "CREATE VIEW dbo.big AS SELECT * FROM dbo.orders WHERE total > 100"
        );
        let query = Relation::new("big", RelationType::View).with_definition("SELECT 1 AS one");
        assert_eq!(
            SqlServerDialect.create_relation("dbo", &query)[0],
            "CREATE VIEW dbo.big AS\nSELECT 1 AS one"
        );
    }

    #[test]
    fn a_changed_column_restates_type_and_nullability() {
        let old = Column::new("note", "nvarchar(80)").nullable(true);
        let new = Column::new("note", "nvarchar(200)");
        assert_eq!(
            SqlServerDialect.alter_column("dbo", "orders", &old, &new),
            ["ALTER TABLE dbo.orders ALTER COLUMN note nvarchar(200) NOT NULL"]
        );
    }

    #[test]
    fn a_changed_default_replaces_the_constraint() {
        let old = Column::new("total", "money").with_default("0");
        let new = Column::new("total", "money").with_default("1");
        let statements = SqlServerDialect.alter_column("dbo", "orders", &old, &new);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].starts_with("DECLARE @default sysname"));
        assert!(statements[0].contains("OBJECT_ID('dbo.orders')"));
        assert!(
            statements[0]
                .contains("EXEC ('ALTER TABLE dbo.orders DROP CONSTRAINT ' + QUOTENAME(@default))")
        );
        assert!(!statements[0].contains(';'));
        assert_eq!(
            statements[1],
            "ALTER TABLE dbo.orders ADD CONSTRAINT DF_orders_total DEFAULT 1 FOR total"
        );
    }

    #[test]
    fn renames_go_through_sp_rename() {
        let dialect = SqlServerDialect;
        assert_eq!(
            dialect.rename_column("dbo", "orders", "note", "remark"),
            "EXEC sp_rename 'dbo.orders.note', 'remark', 'COLUMN'"
        );
        assert_eq!(
            dialect.rename_column("sales", "order lines", "qty", "quantity"),
            "EXEC sp_rename 'sales.[order lines].qty', 'quantity', 'COLUMN'"
        );
        assert_eq!(
            dialect.rename_relation("dbo", &orders(), "purchases"),
            "EXEC sp_rename 'dbo.orders', 'purchases'"
        );
    }

    #[test]
    fn objects_are_dropped_the_transact_sql_way() {
        let dialect = SqlServerDialect;
        assert_eq!(
            dialect.drop_index("dbo", "orders", &Index::new("IX_total", ["total"])),
            "DROP INDEX IX_total ON dbo.orders"
        );
        assert_eq!(
            dialect.drop_routine(
                "dbo",
                &Routine::new("touch", RoutineType::Procedure, "@id int")
            ),
            "DROP PROCEDURE dbo.touch"
        );
        assert_eq!(
            dialect.drop_trigger(
                "dbo",
                "orders",
                &Trigger::new("orders_audit", "AFTER INSERT")
            ),
            "DROP TRIGGER dbo.orders_audit"
        );
    }

    #[test]
    fn indexes_say_clustered_instead_of_using() {
        let index = Index::new("IX_total", ["total"])
            .with_method("nonclustered")
            .with_predicate("total > 100");
        assert_eq!(
            SqlServerDialect.create_index("dbo", "orders", &index),
            "CREATE NONCLUSTERED INDEX IX_total ON dbo.orders (total) WHERE total > 100"
        );
    }

    #[test]
    fn a_restricting_key_becomes_no_action() {
        let key = Constraint::new(
            "FK_orders_customers",
            ConstraintRule::ForeignKey(
                ForeignKey::new(["customer_id"], "dbo", "customers", ["id"])
                    .with_on_delete(ReferentialAction::Restrict)
                    .with_on_update(ReferentialAction::Cascade),
            ),
        );
        assert_eq!(
            SqlServerDialect.constraint_definition(&key),
            "CONSTRAINT FK_orders_customers FOREIGN KEY (customer_id) REFERENCES dbo.customers (id) \
             ON UPDATE CASCADE"
        );
    }

    #[test]
    fn comments_are_extended_properties() {
        let relation = orders().with_comment("Every order");
        let sql = SqlServerDialect
            .comment_on_relation("dbo", &relation)
            .unwrap();
        assert!(sql.starts_with("IF EXISTS (SELECT 1 FROM sys.extended_properties"));
        assert!(sql.contains("EXEC sys.sp_updateextendedproperty"));
        assert!(sql.contains("ELSE EXEC sys.sp_addextendedproperty"));
        assert!(sql.contains("@value = N'Every order'"));
        assert!(sql.contains("@level1type = N'TABLE', @level1name = 'orders'"));

        let cleared = SqlServerDialect.comment_on_column(
            "dbo",
            "orders",
            &Column::new("note", "nvarchar(80)"),
        );
        let cleared = cleared.unwrap();
        assert!(cleared.contains("sp_dropextendedproperty"));
        assert!(cleared.contains("@level2type = N'COLUMN', @level2name = 'note'"));
    }

    #[test]
    fn a_migration_adds_columns_without_column_and_drops_defaults_first() {
        let target = Schema::new("dbo").with_relations([orders()]);
        let mut changed = orders().columns().to_vec();
        changed.remove(1);
        changed.push(Column::new("placed", "datetime2").with_default("sysdatetime()"));
        let source = Schema::new("dbo").with_relations([orders().with_columns(changed)]);
        let statements = SqlServerDialect.migrate("dbo", &diff_schemas(&target, &source));
        assert_eq!(
            statements[0],
            "ALTER TABLE dbo.orders ADD placed datetime2 DEFAULT sysdatetime() NOT NULL"
        );
        assert!(statements[1].starts_with("DECLARE @default sysname"));
        assert_eq!(statements[2], "ALTER TABLE dbo.orders DROP COLUMN total");
        assert_eq!(statements.len(), 3);
    }

    #[test]
    fn explain_asks_for_xml_plans() {
        let dialect = SqlServerDialect;
        assert_eq!(
            dialect.explain("SELECT 1", false).unwrap(),
            "SET SHOWPLAN_XML ON;\nSELECT 1"
        );
        assert_eq!(
            dialect.explain("SELECT 1", true).unwrap(),
            "SET STATISTICS XML ON;\nSELECT 1"
        );
    }

    #[test]
    fn sequences_and_transactions_are_spelled_out() {
        let dialect = SqlServerDialect;
        let sequence = Sequence::new("invoice_numbers", "bigint")
            .with_start(1000)
            .with_increment(10)
            .with_range(1, 9_999_999);
        assert_eq!(
            dialect.create_sequence("dbo", &sequence),
            "CREATE SEQUENCE dbo.invoice_numbers AS bigint START WITH 1000 INCREMENT BY 10 \
             MINVALUE 1 MAXVALUE 9999999 NO CYCLE"
        );
        assert_eq!(dialect.begin_transaction(), "BEGIN TRANSACTION");
        assert_eq!(dialect.commit(), "COMMIT");
        assert_eq!(dialect.rollback(), "ROLLBACK");
    }
}
