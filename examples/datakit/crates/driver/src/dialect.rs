use datakit_catalog::{
    Column, Constraint, Index, Relation, RelationChange, Routine, SchemaDiff, Sequence, Trigger,
};

use crate::{PlanNode, RowChange, Value, ddl};

/// What differs between databases in syntax, as data the rest of DataKit
/// reads. A dialect never does IO.
///
/// The provided methods write standard SQL, which PostgreSQL follows
/// closely; a dialect overrides what its database spells another way.
pub trait Dialect: Send + Sync {
    /// Words the database reserves; an identifier spelled like one must be
    /// quoted.
    fn reserved_words(&self) -> &'static [&'static str];

    /// Keywords offered by completion, in upper case.
    fn keywords(&self) -> &'static [&'static str];

    /// Built-in functions offered by completion.
    fn functions(&self) -> &'static [&'static str];

    /// Column types offered when a column is created or changed, most
    /// common first.
    fn data_types(&self) -> &'static [&'static str] {
        &[
            "integer",
            "bigint",
            "smallint",
            "numeric",
            "real",
            "double precision",
            "boolean",
            "text",
            "varchar(255)",
            "char(1)",
            "date",
            "time",
            "timestamp",
        ]
    }

    /// The character that quotes an identifier.
    fn identifier_quote(&self) -> char {
        '"'
    }

    /// The name an unquoted identifier refers to. PostgreSQL folds it to
    /// lower case.
    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_lowercase()
    }

    /// Whether the database has more than one schema per database, so that
    /// names are qualified with one.
    fn has_schemas(&self) -> bool {
        true
    }

    /// `identifier`, quoted only when it has to be: when folding would change
    /// it, when it is not a plain word, or when it is reserved.
    fn quote_identifier(&self, identifier: &str) -> String {
        let plain = identifier
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
            && identifier
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$')
            && self.fold_identifier(identifier) == identifier;
        let reserved = self
            .reserved_words()
            .iter()
            .any(|word| word.eq_ignore_ascii_case(identifier));
        if plain && !reserved {
            return identifier.to_string();
        }
        let quote = self.identifier_quote();
        let close = match quote {
            '[' => ']',
            quote => quote,
        };
        let escaped = identifier.replace(close, &format!("{close}{close}"));
        format!("{quote}{escaped}{close}")
    }

    /// `schema.relation`, each part quoted as needed; just the relation for
    /// a database without schemas.
    fn qualified_name(&self, schema: &str, relation: &str) -> String {
        if !self.has_schemas() || schema.is_empty() {
            return self.quote_identifier(relation);
        }
        format!(
            "{}.{}",
            self.quote_identifier(schema),
            self.quote_identifier(relation)
        )
    }

    /// `text` as a string literal.
    fn string_literal(&self, text: &str) -> String {
        format!("'{}'", text.replace('\'', "''"))
    }

    /// `value` as a literal. Text that is not a number is quoted, which the
    /// database converts to the column's type where one is expected.
    fn literal(&self, value: &Value) -> String {
        match value {
            Value::Null => "NULL".into(),
            Value::Bool(true) => "TRUE".into(),
            Value::Bool(false) => "FALSE".into(),
            Value::Int(value) => value.to_string(),
            Value::Float(value) if value.is_finite() => value.to_string(),
            Value::Float(value) => self.string_literal(&value.to_string()),
            Value::Text(text) => self.string_literal(text),
        }
    }

    /// A statement that selects every column of a relation, at most `limit`
    /// rows when given.
    fn select_rows(&self, schema: &str, relation: &str, limit: Option<u64>) -> String {
        self.select_rows_where(schema, relation, "", "", limit)
    }

    /// Like [`Self::select_rows`], with a `WHERE` condition and an
    /// `ORDER BY` list when they are not empty.
    fn select_rows_where(
        &self,
        schema: &str,
        relation: &str,
        condition: &str,
        order_by: &str,
        limit: Option<u64>,
    ) -> String {
        let mut sql = format!("SELECT * FROM {}", self.qualified_name(schema, relation));
        if !condition.trim().is_empty() {
            sql.push_str(&format!(" WHERE {}", condition.trim()));
        }
        if !order_by.trim().is_empty() {
            sql.push_str(&format!(" ORDER BY {}", order_by.trim()));
        }
        if let Some(limit) = limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }
        sql
    }

    /// One page of [`Self::select_rows_where`]: at most `limit` rows after
    /// skipping `offset`.
    fn select_page(
        &self,
        schema: &str,
        relation: &str,
        condition: &str,
        order_by: &str,
        limit: u64,
        offset: u64,
    ) -> String {
        let mut sql = self.select_rows_where(schema, relation, condition, order_by, Some(limit));
        if offset > 0 {
            sql.push_str(&format!(" OFFSET {offset}"));
        }
        sql
    }

    /// A statement that counts the rows of a relation.
    fn count_rows(&self, schema: &str, relation: &str, condition: &str) -> String {
        let mut sql = format!(
            "SELECT count(*) FROM {}",
            self.qualified_name(schema, relation)
        );
        if !condition.trim().is_empty() {
            sql.push_str(&format!(" WHERE {}", condition.trim()));
        }
        sql
    }

    /// Whether the database speaks commands, one a line, rather than SQL,
    /// as Redis does. The console then runs the line at the caret, and
    /// leaves out what only SQL has: parameters, inspections, formatting.
    fn statements_are_lines(&self) -> bool {
        false
    }

    /// Whether `statement` only reads, for a database whose language is not
    /// SQL; `None` leaves the question to the SQL rules.
    fn reads_only(&self, statement: &str) -> Option<bool> {
        let _ = statement;
        None
    }

    /// Whether statements can be grouped into a transaction.
    fn supports_transactions(&self) -> bool {
        true
    }

    /// The statements that start, commit and roll back a transaction.
    fn begin_transaction(&self) -> &'static str {
        "BEGIN"
    }

    fn commit(&self) -> &'static str {
        "COMMIT"
    }

    fn rollback(&self) -> &'static str {
        "ROLLBACK"
    }

    /// The statement that applies one change to a row.
    fn row_change(&self, schema: &str, relation: &str, change: &RowChange) -> String {
        ddl::row_change(self, schema, relation, change)
    }

    /// One statement inserting `rows`, each a value per column of
    /// `columns`.
    fn insert_rows(
        &self,
        schema: &str,
        relation: &str,
        columns: &[&str],
        rows: &[Vec<Value>],
    ) -> String {
        let columns: Vec<String> = columns
            .iter()
            .map(|column| self.quote_identifier(column))
            .collect();
        let rows: Vec<String> = rows
            .iter()
            .map(|row| {
                let values: Vec<String> = row.iter().map(|value| self.literal(value)).collect();
                format!("({})", values.join(", "))
            })
            .collect();
        format!(
            "INSERT INTO {} ({}) VALUES\n{}",
            self.qualified_name(schema, relation),
            columns.join(", "),
            rows.join(",\n")
        )
    }

    /// The most rows [`Self::insert_rows`] should insert at once.
    fn insert_batch_size(&self) -> usize {
        500
    }

    /// The statement that asks for `sql`'s execution plan, actually running
    /// it when `analyze` is set; `None` when the database cannot explain.
    fn explain(&self, sql: &str, analyze: bool) -> Option<String> {
        let _ = (sql, analyze);
        None
    }

    /// The plan in the rows an [`Self::explain`] statement returned.
    fn parse_plan(&self, rows: &[Vec<Value>]) -> anyhow::Result<PlanNode> {
        let _ = rows;
        anyhow::bail!("This database cannot explain statements")
    }

    // Data definition. Every statement is complete and ends without a
    // semicolon; the caller joins them.

    /// The statements that create `relation` in `schema` with its comments,
    /// indexes and triggers.
    fn create_relation(&self, schema: &str, relation: &Relation) -> Vec<String> {
        ddl::create_relation(self, schema, relation)
    }

    /// The column as it appears in `CREATE TABLE`: name, type, default,
    /// nullability.
    fn column_definition(&self, column: &Column) -> String {
        ddl::column_definition(self, column)
    }

    /// The constraint as it appears in `CREATE TABLE`.
    fn constraint_definition(&self, constraint: &Constraint) -> String {
        ddl::constraint_definition(self, constraint)
    }

    fn create_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        ddl::create_index(self, schema, relation, index)
    }

    fn create_sequence(&self, schema: &str, sequence: &Sequence) -> String {
        ddl::create_sequence(self, schema, sequence)
    }

    /// The statement that creates `routine`: its definition, which the
    /// database keeps whole.
    fn create_routine(&self, schema: &str, routine: &Routine) -> Option<String> {
        let _ = schema;
        routine
            .definition()
            .map(|definition| definition.trim().trim_end_matches(';').to_string())
    }

    fn create_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> Option<String> {
        let _ = (schema, relation);
        trigger
            .definition()
            .map(|definition| definition.trim().trim_end_matches(';').to_string())
    }

    fn drop_relation(&self, schema: &str, relation: &Relation) -> String {
        let object = match relation.relation_type() {
            datakit_catalog::RelationType::View => "VIEW",
            datakit_catalog::RelationType::MaterializedView => "MATERIALIZED VIEW",
            datakit_catalog::RelationType::ForeignTable => "FOREIGN TABLE",
            _ => "TABLE",
        };
        format!(
            "DROP {object} {}",
            self.qualified_name(schema, &relation.name())
        )
    }

    fn drop_index(&self, schema: &str, relation: &str, index: &Index) -> String {
        let _ = relation;
        format!("DROP INDEX {}", self.qualified_name(schema, &index.name()))
    }

    fn drop_sequence(&self, schema: &str, sequence: &Sequence) -> String {
        format!(
            "DROP SEQUENCE {}",
            self.qualified_name(schema, &sequence.name())
        )
    }

    fn drop_routine(&self, schema: &str, routine: &Routine) -> String {
        let object = match routine.routine_type() {
            datakit_catalog::RoutineType::Procedure => "PROCEDURE",
            datakit_catalog::RoutineType::Aggregate => "AGGREGATE",
            _ => "FUNCTION",
        };
        format!(
            "DROP {object} {}({})",
            self.qualified_name(schema, &routine.name()),
            routine.arguments()
        )
    }

    fn drop_trigger(&self, schema: &str, relation: &str, trigger: &Trigger) -> String {
        format!(
            "DROP TRIGGER {} ON {}",
            self.quote_identifier(&trigger.name()),
            self.qualified_name(schema, relation)
        )
    }

    /// The statement that adds `column` to `relation`.
    fn add_column(&self, schema: &str, relation: &str, column: &Column) -> String {
        format!(
            "ALTER TABLE {} ADD COLUMN {}",
            self.qualified_name(schema, relation),
            self.column_definition(column)
        )
    }

    /// The statements that turn `change.target()` into `change.source()`.
    fn alter_relation(&self, schema: &str, change: &RelationChange) -> Vec<String> {
        ddl::alter_relation(self, schema, change)
    }

    /// The statements that change `old` into `new`, a column of `relation`
    /// with the same name.
    fn alter_column(
        &self,
        schema: &str,
        relation: &str,
        old: &Column,
        new: &Column,
    ) -> Vec<String> {
        ddl::alter_column(self, schema, relation, old, new)
    }

    fn rename_column(&self, schema: &str, relation: &str, old: &str, new: &str) -> String {
        format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            self.qualified_name(schema, relation),
            self.quote_identifier(old),
            self.quote_identifier(new)
        )
    }

    fn rename_relation(&self, schema: &str, relation: &Relation, new: &str) -> String {
        let object = if relation.relation_type().is_view() {
            "VIEW"
        } else {
            "TABLE"
        };
        format!(
            "ALTER {object} {} RENAME TO {}",
            self.qualified_name(schema, &relation.name()),
            self.quote_identifier(new)
        )
    }

    /// The statement that sets or clears a relation's comment; `None` when
    /// the database has no such statement.
    fn comment_on_relation(&self, schema: &str, relation: &Relation) -> Option<String> {
        let object = match relation.relation_type() {
            datakit_catalog::RelationType::View => "VIEW",
            datakit_catalog::RelationType::MaterializedView => "MATERIALIZED VIEW",
            _ => "TABLE",
        };
        Some(format!(
            "COMMENT ON {object} {} IS {}",
            self.qualified_name(schema, &relation.name()),
            relation
                .comment()
                .map_or("NULL".into(), |comment| self.string_literal(comment))
        ))
    }

    fn comment_on_column(&self, schema: &str, relation: &str, column: &Column) -> Option<String> {
        Some(format!(
            "COMMENT ON COLUMN {}.{} IS {}",
            self.qualified_name(schema, relation),
            self.quote_identifier(&column.name()),
            column
                .comment()
                .map_or("NULL".into(), |comment| self.string_literal(comment))
        ))
    }

    /// The statements that make the target of `diff`, a schema called
    /// `schema`, look like its source. Destructive statements come last, so
    /// a script stopped partway loses nothing.
    fn migrate(&self, schema: &str, diff: &SchemaDiff) -> Vec<String> {
        ddl::migrate(self, schema, diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Plain;

    impl Dialect for Plain {
        fn reserved_words(&self) -> &'static [&'static str] {
            &["SELECT", "USER", "ORDER"]
        }

        fn keywords(&self) -> &'static [&'static str] {
            &[]
        }

        fn functions(&self) -> &'static [&'static str] {
            &[]
        }
    }

    #[test]
    fn identifiers_are_quoted_only_when_needed() {
        assert_eq!(Plain.quote_identifier("orders"), "orders");
        assert_eq!(Plain.quote_identifier("order"), "\"order\"");
        assert_eq!(Plain.quote_identifier("Orders"), "\"Orders\"");
        assert_eq!(Plain.quote_identifier("line items"), "\"line items\"");
        assert_eq!(Plain.quote_identifier("say\"hi"), "\"say\"\"hi\"");
        assert_eq!(Plain.quote_identifier("2fa"), "\"2fa\"");
    }

    #[test]
    fn selecting_rows_quotes_both_parts() {
        assert_eq!(
            Plain.select_rows("public", "user", Some(500)),
            "SELECT * FROM public.\"user\" LIMIT 500"
        );
        assert_eq!(
            Plain.select_rows_where("s", "t", " a > 1 ", "b DESC", None),
            "SELECT * FROM s.t WHERE a > 1 ORDER BY b DESC"
        );
    }

    #[test]
    fn literals_quote_text_and_keep_numbers() {
        assert_eq!(Plain.literal(&Value::Null), "NULL");
        assert_eq!(Plain.literal(&Value::Int(4)), "4");
        assert_eq!(Plain.literal(&Value::Text("it's".into())), "'it''s'");
        assert_eq!(Plain.literal(&Value::Float(f64::NAN)), "'NaN'");
    }
}
