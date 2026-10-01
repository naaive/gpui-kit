//! Changes the editor offers at the caret that fix nothing but save typing,
//! as DataGrip's intentions do.

use std::ops::Range;

use datakit_catalog::Catalog;
use datakit_driver::Dialect;

use crate::{
    analysis::{identifier, references, significant, statement_tokens},
    lexer::Lexeme,
};

/// What an [`Intention`] does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refactoring {
    /// Replace `*` or `alias.*` in a select list with the columns it
    /// stands for.
    ExpandStar,
}

/// A change offered at the caret: `replacement` in place of `range`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intention {
    refactoring: Refactoring,
    range: Range<usize>,
    replacement: String,
}

impl Intention {
    pub fn refactoring(&self) -> Refactoring {
        self.refactoring
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn replacement(&self) -> &str {
        &self.replacement
    }
}

/// The intentions for the caret at byte `offset` of `text`.
pub fn intentions(
    text: &str,
    offset: usize,
    catalog: &Catalog,
    dialect: &dyn Dialect,
) -> Vec<Intention> {
    expand_star(text, offset, catalog, dialect)
        .into_iter()
        .collect()
}

fn expand_star(
    text: &str,
    offset: usize,
    catalog: &Catalog,
    dialect: &dyn Dialect,
) -> Option<Intention> {
    let (_, tokens) = statement_tokens(text, offset)?;
    let significant = significant(&tokens);
    let star = significant.iter().position(|token| {
        let range = token.range();
        token.lexeme() == Lexeme::Operator
            && token.text(text) == "*"
            && range.start <= offset
            && offset <= range.end
    })?;
    // `alias.*` expands one relation; `*` all of them.
    let qualifier = (star >= 2 && significant[star - 1].lexeme() == Lexeme::Period)
        .then(|| significant[star - 2])
        .filter(|token| matches!(token.lexeme(), Lexeme::Word | Lexeme::QuotedIdentifier));
    let first = star - if qualifier.is_some() { 2 } else { 0 };
    // Only a star in a select list: not `count(*)`, not `a * b`.
    let before = first.checked_sub(1).map(|ix| significant[ix])?;
    let in_select_list = before.lexeme() == Lexeme::Comma
        || ["select", "distinct", "all"]
            .iter()
            .any(|keyword| before.is_keyword(text, keyword));
    if !in_select_list {
        return None;
    }

    let references = references(text, &tokens, dialect);
    let qualifier = match qualifier {
        Some(token) => Some(identifier(text, token, dialect)?),
        None => None,
    };
    let expanded: Vec<_> = references
        .iter()
        .filter(|reference| {
            qualifier
                .as_deref()
                .is_none_or(|qualifier| reference.visible_name().eq_ignore_ascii_case(qualifier))
        })
        .collect();
    if expanded.is_empty() {
        return None;
    }
    // Name the relation with each column whenever the statement reads more
    // than one, or the star was qualified.
    let qualify = qualifier.is_some() || references.len() > 1;
    let mut columns = Vec::new();
    for reference in expanded {
        let relation =
            catalog.resolve_relation(reference.schema.as_deref(), &reference.relation)?;
        if relation.columns().is_empty() {
            return None;
        }
        for column in relation.columns() {
            let column = dialect.quote_identifier(&column.name());
            columns.push(if qualify {
                format!(
                    "{}.{column}",
                    dialect.quote_identifier(reference.visible_name())
                )
            } else {
                column
            });
        }
    }
    Some(Intention {
        refactoring: Refactoring::ExpandStar,
        range: significant[first].range().start..significant[star].range().end,
        replacement: columns.join(", "),
    })
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Column, Relation, RelationType, Schema};

    use super::*;
    use crate::test_support::{Plain, caret};

    fn catalog() -> Catalog {
        Catalog::new("app")
            .with_schema(
                Schema::new("shop").with_relations([
                    Relation::new("orders", RelationType::Table).with_columns([
                        Column::new("id", "integer"),
                        Column::new("customer_id", "integer"),
                    ]),
                    Relation::new("customers", RelationType::Table)
                        .with_columns([Column::new("id", "integer"), Column::new("name", "text")]),
                ]),
            )
            .with_search_path(["shop".into()])
    }

    fn expanded(text: &str) -> Option<String> {
        let (text, offset) = caret(text);
        let intention = intentions(&text, offset, &catalog(), &Plain)
            .into_iter()
            .find(|intention| intention.refactoring() == Refactoring::ExpandStar)?;
        let mut result = text.clone();
        result.replace_range(intention.range(), intention.replacement());
        Some(result)
    }

    #[test]
    fn a_star_expands_to_the_columns_it_stands_for() {
        assert_eq!(
            expanded("select *| from orders").as_deref(),
            Some("select id, customer_id from orders")
        );
        assert_eq!(
            expanded("select |* from orders o join customers c on c.id = o.customer_id").as_deref(),
            Some(
                "select o.id, o.customer_id, c.id, c.name from orders o \
                 join customers c on c.id = o.customer_id"
            )
        );
        assert_eq!(
            expanded("select o.id, c.*| from orders o join customers c on true").as_deref(),
            Some("select o.id, c.id, c.name from orders o join customers c on true")
        );
    }

    #[test]
    fn other_stars_stay() {
        assert_eq!(expanded("select count(*|) from orders"), None);
        assert_eq!(expanded("select 2 *| 3 from orders"), None);
        assert_eq!(expanded("select *| from unknown"), None);
    }
}
