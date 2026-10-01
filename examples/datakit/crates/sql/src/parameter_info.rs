//! Parameter info: the signatures of the routine whose argument list the
//! caret is in, and which argument it is at.

use std::ops::Range;

use datakit_catalog::Catalog;
use datakit_driver::Dialect;

use crate::{
    analysis::{identifier, is_reserved, significant, statement_tokens},
    lexer::Lexeme,
};

/// The call around the caret and what the catalog knows of the routine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterInfo {
    name: String,
    name_range: Range<usize>,
    argument: usize,
    signatures: Vec<Signature>,
}

/// One overload: its arguments as the database prints them, and its result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    arguments: Vec<String>,
    result: Option<String>,
}

impl ParameterInfo {
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the routine's name is written.
    pub fn name_range(&self) -> Range<usize> {
        self.name_range.clone()
    }

    /// The 0-based argument the caret is in.
    pub fn argument(&self) -> usize {
        self.argument
    }

    /// The overloads; empty for a routine the catalog does not describe,
    /// such as a built-in function.
    pub fn signatures(&self) -> &[Signature] {
        &self.signatures
    }
}

impl Signature {
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }
}

/// The routine call whose argument list holds byte `offset` of `text`.
pub fn parameter_info(
    text: &str,
    offset: usize,
    catalog: &Catalog,
    dialect: &dyn Dialect,
) -> Option<ParameterInfo> {
    let (_, tokens) = statement_tokens(text, offset)?;
    let significant = significant(&tokens);
    let before = significant
        .iter()
        .rposition(|token| token.range().end <= offset && token.lexeme() != Lexeme::Unterminated)
        .map_or(0, |ix| ix + 1);
    // Back to the unmatched `(`, counting the commas at its depth.
    let mut depth = 0usize;
    let mut argument = 0;
    let mut open = None;
    for ix in (0..before).rev() {
        match significant[ix].lexeme() {
            Lexeme::CloseParen => depth += 1,
            Lexeme::OpenParen if depth == 0 => {
                open = Some(ix);
                break;
            }
            Lexeme::OpenParen => depth -= 1,
            Lexeme::Comma if depth == 0 => argument += 1,
            _ => {}
        }
    }
    let name_ix = open?.checked_sub(1)?;
    let name_token = significant[name_ix];
    /// Words a `(` follows that do not call anything.
    const NOT_CALLS: &[&str] = &[
        "in", "exists", "values", "any", "all", "some", "as", "from", "join", "using", "over",
        "filter", "within", "into", "on", "and", "or", "not", "when", "then", "else", "where",
    ];
    let callable = match name_token.lexeme() {
        Lexeme::QuotedIdentifier => true,
        Lexeme::Word => {
            !is_reserved(text, name_token, dialect)
                && !NOT_CALLS
                    .iter()
                    .any(|word| name_token.is_keyword(text, word))
        }
        _ => false,
    };
    if !callable {
        return None;
    }
    let name = identifier(text, name_token, dialect)?;
    let schema = (name_ix >= 2 && significant[name_ix - 1].lexeme() == Lexeme::Period)
        .then(|| identifier(text, significant[name_ix - 2], dialect))
        .flatten();
    let schemas: Vec<_> = match &schema {
        Some(schema) => catalog.schema(schema).into_iter().collect(),
        None => catalog
            .search_path()
            .iter()
            .filter_map(|schema| catalog.schema(schema))
            .collect(),
    };
    // Overloads from the fewest arguments up.
    let mut signatures: Vec<Signature> = schemas
        .iter()
        .flat_map(|schema| schema.routines_named(&name))
        .map(|routine| Signature {
            arguments: split_arguments(routine.arguments()),
            result: routine.result().map(str::to_string),
        })
        .collect();
    signatures.sort_by_key(|signature| signature.arguments.len());
    Some(ParameterInfo {
        name,
        name_range: name_token.range(),
        argument,
        signatures,
    })
}

/// `a integer, b numeric(10, 2)` as its arguments, splitting only on the
/// commas outside parentheses.
fn split_arguments(arguments: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (ix, c) in arguments.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(arguments[start..ix].trim().to_string());
                start = ix + 1;
            }
            _ => {}
        }
    }
    let last = arguments[start..].trim();
    if !last.is_empty() {
        parts.push(last.to_string());
    }
    parts
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Routine, RoutineType, Schema};

    use super::*;
    use crate::test_support::{Plain, caret};

    fn catalog() -> Catalog {
        Catalog::new("app")
            .with_schema(
                Schema::new("shop").with_routines([
                    Routine::new(
                        "total",
                        RoutineType::Function,
                        "o integer, rate numeric(10, 2)",
                    )
                    .with_result("numeric"),
                    Routine::new("total", RoutineType::Function, "o integer"),
                ]),
            )
            .with_search_path(["shop".into()])
    }

    fn info(text: &str) -> Option<ParameterInfo> {
        let (text, offset) = caret(text);
        parameter_info(&text, offset, &catalog(), &Plain)
    }

    #[test]
    fn the_caret_in_an_argument_list_finds_the_routine_and_argument() {
        let found = info("select total(1, coalesce(a, b), |) from t").unwrap();
        assert_eq!(found.name(), "total");
        assert_eq!(found.argument(), 2);
        assert_eq!(found.signatures().len(), 2);
        assert_eq!(
            found.signatures()[1].arguments(),
            ["o integer", "rate numeric(10, 2)"]
        );
        assert_eq!(found.signatures()[1].result(), Some("numeric"));

        let nested = info("select total(coalesce(a, |b))").unwrap();
        assert_eq!(nested.name(), "coalesce");
        assert_eq!(nested.argument(), 1);
        assert!(nested.signatures().is_empty());

        assert_eq!(info("select (1 + |2)"), None);
        assert_eq!(info("select 1 where a in (1, |2)"), None);
        assert_eq!(info("select total(1)|"), None);
    }
}
