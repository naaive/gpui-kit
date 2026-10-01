use datakit_catalog::{
    Column, Constraint, ConstraintRule, Index, Relation, RelationChange, RelationType, SchemaDiff,
    Trigger,
};
use datakit_driver::{Dialect, PlanNode, Value, ddl};

use crate::{parse, plan, types};

/// SQLite's syntax.
///
/// Names keep the case they were written in, and SQLite matches them without
/// regard to case, so a name is quoted only when it is not a plain word or
/// is a keyword; `Orders` stays `Orders` and reads back the same.
///
/// A connection's databases (`main`, `temp`, attached ones) are schemas to
/// the catalog, but `main` is what an unqualified name means, so names in
/// `main` are written bare and only the others are qualified.
///
/// SQLite cannot change a column in place or add and drop constraints;
/// [`Dialect::alter_relation`] rebuilds the table in those cases, following
/// the procedure in SQLite's `ALTER TABLE` documentation.
pub struct SqliteDialect;

impl Dialect for SqliteDialect {
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

    /// Names are matched without regard to case and stored as written, so
    /// folding keeps them as they are.
    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    fn has_schemas(&self) -> bool {
        false
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
            identifier.to_string()
        } else {
            format!("\"{}\"", identifier.replace('"', "\"\""))
        }
    }

    fn qualified_name(&self, schema: &str, relation: &str) -> String {
        if is_main(schema) {
            self.quote_identifier(relation)
        } else {
            format!(
                "{}.{}",
                self.quote_identifier(schema),
                self.quote_identifier(relation)
            )
        }
    }

    /// Text that reads as a blob literal, as blobs are shown, is written as
    /// one, so an edited row keeps its blobs. SQLite has no infinity or NaN
    /// literal: infinities overflow a real, and NaN is stored as `NULL`.
    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Text(text) if types::is_blob_literal(text) => text.to_string(),
            Value::Float(value) if value.is_nan() => "NULL".into(),
            Value::Float(value) if value.is_infinite() => {
                if value.is_sign_positive() {
                    "9e999".into()
                } else {
                    "-9e999".into()
                }
            }
            value => ddl_literal(self, value),
        }
    }

    /// SQLite plans but cannot measure a statement, so `analyze` asks for
    /// the same plan.
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        let _ = analyze;
        Some(format!("EXPLAIN QUERY PLAN {sql}"))
    }

    fn parse_plan(&self, rows: &[Vec<Value>]) -> anyhow::Result<PlanNode> {
        plan::parse(rows)
    }

    fn create_relation(&self, schema: &str, relation: &Relation) -> Vec<String> {
        if relation.relation_type().is_view() {
            let query = relation
                .definition()
                .unwrap_or("SELECT 1")
                .trim()
                .trim_end_matches(';');
            return vec![format!(
                "CREATE VIEW {} AS\n{query}",
                self.qualified_name(schema, &relation.name())
            )];
        }
        let mut statements = vec![self.create_table(schema, &relation.name(), relation)];
        statements.extend(
            ddl::standalone_indexes(relation)
                .map(|index| self.create_index(schema, &relation.name(), index)),
        );
        statements.extend(
            relation
                .triggers()
                .iter()
                .filter_map(|trigger| self.create_trigger(schema, &relation.name(), trigger)),
        );
        statements
    }

    /// The column as `CREATE TABLE` declares it. A default that is not a
    /// literal is parenthesized, as SQLite requires; a generated column is
    /// `VIRTUAL`, the kind `ALTER TABLE … ADD COLUMN` accepts.
    fn column_definition(&self, column: &Column) -> String {
        let mut definition = self.quote_identifier(&column.name());
        if !column.data_type().trim().is_empty() {
            definition.push(' ');
            definition.push_str(column.data_type());
        }
        if let Some(default) = column.default() {
            if column.is_generated() {
                definition.push_str(&format!(" GENERATED ALWAYS AS ({default})"));
            } else {
                definition.push_str(&format!(" DEFAULT {}", default_expression(default)));
            }
        }
        if !column.is_nullable() {
            definition.push_str(" NOT NULL");
        }
        definition
    }

    /// The constraint as `CREATE TABLE` declares it. A foreign key names its
    /// table bare: SQLite resolves it in the referencing table's database.
    fn constraint_definition(&self, constraint: &Constraint) -> String {
        let ConstraintRule::ForeignKey(key) = constraint.rule() else {
            return ddl::constraint_definition(self, constraint);
        };
        if let Some(definition) = constraint.definition() {
            return format!(
                "CONSTRAINT {} {definition}",
                self.quote_identifier(&constraint.name())
            );
        }
        let mut text = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}",
            self.quote_identifier(&constraint.name()),
            self.identifier_list(key.columns()),
            self.quote_identifier(key.referenced_relation()),
        );
        if !key.referenced_columns().is_empty() {
            text.push_str(&format!(
                " ({})",
                self.identifier_list(key.referenced_columns())
            ));
        }
        if key.on_update() != Default::default() {
            text.push_str(&format!(" ON UPDATE {}", key.on_update().sql()));
        }
        if key.on_delete() != Default::default() {
            text.push_str(&format!(" ON DELETE {}", key.on_delete().sql()));
        }
        text
    }

    /// The index's statement. The index is named in the table's database;
    /// its table is named bare, as SQLite requires.
    fn create_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        if let Some(definition) = index.definition() {
            let definition = definition.trim().trim_end_matches(';');
            return if is_main(schema) {
                definition.to_string()
            } else {
                parse::qualify_created(definition, "INDEX", &self.quote_identifier(schema))
            };
        }
        let keys: Vec<String> = index
            .columns()
            .iter()
            .map(|key| {
                // An expression is kept as written; a name is quoted.
                if key.contains(['(', ' ']) {
                    key.to_string()
                } else {
                    self.quote_identifier(key)
                }
            })
            .collect();
        let mut sql = format!(
            "CREATE {}INDEX {} ON {} ({})",
            if index.is_unique() { "UNIQUE " } else { "" },
            self.qualified_name(schema, &index.name()),
            self.quote_identifier(relation),
            keys.join(", ")
        );
        if let Some(predicate) = index.predicate() {
            sql.push_str(&format!(" WHERE {predicate}"));
        }
        sql
    }

    fn create_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> Option<String> {
        let _ = relation;
        let definition = trigger.definition()?.trim().trim_end_matches(';');
        Some(if is_main(schema) {
            definition.to_string()
        } else {
            parse::qualify_created(definition, "TRIGGER", &self.quote_identifier(schema))
        })
    }

    fn drop_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> String {
        let _ = relation;
        format!(
            "DROP TRIGGER {}",
            self.qualified_name(schema, &trigger.name())
        )
    }

    /// The statements that turn `change.target()` into `change.source()`.
    ///
    /// Adding a column, dropping one nothing depends on, and replacing
    /// indexes and triggers are statements of their own. Anything else —
    /// changing a column, adding or dropping a constraint, a column
    /// `ALTER TABLE` cannot add or drop — rebuilds the table, following the
    /// procedure in SQLite's `ALTER TABLE` documentation.
    fn alter_relation(&self, schema: &str, change: &RelationChange) -> Vec<String> {
        if change.source().relation_type().is_view() {
            return ddl::alter_relation(self, schema, change);
        }
        if needs_rebuild(change) {
            return self.rebuild_table(schema, change.target(), change.source());
        }
        let name = change.source().name();
        let table = self.qualified_name(schema, &name);
        let mut statements = Vec::new();
        for trigger in change.removed_triggers() {
            statements.push(self.drop_trigger(schema, &name, trigger));
        }
        for index in change.removed_indexes() {
            if !index.is_primary() {
                statements.push(self.drop_index(schema, &name, index));
            }
        }
        for column in change.added_columns() {
            statements.push(format!(
                "ALTER TABLE {table} ADD COLUMN {}",
                self.column_definition(column)
            ));
        }
        for column in change.removed_columns() {
            statements.push(format!(
                "ALTER TABLE {table} DROP COLUMN {}",
                self.quote_identifier(&column.name())
            ));
        }
        for index in change.added_indexes() {
            if !index.is_primary() {
                statements.push(self.create_index(schema, &name, index));
            }
        }
        for trigger in change.added_triggers() {
            statements.extend(self.create_trigger(schema, &name, trigger));
        }
        statements
    }

    /// SQLite cannot change a column in place: its type, default and
    /// nullability are part of the table's statement. This writes nothing;
    /// [`Dialect::alter_relation`] rebuilds the table instead.
    fn alter_column(
        &self,
        schema: &str,
        relation: &str,
        old: &Column,
        new: &Column,
    ) -> Vec<String> {
        let _ = (schema, relation, old, new);
        Vec::new()
    }

    fn comment_on_relation(&self, schema: &str, relation: &Relation) -> Option<String> {
        let _ = (schema, relation);
        None
    }

    fn comment_on_column(&self, schema: &str, relation: &str, column: &Column) -> Option<String> {
        let _ = (schema, relation, column);
        None
    }

    /// Like the standard migration, except that foreign keys stay in the
    /// statement that creates their table: SQLite cannot add a constraint
    /// later, and does not require the referenced table to exist yet.
    fn migrate(&self, schema: &str, diff: &SchemaDiff) -> Vec<String> {
        let mut statements = Vec::new();
        let (views, tables): (Vec<&Relation>, Vec<&Relation>) = diff
            .added_relations()
            .iter()
            .partition(|relation| relation.relation_type().is_view());
        for relation in tables {
            statements.extend(self.create_relation(schema, relation));
        }
        for change in diff.changed_relations() {
            statements.extend(self.alter_relation(schema, change));
        }
        for relation in views {
            statements.extend(self.create_relation(schema, relation));
        }
        // Destructive changes last.
        let (views, tables): (Vec<&Relation>, Vec<&Relation>) = diff
            .removed_relations()
            .iter()
            .partition(|relation| relation.relation_type().is_view());
        for relation in views.into_iter().chain(tables) {
            statements.push(self.drop_relation(schema, relation));
        }
        statements
    }
}

impl SqliteDialect {
    /// The statement that creates the table `relation` as `name`, without
    /// its indexes and triggers.
    ///
    /// A single-column primary key on an `AUTOINCREMENT` column is declared
    /// on the column, the only place SQLite accepts `AUTOINCREMENT`. Columns
    /// marked as the key with no primary key constraint get an unnamed one.
    fn create_table(&self, schema: &str, name: &str, relation: &Relation) -> String {
        let key = relation
            .constraints()
            .iter()
            .find(|constraint| matches!(constraint.rule(), ConstraintRule::PrimaryKey { .. }));
        let inline_key = key.filter(|key| {
            let columns = key.rule().columns();
            columns.len() == 1
                && relation
                    .column(&columns[0])
                    .is_some_and(Column::is_auto_increment)
        });
        let mut lines: Vec<String> = relation
            .columns()
            .iter()
            .map(|column| {
                let mut definition = self.column_definition(column);
                if let Some(key) = inline_key
                    && *key.rule().columns()[0] == *column.name()
                {
                    definition.push_str(&format!(
                        " CONSTRAINT {} PRIMARY KEY AUTOINCREMENT",
                        self.quote_identifier(&key.name())
                    ));
                }
                format!("    {definition}")
            })
            .collect();
        if key.is_none() {
            let columns: Vec<_> = relation
                .columns()
                .iter()
                .filter(|column| column.is_primary_key())
                .map(|column| column.name())
                .collect();
            if !columns.is_empty() {
                lines.push(format!(
                    "    PRIMARY KEY ({})",
                    self.identifier_list(&columns)
                ));
            }
        }
        lines.extend(
            relation
                .constraints()
                .iter()
                .filter(|constraint| inline_key != Some(*constraint))
                .map(|constraint| format!("    {}", self.constraint_definition(constraint))),
        );
        format!(
            "CREATE TABLE {} (\n{}\n)",
            self.qualified_name(schema, name),
            lines.join(",\n")
        )
    }

    /// The statements that rebuild the table `target` as `source`, from the
    /// "Making Other Kinds Of Table Schema Changes" procedure of SQLite's
    /// `ALTER TABLE` documentation:
    ///
    /// 1. Turn foreign key enforcement off, which only works outside a
    ///    transaction, so run the statements outside one.
    /// 2. In a savepoint, create the new table as `new_<name>`, copy every
    ///    column the two versions share, drop the old table and rename the
    ///    new one. Renaming runs with `legacy_alter_table` on, so views that
    ///    name the table are not checked while it is missing.
    /// 3. Create the indexes and triggers again, then release the savepoint
    ///    and turn foreign key enforcement back on, as DataKit sessions have
    ///    it.
    ///
    /// Views that read the table keep working when the columns they read
    /// still exist.
    fn rebuild_table(&self, schema: &str, target: &Relation, source: &Relation) -> Vec<String> {
        let name = source.name();
        let temporary = format!("new_{name}");
        let old_table = self.qualified_name(schema, &target.name());
        let new_table = self.qualified_name(schema, &temporary);
        let copied: Vec<_> = source
            .columns()
            .iter()
            .filter(|column| !column.is_generated())
            .filter(|column| {
                target
                    .column(&column.name())
                    .is_some_and(|old| !old.is_generated())
            })
            .map(|column| column.name())
            .collect();

        let mut statements = vec![
            "PRAGMA foreign_keys = OFF".to_string(),
            "SAVEPOINT datakit_rebuild".to_string(),
            self.create_table(schema, &temporary, source),
        ];
        if !copied.is_empty() {
            let columns = self.identifier_list(&copied);
            statements.push(format!(
                "INSERT INTO {new_table} ({columns}) SELECT {columns} FROM {old_table}"
            ));
        }
        statements.extend([
            format!("DROP TABLE {old_table}"),
            "PRAGMA legacy_alter_table = ON".to_string(),
            format!(
                "ALTER TABLE {new_table} RENAME TO {}",
                self.quote_identifier(&name)
            ),
            "PRAGMA legacy_alter_table = OFF".to_string(),
        ]);
        statements.extend(
            ddl::standalone_indexes(source).map(|index| self.create_index(schema, &name, index)),
        );
        statements.extend(
            source
                .triggers()
                .iter()
                .filter_map(|trigger| self.create_trigger(schema, &name, trigger)),
        );
        statements.extend([
            "RELEASE datakit_rebuild".to_string(),
            "PRAGMA foreign_keys = ON".to_string(),
        ]);
        statements
    }

    fn identifier_list(&self, columns: &[std::sync::Arc<str>]) -> String {
        columns
            .iter()
            .map(|column| self.quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The trait's own literal, for the values SQLite writes the standard way.
fn ddl_literal(dialect: &SqliteDialect, value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(true) => "TRUE".into(),
        Value::Bool(false) => "FALSE".into(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Text(text) => dialect.string_literal(text),
    }
}

/// Whether `schema` is the one an unqualified name means.
fn is_main(schema: &str) -> bool {
    schema.is_empty() || schema.eq_ignore_ascii_case("main")
}

/// `default` as a `DEFAULT` clause takes it: a literal as it is, anything
/// else in parentheses. SQLite reports an expression default without the
/// parentheses it was declared with.
fn default_expression(default: &str) -> String {
    let trimmed = default.trim();
    let upper = trimmed.to_ascii_uppercase();
    let number = trimmed
        .trim_start_matches(['+', '-'])
        .parse::<f64>()
        .is_ok();
    let literal = number
        || matches!(
            upper.as_str(),
            "NULL" | "TRUE" | "FALSE" | "CURRENT_TIME" | "CURRENT_DATE" | "CURRENT_TIMESTAMP"
        )
        || is_quoted(trimmed, '\'')
        || (upper.starts_with('X') && is_quoted(&trimmed[1..], '\''))
        || is_parenthesized(trimmed);
    if literal {
        trimmed.to_string()
    } else {
        format!("({trimmed})")
    }
}

/// Whether `text` is one string quoted with `quote`, not two adjacent ones.
fn is_quoted(text: &str, quote: char) -> bool {
    let Some(inner) = text
        .strip_prefix(quote)
        .and_then(|rest| rest.strip_suffix(quote))
    else {
        return false;
    };
    !inner
        .replace(&format!("{quote}{quote}"), "")
        .contains(quote)
}

/// Whether `text` is one parenthesized expression, `(a) + (b)` not being one.
fn is_parenthesized(text: &str) -> bool {
    if !text.starts_with('(') || !text.ends_with(')') {
        return false;
    }
    let mut depth = 0i32;
    for (ix, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 && ix != text.len() - 1 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// Whether the change needs the table rebuilt: SQLite's `ALTER TABLE` can
/// only add a column, drop one, and rename.
fn needs_rebuild(change: &RelationChange) -> bool {
    let column_changed = change.changed_columns().iter().any(|(old, new)| {
        old.data_type() != new.data_type()
            || old.is_nullable() != new.is_nullable()
            || old.default() != new.default()
            || old.is_primary_key() != new.is_primary_key()
            || old.is_generated() != new.is_generated()
            || old.is_auto_increment() != new.is_auto_increment()
    });
    let target = change.target();
    column_changed
        || !change.added_constraints().is_empty()
        || !change.removed_constraints().is_empty()
        || !change.added_columns().iter().all(is_addable)
        || !change
            .removed_columns()
            .iter()
            .all(|column| is_droppable(target, column))
}

/// Whether `ALTER TABLE … ADD COLUMN` can add `column`: not part of the key,
/// with a constant default, and with a default other than `NULL` when it is
/// `NOT NULL`.
fn is_addable(column: &Column) -> bool {
    if column.is_primary_key() || column.is_auto_increment() {
        return false;
    }
    if column.is_generated() {
        return true;
    }
    let default = column.default().map(default_expression);
    let constant = default.as_deref().is_none_or(|default| {
        !default.starts_with('(') && !default.to_ascii_uppercase().starts_with("CURRENT_")
    });
    let null_default = default
        .as_deref()
        .is_none_or(|default| default.eq_ignore_ascii_case("NULL"));
    constant && (column.is_nullable() || !null_default)
}

/// Whether `ALTER TABLE … DROP COLUMN` can drop `column` from `table`: no
/// key, constraint or index uses it.
fn is_droppable(table: &Relation, column: &Column) -> bool {
    let name = column.name();
    let named = |columns: &[std::sync::Arc<str>]| {
        columns
            .iter()
            .any(|other| other.eq_ignore_ascii_case(&name) || other.contains(&*name))
    };
    !column.is_primary_key()
        && !table
            .constraints()
            .iter()
            .any(|constraint| match constraint.rule() {
                ConstraintRule::Check { expression } => expression.contains(&*name),
                rule => named(rule.columns()),
            })
        && !table.indexes().iter().any(|index| named(index.columns()))
        && table.relation_type() == RelationType::Table
}

/// Types offered when a column is created or changed, most common first.
/// SQLite stores values by affinity; these are the names that give each
/// affinity, and the conventional names for what SQLite has no type for.
const DATA_TYPES: &[&str] = &[
    "INTEGER",
    "TEXT",
    "REAL",
    "NUMERIC",
    "BLOB",
    "BOOLEAN",
    "DATE",
    "DATETIME",
    "VARCHAR(255)",
    "DECIMAL(10,2)",
];

/// Every SQLite keyword, from the "SQL Keywords" page of SQLite's
/// documentation, and the literals `TRUE` and `FALSE`. SQLite accepts many
/// of them as names, but quoting every one is always right.
const RESERVED: &[&str] = &[
    "ABORT",
    "ACTION",
    "ADD",
    "AFTER",
    "ALL",
    "ALTER",
    "ALWAYS",
    "ANALYZE",
    "AND",
    "AS",
    "ASC",
    "ATTACH",
    "AUTOINCREMENT",
    "BEFORE",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASCADE",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "COMMIT",
    "CONFLICT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "DATABASE",
    "DEFAULT",
    "DEFERRABLE",
    "DEFERRED",
    "DELETE",
    "DESC",
    "DETACH",
    "DISTINCT",
    "DO",
    "DROP",
    "EACH",
    "ELSE",
    "END",
    "ESCAPE",
    "EXCEPT",
    "EXCLUDE",
    "EXCLUSIVE",
    "EXISTS",
    "EXPLAIN",
    "FAIL",
    "FALSE",
    "FILTER",
    "FIRST",
    "FOLLOWING",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "GENERATED",
    "GLOB",
    "GROUP",
    "GROUPS",
    "HAVING",
    "IF",
    "IGNORE",
    "IMMEDIATE",
    "IN",
    "INDEX",
    "INDEXED",
    "INITIALLY",
    "INNER",
    "INSERT",
    "INSTEAD",
    "INTERSECT",
    "INTO",
    "IS",
    "ISNULL",
    "JOIN",
    "KEY",
    "LAST",
    "LEFT",
    "LIKE",
    "LIMIT",
    "MATCH",
    "MATERIALIZED",
    "NATURAL",
    "NO",
    "NOT",
    "NOTHING",
    "NOTNULL",
    "NULL",
    "NULLS",
    "OF",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OTHERS",
    "OUTER",
    "OVER",
    "PARTITION",
    "PLAN",
    "PRAGMA",
    "PRECEDING",
    "PRIMARY",
    "QUERY",
    "RAISE",
    "RANGE",
    "RECURSIVE",
    "REFERENCES",
    "REGEXP",
    "REINDEX",
    "RELEASE",
    "RENAME",
    "REPLACE",
    "RESTRICT",
    "RETURNING",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SAVEPOINT",
    "SELECT",
    "SET",
    "TABLE",
    "TEMP",
    "TEMPORARY",
    "THEN",
    "TIES",
    "TO",
    "TRANSACTION",
    "TRIGGER",
    "TRUE",
    "UNBOUNDED",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VACUUM",
    "VALUES",
    "VIEW",
    "VIRTUAL",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
    "WITHOUT",
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
    "FULL JOIN",
    "CROSS JOIN",
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
    "IS DISTINCT FROM",
    "LIKE",
    "GLOB",
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
    "INSERT OR REPLACE INTO",
    "INSERT OR IGNORE INTO",
    "REPLACE INTO",
    "VALUES",
    "DEFAULT VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "RETURNING",
    "ON CONFLICT",
    "DO NOTHING",
    "DO UPDATE",
    "CREATE TABLE",
    "CREATE TABLE IF NOT EXISTS",
    "CREATE VIEW",
    "CREATE INDEX",
    "CREATE UNIQUE INDEX",
    "CREATE TRIGGER",
    "CREATE VIRTUAL TABLE",
    "CREATE TEMP TABLE",
    "ALTER TABLE",
    "ADD COLUMN",
    "DROP COLUMN",
    "RENAME TO",
    "RENAME COLUMN",
    "DROP TABLE",
    "DROP VIEW",
    "DROP INDEX",
    "DROP TRIGGER",
    "IF EXISTS",
    "PRIMARY KEY",
    "AUTOINCREMENT",
    "FOREIGN KEY",
    "REFERENCES",
    "UNIQUE",
    "CHECK",
    "DEFAULT",
    "NOT NULL",
    "COLLATE NOCASE",
    "GENERATED ALWAYS AS",
    "WITHOUT ROWID",
    "STRICT",
    "BEGIN",
    "BEGIN IMMEDIATE",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "RELEASE",
    "EXPLAIN",
    "EXPLAIN QUERY PLAN",
    "ANALYZE",
    "VACUUM",
    "REINDEX",
    "PRAGMA",
    "ATTACH DATABASE",
    "DETACH DATABASE",
    "NULL",
    "TRUE",
    "FALSE",
    "ASC",
    "DESC",
    "NULLS FIRST",
    "NULLS LAST",
    "OVER",
    "PARTITION BY",
    "WINDOW",
    "FILTER",
    "CAST",
];

/// Built-in functions offered by completion: the core, aggregate, window,
/// date and JSON functions every SQLite build has.
const FUNCTIONS: &[&str] = &[
    "count",
    "sum",
    "total",
    "avg",
    "min",
    "max",
    "group_concat",
    "string_agg",
    "coalesce",
    "ifnull",
    "iif",
    "nullif",
    "length",
    "octet_length",
    "lower",
    "upper",
    "trim",
    "ltrim",
    "rtrim",
    "substr",
    "substring",
    "replace",
    "instr",
    "concat",
    "concat_ws",
    "printf",
    "format",
    "quote",
    "hex",
    "unhex",
    "char",
    "unicode",
    "abs",
    "round",
    "sign",
    "random",
    "randomblob",
    "zeroblob",
    "typeof",
    "like",
    "glob",
    "likelihood",
    "changes",
    "total_changes",
    "last_insert_rowid",
    "sqlite_version",
    "date",
    "time",
    "datetime",
    "julianday",
    "unixepoch",
    "strftime",
    "timediff",
    "row_number",
    "rank",
    "dense_rank",
    "percent_rank",
    "cume_dist",
    "ntile",
    "lag",
    "lead",
    "first_value",
    "last_value",
    "nth_value",
    "json",
    "json_array",
    "json_object",
    "json_extract",
    "json_insert",
    "json_replace",
    "json_set",
    "json_remove",
    "json_patch",
    "json_type",
    "json_valid",
    "json_array_length",
    "json_group_array",
    "json_group_object",
    "json_each",
    "json_tree",
];

#[cfg(test)]
mod tests {
    use datakit_catalog::{ForeignKey, ReferentialAction, Schema, diff_schemas};

    use super::*;

    fn orders() -> Relation {
        Relation::new("orders", RelationType::Table)
            .with_columns([
                Column::new("id", "INTEGER").auto_increment(true),
                Column::new("customer_id", "INTEGER"),
                Column::new("placed_at", "TEXT").with_default("datetime('now')"),
                Column::new("note", "TEXT")
                    .nullable(true)
                    .with_comment("ignored"),
            ])
            .with_constraints([
                Constraint::new(
                    "orders_pkey",
                    ConstraintRule::PrimaryKey {
                        columns: vec!["id".into()].into(),
                    },
                ),
                Constraint::new(
                    "orders_customer_id_fkey",
                    ConstraintRule::ForeignKey(
                        ForeignKey::new(["customer_id"], "main", "customers", ["id"])
                            .with_on_delete(ReferentialAction::Cascade),
                    ),
                ),
            ])
            .with_indexes([
                Index::new("orders_pkey", ["id"]).unique(true).primary(true),
                Index::new("orders_customer", ["customer_id"]),
            ])
    }

    #[test]
    fn names_are_quoted_only_when_sqlite_needs_it() {
        let dialect = SqliteDialect;
        assert_eq!(dialect.quote_identifier("orders"), "orders");
        assert_eq!(dialect.quote_identifier("Orders"), "Orders");
        assert_eq!(dialect.quote_identifier("order"), "\"order\"");
        assert_eq!(dialect.quote_identifier("key"), "\"key\"");
        assert_eq!(dialect.quote_identifier("line items"), "\"line items\"");
        assert_eq!(dialect.quote_identifier("say\"hi"), "\"say\"\"hi\"");
        assert_eq!(dialect.quote_identifier("2fa"), "\"2fa\"");
        assert_eq!(dialect.fold_identifier("Orders"), "Orders");
    }

    #[test]
    fn main_is_never_written_and_other_databases_are() {
        let dialect = SqliteDialect;
        assert_eq!(
            dialect.select_rows("main", "Orders", Some(10)),
            "SELECT * FROM Orders LIMIT 10"
        );
        assert_eq!(dialect.qualified_name("aux", "t"), "aux.t");
        assert_eq!(
            dialect.create_index("aux", "t", &Index::new("t_a", ["a"])),
            "CREATE INDEX aux.t_a ON t (a)"
        );
        assert_eq!(
            dialect.create_index(
                "aux",
                "t",
                &Index::new("t_a", ["a"]).with_definition("CREATE INDEX t_a ON t (a)")
            ),
            "CREATE INDEX aux.t_a ON t (a)"
        );
    }

    #[test]
    fn literals_keep_blobs_and_avoid_what_sqlite_cannot_read() {
        let dialect = SqliteDialect;
        assert_eq!(dialect.literal(&Value::Text("X'0A0B'".into())), "X'0A0B'");
        assert_eq!(dialect.literal(&Value::Text("it's".into())), "'it''s'");
        assert_eq!(dialect.literal(&Value::Float(f64::INFINITY)), "9e999");
        assert_eq!(dialect.literal(&Value::Float(f64::NAN)), "NULL");
        assert_eq!(dialect.literal(&Value::Bool(true)), "TRUE");
    }

    #[test]
    fn a_table_declares_autoincrement_on_its_key_column() {
        let statements = SqliteDialect.create_relation("main", &orders());
        assert_eq!(
            statements,
            vec![
                "CREATE TABLE orders (\n    \
                 id INTEGER NOT NULL CONSTRAINT orders_pkey PRIMARY KEY AUTOINCREMENT,\n    \
                 customer_id INTEGER NOT NULL,\n    \
                 placed_at TEXT DEFAULT (datetime('now')) NOT NULL,\n    \
                 note TEXT,\n    \
                 CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers (id) ON DELETE CASCADE\n)",
                "CREATE INDEX orders_customer ON orders (customer_id)",
            ]
        );
    }

    #[test]
    fn defaults_are_parenthesized_unless_literal() {
        assert_eq!(default_expression("0"), "0");
        assert_eq!(default_expression("-1.5"), "-1.5");
        assert_eq!(default_expression("'a''b'"), "'a''b'");
        assert_eq!(default_expression("X'00'"), "X'00'");
        assert_eq!(default_expression("CURRENT_TIMESTAMP"), "CURRENT_TIMESTAMP");
        assert_eq!(default_expression("(1 + 2)"), "(1 + 2)");
        assert_eq!(default_expression("(1) + (2)"), "((1) + (2))");
        assert_eq!(default_expression("'a' || 'b'"), "('a' || 'b')");
    }

    #[test]
    fn adding_a_nullable_column_alters_the_table() {
        let mut columns = orders().columns().to_vec();
        columns.push(Column::new("total", "REAL").nullable(true));
        let source = orders().with_columns(columns);
        let change = RelationChange::between(&orders(), &source).unwrap();
        assert_eq!(
            SqliteDialect.alter_relation("main", &change),
            vec!["ALTER TABLE orders ADD COLUMN total REAL"]
        );
    }

    #[test]
    fn changing_a_column_rebuilds_the_table() {
        let target =
            orders().with_triggers([Trigger::new("orders_touch", "AFTER UPDATE FOR EACH ROW")
                .with_definition(
                    "CREATE TRIGGER orders_touch AFTER UPDATE ON orders BEGIN SELECT 1; END",
                )]);
        let mut columns = target.columns().to_vec();
        columns[3] = columns[3].clone().nullable(false).with_default("''");
        columns.push(Column::new("total", "REAL").with_default("0"));
        let source = target.clone().with_columns(columns);
        let diff = diff_schemas(
            &Schema::new("main").with_relations([target]),
            &Schema::new("main").with_relations([source]),
        );
        let statements = SqliteDialect.migrate("main", &diff);
        assert_eq!(
            statements,
            vec![
                "PRAGMA foreign_keys = OFF".to_string(),
                "SAVEPOINT datakit_rebuild".into(),
                "CREATE TABLE new_orders (\n    \
                 id INTEGER NOT NULL CONSTRAINT orders_pkey PRIMARY KEY AUTOINCREMENT,\n    \
                 customer_id INTEGER NOT NULL,\n    \
                 placed_at TEXT DEFAULT (datetime('now')) NOT NULL,\n    \
                 note TEXT DEFAULT '' NOT NULL,\n    \
                 total REAL DEFAULT 0 NOT NULL,\n    \
                 CONSTRAINT orders_customer_id_fkey FOREIGN KEY (customer_id) REFERENCES customers (id) ON DELETE CASCADE\n)"
                    .into(),
                "INSERT INTO new_orders (id, customer_id, placed_at, note) \
                 SELECT id, customer_id, placed_at, note FROM orders"
                    .into(),
                "DROP TABLE orders".into(),
                "PRAGMA legacy_alter_table = ON".into(),
                "ALTER TABLE new_orders RENAME TO orders".into(),
                "PRAGMA legacy_alter_table = OFF".into(),
                "CREATE INDEX orders_customer ON orders (customer_id)".into(),
                "CREATE TRIGGER orders_touch AFTER UPDATE ON orders BEGIN SELECT 1; END".into(),
                "RELEASE datakit_rebuild".into(),
                "PRAGMA foreign_keys = ON".into(),
            ]
        );
    }

    #[test]
    fn a_column_nothing_uses_is_dropped_in_place() {
        let mut columns = orders().columns().to_vec();
        columns.retain(|column| &*column.name() != "note");
        let source = orders().with_columns(columns);
        let change = RelationChange::between(&orders(), &source).unwrap();
        assert_eq!(
            SqliteDialect.alter_relation("main", &change),
            vec!["ALTER TABLE orders DROP COLUMN note"]
        );

        let mut columns = orders().columns().to_vec();
        columns.retain(|column| &*column.name() != "customer_id");
        let source = orders()
            .with_columns(columns)
            .with_constraints(orders().constraints()[..1].to_vec())
            .with_indexes(orders().indexes()[..1].to_vec());
        let change = RelationChange::between(&orders(), &source).unwrap();
        assert_eq!(
            SqliteDialect.alter_relation("main", &change)[0],
            "PRAGMA foreign_keys = OFF",
            "a column a key uses needs a rebuild"
        );
    }

    #[test]
    fn plans_are_explained_and_comments_are_not_written() {
        let dialect = SqliteDialect;
        assert_eq!(
            dialect.explain("SELECT 1", true).as_deref(),
            Some("EXPLAIN QUERY PLAN SELECT 1")
        );
        assert_eq!(dialect.comment_on_relation("main", &orders()), None);
        assert_eq!(
            dialect.drop_trigger("main", "orders", &Trigger::new("t", "")),
            "DROP TRIGGER t"
        );
        assert_eq!(
            dialect.drop_index("main", "orders", &Index::new("i", ["a"])),
            "DROP INDEX i"
        );
    }
}
