use datakit_catalog::Relation;
use datakit_driver::{Dialect, Value};

use crate::command::{COMMANDS, quote, reads_only};

/// Redis's commands, as DataKit's SQL-shaped questions ask for them.
///
/// A schema is a database, `db0` upward, and a relation a key. Reading a
/// key is `DATAKIT.VALUE`, which the connection answers by the key's type.
pub struct RedisDialect;

impl Dialect for RedisDialect {
    fn reserved_words(&self) -> &'static [&'static str] {
        &[]
    }

    fn keywords(&self) -> &'static [&'static str] {
        COMMANDS
    }

    fn functions(&self) -> &'static [&'static str] {
        &[]
    }

    fn identifier_quote(&self) -> char {
        '"'
    }

    /// Keys are case-sensitive.
    fn fold_identifier(&self, identifier: &str) -> String {
        identifier.to_string()
    }

    fn quote_identifier(&self, identifier: &str) -> String {
        quote(identifier)
    }

    /// A key names itself; the database is chosen by the session.
    fn qualified_name(&self, _: &str, relation: &str) -> String {
        quote(relation)
    }

    fn string_literal(&self, text: &str) -> String {
        quote(text)
    }

    fn literal(&self, value: &Value) -> String {
        match value.display() {
            Some(text) => quote(&text),
            None => "\"\"".to_string(),
        }
    }

    fn select_rows_where(
        &self,
        schema: &str,
        relation: &str,
        _: &str,
        _: &str,
        limit: Option<u64>,
    ) -> String {
        self.select_page(schema, relation, "", "", limit.unwrap_or(1000), 0)
    }

    fn select_page(
        &self,
        schema: &str,
        relation: &str,
        _: &str,
        _: &str,
        limit: u64,
        offset: u64,
    ) -> String {
        format!(
            "DATAKIT.VALUE {} {} {limit} {offset}",
            quote(schema),
            quote(relation)
        )
    }

    fn count_rows(&self, schema: &str, relation: &str, _: &str) -> String {
        format!("DATAKIT.LEN {} {}", quote(schema), quote(relation))
    }

    fn statements_are_lines(&self) -> bool {
        true
    }

    fn reads_only(&self, statement: &str) -> Option<bool> {
        Some(reads_only(statement))
    }

    fn supports_transactions(&self) -> bool {
        false
    }

    /// A key holds no definition to show.
    fn create_relation(&self, _: &str, _: &Relation) -> Vec<String> {
        Vec::new()
    }

    fn drop_relation(&self, _: &str, relation: &Relation) -> String {
        format!("DEL {}", quote(&relation.name()))
    }

    fn rename_relation(&self, _: &str, relation: &Relation, new: &str) -> String {
        format!("RENAME {} {}", quote(&relation.name()), quote(new))
    }
}
