//! Problems a console can point out before a statement runs: names the
//! catalog does not have, columns two relations share, a `DELETE` that
//! would empty a table.
//!
//! Inspections stay quiet unless they are sure. A name is unknown only when
//! every schema it could be in has been read; a statement that reads a
//! subquery or a common table expression is not checked for bare column
//! names at all.

use std::{collections::HashSet, ops::Range};

use datakit_catalog::{Catalog, Relation};
use datakit_driver::Dialect;

use crate::{
    analysis::{
        Reference, common_table_expressions, identifier, is_reserved, output_names, references,
        significant,
    },
    lexer::{Lexeme, Token, lex},
    statement::split_statements,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The statement will fail.
    Error,
    /// The statement runs but may not do what was meant.
    Warning,
}

/// What is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    UnknownSchema {
        schema: String,
    },
    UnknownRelation {
        relation: String,
    },
    UnknownColumn {
        column: String,
        relation: Option<String>,
    },
    /// More than one relation of the statement has the column.
    AmbiguousColumn {
        column: String,
        relations: Vec<String>,
    },
    /// The statement changes every row of the table.
    DeleteWithoutWhere,
    UpdateWithoutWhere,
}

/// A change to the text that fixes a problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    range: Range<usize>,
    replacement: String,
}

impl Fix {
    fn new(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }

    /// The bytes the fix replaces.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

/// One problem, where it is, and how it could be fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    range: Range<usize>,
    severity: Severity,
    problem: Problem,
    fixes: Vec<Fix>,
}

impl Inspection {
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }

    pub fn problem(&self) -> &Problem {
        &self.problem
    }

    /// Fixes, most likely first.
    pub fn fixes(&self) -> &[Fix] {
        &self.fixes
    }
}

/// Every problem in `text`, in order.
pub fn inspect(text: &str, catalog: &Catalog, dialect: &dyn Dialect) -> Vec<Inspection> {
    let mut found = Vec::new();
    for range in split_statements(text) {
        let tokens: Vec<Token> = lex(&text[range.clone()])
            .into_iter()
            .map(|token| token.offset_by(range.start))
            .collect();
        Statement {
            text,
            range,
            tokens: &tokens,
            catalog,
            dialect,
            found: &mut found,
        }
        .inspect();
    }
    found
}

struct Statement<'a, 'f> {
    text: &'a str,
    range: Range<usize>,
    tokens: &'a [Token],
    catalog: &'a Catalog,
    dialect: &'a dyn Dialect,
    found: &'f mut Vec<Inspection>,
}

impl<'a> Statement<'a, '_> {
    fn inspect(&mut self) {
        let significant = significant(self.tokens);
        let Some(first) = significant.first() else {
            return;
        };
        let references = references(self.text, self.tokens, self.dialect);
        let ctes = common_table_expressions(self.text, self.tokens, self.dialect);

        let mut resolved: Vec<(&Reference, Option<&Relation>)> = Vec::new();
        for reference in &references {
            let relation = self.check_reference(reference, &ctes);
            resolved.push((reference, relation));
        }

        if first.is_keyword(self.text, "delete") || first.is_keyword(self.text, "update") {
            self.check_where(&significant, first.is_keyword(self.text, "delete"));
        }

        self.check_qualified_columns(&significant, &resolved);
        let subquery_in_from = significant.windows(2).any(|pair| {
            (pair[0].is_keyword(self.text, "from") || pair[0].is_keyword(self.text, "join"))
                && pair[1].lexeme() == Lexeme::OpenParen
        });
        let all_known = !resolved.is_empty()
            && resolved.iter().all(|(_, relation)| relation.is_some())
            && ctes.is_empty()
            && !subquery_in_from;
        let reads_columns = ["select", "update", "delete"]
            .iter()
            .any(|word| first.is_keyword(self.text, word));
        if all_known && reads_columns {
            let relations: Vec<(&Reference, &Relation)> = resolved
                .iter()
                .map(|(reference, relation)| (*reference, relation.unwrap()))
                .collect();
            self.check_bare_columns(&significant, &relations);
        }
    }

    /// The catalog relation `reference` names, reporting it when the
    /// catalog is sure it does not exist.
    fn check_reference(
        &mut self,
        reference: &Reference,
        ctes: &HashSet<String>,
    ) -> Option<&'a Relation> {
        let catalog = self.catalog;
        match &reference.schema {
            Some(schema_name) => {
                let Some(schema) = catalog.schema(schema_name) else {
                    if catalog.has_schemas() {
                        let range = reference.schema_range.clone().unwrap();
                        let fixes = similar(
                            schema_name,
                            catalog.schemas().iter().map(|s| s.name().to_string()),
                        )
                        .into_iter()
                        .map(|name| Fix::new(range.clone(), self.dialect.quote_identifier(&name)))
                        .collect();
                        self.report(
                            range,
                            Severity::Error,
                            Problem::UnknownSchema {
                                schema: schema_name.clone(),
                            },
                            fixes,
                        );
                    }
                    return None;
                };
                let relations = schema.relations()?;
                let found = schema.relation(&reference.relation);
                if found.is_none() {
                    let fixes = similar(
                        &reference.relation,
                        relations.iter().map(|r| r.name().to_string()),
                    )
                    .into_iter()
                    .map(|name| {
                        Fix::new(
                            reference.relation_range.clone(),
                            self.dialect.quote_identifier(&name),
                        )
                    })
                    .collect();
                    self.report(
                        reference.relation_range.clone(),
                        Severity::Error,
                        Problem::UnknownRelation {
                            relation: reference.relation.clone(),
                        },
                        fixes,
                    );
                }
                found
            }
            None => {
                if ctes.contains(&reference.relation) {
                    return None;
                }
                if let Some(found) = catalog.resolve_relation(None, &reference.relation) {
                    return Some(found);
                }
                // Unknown only if every schema it could be in was read.
                let search_path = catalog.search_path();
                let all_read = !search_path.is_empty()
                    && search_path.iter().all(|schema| {
                        catalog
                            .schema(schema)
                            .is_none_or(|schema| schema.is_loaded())
                    });
                if all_read {
                    let candidates = search_path
                        .iter()
                        .filter_map(|schema| catalog.schema(schema))
                        .flat_map(|schema| schema.relations().unwrap_or_default())
                        .map(|relation| relation.name().to_string());
                    let fixes = similar(&reference.relation, candidates)
                        .into_iter()
                        .map(|name| {
                            Fix::new(
                                reference.relation_range.clone(),
                                self.dialect.quote_identifier(&name),
                            )
                        })
                        .collect();
                    self.report(
                        reference.relation_range.clone(),
                        Severity::Error,
                        Problem::UnknownRelation {
                            relation: reference.relation.clone(),
                        },
                        fixes,
                    );
                }
                None
            }
        }
    }

    fn check_where(&mut self, significant: &[&Token], delete: bool) {
        let mut depth = 0;
        for token in significant {
            match token.lexeme() {
                Lexeme::OpenParen => depth += 1,
                Lexeme::CloseParen => depth -= 1,
                _ if depth == 0 && token.is_keyword(self.text, "where") => return,
                _ => {}
            }
        }
        let first = significant[0].range();
        let last = significant
            .iter()
            .rev()
            .find(|token| token.lexeme() != Lexeme::Semicolon)
            .map(|token| token.range().end)
            .unwrap_or(self.range.end);
        self.report(
            first,
            Severity::Warning,
            if delete {
                Problem::DeleteWithoutWhere
            } else {
                Problem::UpdateWithoutWhere
            },
            vec![Fix::new(last..last, " WHERE ")],
        );
    }

    /// `alias.column` where the alias is a catalog relation without that
    /// column.
    fn check_qualified_columns(
        &mut self,
        significant: &[&Token],
        resolved: &[(&Reference, Option<&Relation>)],
    ) {
        for ix in 2..significant.len() {
            if significant[ix - 1].lexeme() != Lexeme::Period {
                continue;
            }
            let Some(qualifier) = identifier(self.text, significant[ix - 2], self.dialect) else {
                continue;
            };
            // `schema.relation.column` is not checked.
            if ix >= 4 && significant[ix - 3].lexeme() == Lexeme::Period {
                continue;
            }
            let Some(column) = identifier(self.text, significant[ix], self.dialect) else {
                continue;
            };
            if significant
                .get(ix + 1)
                .is_some_and(|t| matches!(t.lexeme(), Lexeme::OpenParen | Lexeme::Period))
            {
                continue;
            }
            let Some((reference, Some(relation))) = resolved
                .iter()
                .find(|(reference, _)| reference.visible_name() == qualifier)
            else {
                continue;
            };
            // The reference itself (`FROM schema.table`) is not a column.
            if reference.relation_range == significant[ix].range() {
                continue;
            }
            if relation.column(&column).is_none() {
                let range = significant[ix].range();
                let fixes = similar(
                    &column,
                    relation.columns().iter().map(|c| c.name().to_string()),
                )
                .into_iter()
                .map(|name| Fix::new(range.clone(), self.dialect.quote_identifier(&name)))
                .collect();
                self.report(
                    range,
                    Severity::Error,
                    Problem::UnknownColumn {
                        column,
                        relation: Some(relation.name().to_string()),
                    },
                    fixes,
                );
            }
        }
    }

    /// Bare names in expressions: each must be a column of exactly one
    /// relation the statement reads.
    fn check_bare_columns(
        &mut self,
        significant: &[&Token],
        relations: &[(&Reference, &Relation)],
    ) {
        let outputs = output_names(self.text, self.tokens, self.dialect);
        let keywords: HashSet<String> = self
            .dialect
            .keywords()
            .iter()
            .chain(self.dialect.reserved_words())
            .flat_map(|keyword| keyword.split_whitespace())
            .chain(EXPRESSION_WORDS.iter().copied())
            .map(str::to_lowercase)
            .collect();
        let functions: HashSet<String> = self
            .dialect
            .functions()
            .iter()
            .map(|f| f.to_lowercase())
            .collect();
        let declared: HashSet<Range<usize>> = relations
            .iter()
            .flat_map(|(reference, _)| {
                [
                    Some(reference.relation_range.clone()),
                    reference.alias_range.clone(),
                    reference.schema_range.clone(),
                ]
            })
            .flatten()
            .collect();

        for ix in 1..significant.len() {
            let token = significant[ix];
            if !matches!(token.lexeme(), Lexeme::Word | Lexeme::QuotedIdentifier)
                || declared.contains(&token.range())
            {
                continue;
            }
            let previous = significant[ix - 1];
            let next = significant.get(ix + 1);
            if previous.lexeme() == Lexeme::Period
                || previous.is_keyword(self.text, "as")
                || (previous.lexeme() == Lexeme::Operator && previous.text(self.text) == "::")
                || next.is_some_and(|t| {
                    matches!(
                        t.lexeme(),
                        Lexeme::Period | Lexeme::OpenParen | Lexeme::String
                    )
                })
            {
                continue;
            }
            let Some(name) = identifier(self.text, token, self.dialect) else {
                continue;
            };
            if token.lexeme() == Lexeme::Word {
                let lower = token.text(self.text).to_lowercase();
                if keywords.contains(&lower)
                    || functions.contains(&lower)
                    || is_reserved(self.text, token, self.dialect)
                {
                    continue;
                }
            }
            if outputs.contains(&name) || relations.iter().any(|(r, _)| r.visible_name() == name) {
                continue;
            }
            let owners: Vec<&(&Reference, &Relation)> = relations
                .iter()
                .filter(|(_, relation)| relation.column(&name).is_some())
                .collect();
            match owners.len() {
                1 => {}
                0 => {
                    let candidates = relations
                        .iter()
                        .flat_map(|(_, relation)| relation.columns())
                        .map(|column| column.name().to_string());
                    let fixes = similar(&name, candidates)
                        .into_iter()
                        .map(|found| Fix::new(token.range(), self.dialect.quote_identifier(&found)))
                        .collect();
                    self.report(
                        token.range(),
                        Severity::Error,
                        Problem::UnknownColumn {
                            column: name,
                            relation: None,
                        },
                        fixes,
                    );
                }
                _ => {
                    let fixes = owners
                        .iter()
                        .map(|(reference, _)| {
                            Fix::new(
                                token.range(),
                                format!(
                                    "{}.{}",
                                    self.dialect.quote_identifier(reference.visible_name()),
                                    token.text(self.text)
                                ),
                            )
                        })
                        .collect();
                    self.report(
                        token.range(),
                        Severity::Error,
                        Problem::AmbiguousColumn {
                            column: name,
                            relations: owners
                                .iter()
                                .map(|(reference, _)| reference.visible_name().to_string())
                                .collect(),
                        },
                        fixes,
                    );
                }
            }
        }
    }

    fn report(
        &mut self,
        range: Range<usize>,
        severity: Severity,
        problem: Problem,
        fixes: Vec<Fix>,
    ) {
        self.found.push(Inspection {
            range,
            severity,
            problem,
            fixes,
        });
    }
}

/// Words that appear in expressions without being names, whatever the
/// dialect's keyword list says.
const EXPRESSION_WORDS: &[&str] = &[
    "true",
    "false",
    "null",
    "is",
    "in",
    "between",
    "like",
    "ilike",
    "asc",
    "desc",
    "nulls",
    "first",
    "last",
    "distinct",
    "all",
    "any",
    "some",
    "exists",
    "case",
    "when",
    "then",
    "else",
    "end",
    "interval",
    "year",
    "month",
    "day",
    "hour",
    "minute",
    "second",
    "epoch",
    "over",
    "partition",
    "rows",
    "range",
    "preceding",
    "following",
    "unbounded",
    "current",
    "row",
    "filter",
    "within",
    "group",
    "by",
    "order",
    "limit",
    "offset",
    "having",
    "returning",
    "default",
    "set",
    "values",
    "using",
    "natural",
    "inner",
    "outer",
    "left",
    "right",
    "full",
    "cross",
    "lateral",
    "only",
    "date",
    "time",
    "timestamp",
    "zone",
    "at",
    "with",
    "without",
    "current_date",
    "current_time",
    "current_timestamp",
    "current_user",
    "localtime",
    "localtimestamp",
    "session_user",
    "collate",
    "escape",
    "similar",
    "to",
    "of",
];

/// Names among `candidates` close to `name`: the same but for case, or a
/// small typo away. At most three, closest first.
fn similar(name: &str, candidates: impl Iterator<Item = String>) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut scored: Vec<(usize, String)> = candidates
        .filter(|candidate| candidate != name)
        .filter_map(|candidate| {
            let distance = edit_distance(&lower, &candidate.to_lowercase());
            let limit = (name.chars().count() / 3).clamp(1, 3);
            (distance <= limit).then_some((distance, candidate))
        })
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            current.push(
                (previous[j] + cost)
                    .min(previous[j + 1] + 1)
                    .min(current[j] + 1),
            );
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Column, RelationType, Schema};

    use super::*;
    use crate::test_support::Plain;

    fn catalog() -> Catalog {
        Catalog::new("app")
            .with_schemas([Schema::new("shop"), Schema::new("audit")])
            .with_schema(
                Schema::new("shop").with_relations([
                    Relation::new("orders", RelationType::Table).with_columns([
                        Column::new("id", "integer"),
                        Column::new("customer_id", "integer"),
                        Column::new("total", "numeric"),
                    ]),
                    Relation::new("customers", RelationType::Table)
                        .with_columns([Column::new("id", "integer"), Column::new("name", "text")]),
                ]),
            )
            .with_search_path(["shop".into()])
    }

    fn problems(text: &str) -> Vec<Problem> {
        inspect(text, &catalog(), &Plain)
            .into_iter()
            .map(|inspection| inspection.problem().clone())
            .collect()
    }

    #[test]
    fn a_correct_statement_has_no_problems() {
        assert!(problems(
            "select o.id, c.name, total as t from orders o join customers c on c.id = o.customer_id \
             where total > 10 order by t desc nulls last"
        )
        .is_empty());
        assert!(problems("select count(*), now() from orders where id in (1, 2)").is_empty());
    }

    #[test]
    fn unknown_relations_are_reported_with_close_names() {
        let found = inspect("select * from oders", &catalog(), &Plain);
        assert_eq!(
            found[0].problem(),
            &Problem::UnknownRelation {
                relation: "oders".into()
            }
        );
        assert_eq!(found[0].fixes()[0].replacement(), "orders");
    }

    #[test]
    fn a_relation_in_an_unread_schema_is_not_reported() {
        assert!(problems("select * from audit.events").is_empty());
        assert_eq!(
            problems("select * from nowhere.events"),
            vec![Problem::UnknownSchema {
                schema: "nowhere".into()
            }]
        );
    }

    #[test]
    fn unknown_and_ambiguous_columns() {
        let found = inspect(
            "select o.totl, id from orders o join customers c on c.id = o.customer_id",
            &catalog(),
            &Plain,
        );
        assert_eq!(
            found[0].problem(),
            &Problem::UnknownColumn {
                column: "totl".into(),
                relation: Some("orders".into())
            }
        );
        assert_eq!(found[0].fixes()[0].replacement(), "total");
        assert_eq!(
            found[1].problem(),
            &Problem::AmbiguousColumn {
                column: "id".into(),
                relations: vec!["o".into(), "c".into()]
            }
        );
        assert_eq!(found[1].fixes()[0].replacement(), "o.id");
    }

    #[test]
    fn common_table_expressions_and_subqueries_are_not_guessed_at() {
        assert!(
            problems("with recent as (select * from orders) select whatever from recent")
                .is_empty()
        );
        assert!(problems("select x from (select 1 as x) s").is_empty());
    }

    #[test]
    fn deleting_every_row_is_a_warning_with_a_fix() {
        let text = "delete from orders;";
        let found = inspect(text, &catalog(), &Plain);
        assert_eq!(found[0].problem(), &Problem::DeleteWithoutWhere);
        assert_eq!(found[0].severity(), Severity::Warning);
        let fix = &found[0].fixes()[0];
        assert_eq!(fix.range(), 18..18);
        assert!(problems("delete from orders where id = 1").is_empty());
        assert_eq!(
            problems("update orders set total = 0"),
            vec![Problem::UpdateWithoutWhere]
        );
    }
}
