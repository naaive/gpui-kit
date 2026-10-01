//! What a statement names: the relations it reads and writes, the aliases
//! it gives them, the common table expressions it defines.
//!
//! Completion, resolution and inspections all start here. Like the rest of
//! the crate this reads tokens, not a parse tree, so it works on statements
//! that are still being typed.

use std::{collections::HashSet, ops::Range};

use datakit_driver::Dialect;

use crate::{
    lexer::{Lexeme, Token, lex},
    statement::statement_at,
};

/// A relation the statement names, with the name it goes by in the
/// statement and where each part is written.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Reference {
    pub schema: Option<String>,
    pub relation: String,
    pub alias: Option<String>,
    pub schema_range: Option<Range<usize>>,
    pub relation_range: Range<usize>,
    pub alias_range: Option<Range<usize>>,
}

impl Reference {
    /// The name the rest of the statement uses for the relation.
    pub fn visible_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.relation)
    }

    /// Where the visible name is declared.
    pub fn declaration(&self) -> Range<usize> {
        self.alias_range
            .clone()
            .unwrap_or_else(|| self.relation_range.clone())
    }
}

/// The statement around `offset` of `text`: its byte range and its tokens,
/// with ranges in `text`.
pub(crate) fn statement_tokens(text: &str, offset: usize) -> Option<(Range<usize>, Vec<Token>)> {
    let range = statement_at(text, offset)?;
    let tokens = lex(&text[range.clone()])
        .into_iter()
        .map(|token| token.offset_by(range.start))
        .collect();
    Some((range, tokens))
}

/// The tokens that mean something: no whitespace, no comments.
pub(crate) fn significant(tokens: &[Token]) -> Vec<&Token> {
    tokens
        .iter()
        .filter(|token| token.lexeme().is_significant())
        .collect()
}

/// Every `FROM`/`JOIN`/`UPDATE`/`INTO` reference in the statement.
pub(crate) fn references(text: &str, tokens: &[Token], dialect: &dyn Dialect) -> Vec<Reference> {
    let significant = significant(tokens);
    let mut references = Vec::new();
    let mut ix = 0;
    while ix < significant.len() {
        let token = significant[ix];
        ix += 1;
        let is_from = token.is_keyword(text, "from");
        if !(is_from
            || ["join", "update", "into"]
                .iter()
                .any(|keyword| token.is_keyword(text, keyword)))
        {
            continue;
        }
        let into = token.is_keyword(text, "into");
        while let Some((reference, next)) = reference_at(text, &significant, ix, into, dialect) {
            references.push(reference);
            ix = next;
            // `FROM a, b` lists several; anything else ends the list.
            let more = is_from
                && significant
                    .get(ix)
                    .is_some_and(|t| t.lexeme() == Lexeme::Comma);
            if !more {
                break;
            }
            ix += 1;
        }
    }
    references
}

/// The reference starting at `tokens[ix]`, and the index after it. After
/// `INTO`, a parenthesis is the column list rather than a function call.
fn reference_at(
    text: &str,
    tokens: &[&Token],
    mut ix: usize,
    into: bool,
    dialect: &dyn Dialect,
) -> Option<(Reference, usize)> {
    let first_token = tokens.get(ix)?;
    let first = identifier(text, first_token, dialect)?;
    // `FROM LATERAL`, `JOIN ONLY t`: a keyword is not a relation.
    if first_token.lexeme() == Lexeme::Word
        && ["lateral", "only", "select"]
            .iter()
            .any(|word| first_token.is_keyword(text, word))
    {
        return None;
    }
    ix += 1;
    let (schema, schema_range, relation, relation_range) =
        if tokens.get(ix).is_some_and(|t| t.lexeme() == Lexeme::Period) {
            match tokens.get(ix + 1) {
                Some(token) => match identifier(text, token, dialect) {
                    Some(relation) => {
                        ix += 2;
                        (
                            Some(first),
                            Some(first_token.range()),
                            relation,
                            token.range(),
                        )
                    }
                    None => return None,
                },
                None => return None,
            }
        } else {
            (None, None, first, first_token.range())
        };
    // A function call in FROM (`generate_series(1, 3)`) is not a relation.
    if !into
        && tokens
            .get(ix)
            .is_some_and(|t| t.lexeme() == Lexeme::OpenParen)
    {
        return None;
    }
    let mut alias = None;
    let mut alias_range = None;
    if tokens.get(ix).is_some_and(|t| t.is_keyword(text, "as")) {
        ix += 1;
    }
    if let Some(token) = tokens.get(ix)
        && let Some(name) = identifier(text, token, dialect)
        && !is_reserved(text, token, dialect)
    {
        alias = Some(name);
        alias_range = Some(token.range());
        ix += 1;
    }
    Some((
        Reference {
            schema,
            relation,
            alias,
            schema_range,
            relation_range,
            alias_range,
        },
        ix,
    ))
}

/// The names of the common table expressions the statement defines:
/// `WITH a AS (…), b AS (…)`.
pub(crate) fn common_table_expressions(
    text: &str,
    tokens: &[Token],
    dialect: &dyn Dialect,
) -> HashSet<String> {
    let significant = significant(tokens);
    let mut names = HashSet::new();
    let mut depth = 0i32;
    let mut expecting = false;
    for (ix, token) in significant.iter().enumerate() {
        match token.lexeme() {
            Lexeme::OpenParen => depth += 1,
            Lexeme::CloseParen => depth -= 1,
            _ => {}
        }
        if token.is_keyword(text, "with") || token.is_keyword(text, "recursive") {
            expecting = true;
            continue;
        }
        if expecting && depth >= 0 {
            if let Some(name) = identifier(text, token, dialect) {
                // `name AS (` or `name (columns) AS (`.
                let next = significant.get(ix + 1);
                if next.is_some_and(|t| t.is_keyword(text, "as") || t.lexeme() == Lexeme::OpenParen)
                {
                    names.insert(name);
                }
            }
            expecting = false;
        }
        // After a CTE body, a comma at the same depth starts another.
        if token.lexeme() == Lexeme::Comma
            && !names.is_empty()
            && significant[..ix]
                .iter()
                .rev()
                .find(|t| t.lexeme() != Lexeme::Comma)
                .is_some_and(|t| t.lexeme() == Lexeme::CloseParen)
        {
            expecting = true;
        }
    }
    names
}

/// Names the select list gives its columns with `AS`, which `ORDER BY` and
/// `GROUP BY` may use.
pub(crate) fn output_names(text: &str, tokens: &[Token], dialect: &dyn Dialect) -> HashSet<String> {
    let significant = significant(tokens);
    significant
        .windows(2)
        .filter(|pair| pair[0].is_keyword(text, "as"))
        .filter_map(|pair| identifier(text, pair[1], dialect))
        .collect()
}

/// The name a token refers to: an unquoted word folded as the dialect folds
/// it, or a quoted identifier with its quotes removed.
pub(crate) fn identifier(text: &str, token: &Token, dialect: &dyn Dialect) -> Option<String> {
    match token.lexeme() {
        Lexeme::Word => Some(dialect.fold_identifier(token.text(text))),
        Lexeme::QuotedIdentifier => {
            let raw = token.text(text);
            let quote = raw.chars().next()?;
            let inner = raw.strip_prefix(quote)?.strip_suffix(quote)?;
            Some(inner.replace(&format!("{quote}{quote}"), &quote.to_string()))
        }
        _ => None,
    }
}

pub(crate) fn is_reserved(text: &str, token: &Token, dialect: &dyn Dialect) -> bool {
    token.lexeme() == Lexeme::Word
        && (dialect
            .reserved_words()
            .iter()
            .any(|word| token.is_keyword(text, word))
            || CLAUSE_WORDS.iter().any(|word| token.is_keyword(text, word)))
}

/// Words that follow a relation reference and so can never be its alias,
/// whether or not the dialect reserves them.
const CLAUSE_WORDS: &[&str] = &[
    "where",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "cross",
    "natural",
    "on",
    "using",
    "group",
    "order",
    "having",
    "limit",
    "offset",
    "union",
    "except",
    "intersect",
    "set",
    "values",
    "returning",
    "window",
    "fetch",
    "for",
    "lateral",
    "default",
    "select",
];
