use crate::lexer::{Lexeme, lex};

/// Statements that only read, by their first word, and those that change
/// only the session: its settings and its transaction.
const READING: &[&str] = &[
    "select", "with", "values", "table", "show", "describe", "desc", "explain", "set", "use",
    "begin", "start", "commit", "rollback",
];

/// Words that make a reading statement write after all: `WITH … DELETE`,
/// `SELECT … INTO new_table`, `EXPLAIN ANALYZE DELETE` and the like.
const WRITING: &[&str] = &[
    "insert", "update", "delete", "merge", "into", "create", "alter", "drop", "truncate", "grant",
    "revoke", "call", "exec", "execute", "copy",
];

/// Whether `statement` only reads, as a read-only data source requires.
///
/// The test errs toward writing: a statement that starts with a reading
/// word but names a writing one anywhere outside strings and comments, or
/// `EXPLAIN ANALYZE`, which runs what it explains, does not count.
pub fn is_reading_statement(statement: &str) -> bool {
    let words: Vec<_> = lex(statement)
        .into_iter()
        .filter(|token| token.lexeme() == Lexeme::Word)
        .map(|token| token.text(statement).to_ascii_lowercase())
        .collect();
    let Some(first) = words.first() else {
        return true;
    };
    if !READING.contains(&first.as_str()) {
        return false;
    }
    if first == "explain" && words.get(1).is_some_and(|word| word == "analyze") {
        return false;
    }
    !words.iter().any(|word| WRITING.contains(&word.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_statements_that_read_count() {
        for statement in [
            "select * from orders",
            "select replace(name, 'a', 'b') from t",
            "  -- delete\nSELECT 'drop table x'",
            "with t as (select 1) select * from t",
            "explain select 1",
            "show tables",
            "set search_path to x",
            "",
        ] {
            assert!(is_reading_statement(statement), "{statement}");
        }
        for statement in [
            "delete from orders",
            "update orders set total = 0",
            "with gone as (delete from t returning *) select * from gone",
            "select * into copy from orders",
            "explain analyze delete from orders",
            "vacuum",
        ] {
            assert!(!is_reading_statement(statement), "{statement}");
        }
    }
}
