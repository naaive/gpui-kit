//! Statement parameters: `$1` and `:name` placeholders a person fills in
//! before the statement runs.
//!
//! Values are SQL text — `42`, `'Ada'`, `now()` — and replace their
//! placeholders before the statement is sent, which works with every
//! database and protocol and shows in the history exactly what ran.

use std::{collections::HashMap, ops::Range};

use crate::lexer::{Lexeme, lex};

/// One placeholder and every place it appears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parameter {
    name: String,
    ranges: Vec<Range<usize>>,
}

impl Parameter {
    /// The placeholder as written: `$1`, `:customer`.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn ranges(&self) -> &[Range<usize>] {
        &self.ranges
    }
}

/// The placeholders of `sql`, in order of first appearance. Placeholders in
/// strings, comments and dollar-quoted bodies are not placeholders.
pub fn parameters(sql: &str) -> Vec<Parameter> {
    let mut found: Vec<Parameter> = Vec::new();
    for token in lex(sql) {
        if token.lexeme() != Lexeme::Parameter {
            continue;
        }
        let name = token.text(sql);
        match found.iter_mut().find(|parameter| parameter.name == name) {
            Some(parameter) => parameter.ranges.push(token.range()),
            None => found.push(Parameter {
                name: name.to_string(),
                ranges: vec![token.range()],
            }),
        }
    }
    found
}

/// `sql` with every placeholder in `values` replaced by its value.
/// Placeholders without a value stay as they are.
pub fn substitute(sql: &str, values: &HashMap<String, String>) -> String {
    let mut edits: Vec<(Range<usize>, &str)> = parameters(sql)
        .iter()
        .filter_map(|parameter| {
            values
                .get(&parameter.name)
                .map(|value| (parameter, value.as_str()))
        })
        .flat_map(|(parameter, value)| {
            parameter
                .ranges
                .iter()
                .map(move |range| (range.clone(), value))
        })
        .collect();
    edits.sort_by_key(|(range, _)| range.start);
    let mut result = String::with_capacity(sql.len());
    let mut position = 0;
    for (range, value) in edits {
        result.push_str(&sql[position..range.start]);
        result.push_str(value);
        position = range.end;
    }
    result.push_str(&sql[position..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_found_once_each_outside_strings() {
        let sql = "select * from t where a = $1 and b = :name and c = $1 and d = ':no' -- :no";
        let found = parameters(sql);
        let names: Vec<&str> = found.iter().map(Parameter::name).collect();
        assert_eq!(names, ["$1", ":name"]);
        assert_eq!(found[0].ranges().len(), 2);
    }

    #[test]
    fn casts_are_not_placeholders() {
        assert!(parameters("select '1'::int, x::text").is_empty());
    }

    #[test]
    fn values_replace_their_placeholders() {
        let values = HashMap::from([
            ("$1".to_string(), "42".to_string()),
            (":name".to_string(), "'Ada'".to_string()),
        ]);
        assert_eq!(
            substitute("select $1, :name, $1, $2", &values),
            "select 42, 'Ada', 42, $2"
        );
    }
}
