//! What the name under the caret refers to.
//!
//! Quick documentation shows it, and Go to Declaration goes to it: an alias
//! to where the statement declares it, a table to the table, a column to its
//! table's column.

use std::{ops::Range, sync::Arc};

use datakit_catalog::Catalog;
use datakit_driver::Dialect;

use crate::{
    analysis::{Reference, identifier, references, significant, statement_tokens},
    lexer::{Lexeme, Token},
};

/// What a name refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Schema {
        schema: Arc<str>,
    },
    Relation {
        schema: Arc<str>,
        relation: Arc<str>,
    },
    Column {
        schema: Arc<str>,
        relation: Arc<str>,
        column: Arc<str>,
    },
    /// A function or procedure, by name; overloads are not told apart.
    Routine {
        schema: Arc<str>,
        name: Arc<str>,
    },
    /// A name the statement declares itself, such as an alias, declared at
    /// this byte range.
    Declaration(Range<usize>),
}

/// A name in the text and what it refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolution {
    range: Range<usize>,
    target: Target,
}

impl Resolution {
    /// The bytes of the name.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn target(&self) -> &Target {
        &self.target
    }
}

/// What the identifier at byte `offset` of `text` refers to, if anything
/// the catalog or the statement knows.
pub fn resolve(
    text: &str,
    offset: usize,
    catalog: &Catalog,
    dialect: &dyn Dialect,
) -> Option<Resolution> {
    let (_, tokens) = statement_tokens(text, offset)?;
    let significant = significant(&tokens);
    let ix = significant.iter().position(|token| {
        let range = token.range();
        range.start <= offset
            && offset <= range.end
            && matches!(token.lexeme(), Lexeme::Word | Lexeme::QuotedIdentifier)
    })?;
    let token = significant[ix];
    let name = identifier(text, token, dialect)?;
    let references = references(text, &tokens, dialect);
    let resolver = Resolver {
        catalog,
        references: &references,
    };
    let target = resolver.target(text, &significant, ix, &name, dialect)?;
    Some(Resolution {
        range: token.range(),
        target,
    })
}

struct Resolver<'a> {
    catalog: &'a Catalog,
    references: &'a [Reference],
}

impl Resolver<'_> {
    fn target(
        &self,
        text: &str,
        tokens: &[&Token],
        ix: usize,
        name: &str,
        dialect: &dyn Dialect,
    ) -> Option<Target> {
        let range = tokens[ix].range();

        // The name is part of a FROM/JOIN reference.
        if let Some(reference) = self
            .references
            .iter()
            .find(|reference| reference.relation_range == range)
        {
            return self.relation(reference.schema.as_deref(), &reference.relation);
        }
        if let Some(reference) = self
            .references
            .iter()
            .find(|reference| reference.schema_range.as_ref() == Some(&range))
        {
            let schema = self.catalog.schema(reference.schema.as_deref()?)?;
            return Some(Target::Schema {
                schema: schema.name(),
            });
        }
        if let Some(reference) = self
            .references
            .iter()
            .find(|reference| reference.alias_range.as_ref() == Some(&range))
        {
            return self.relation(reference.schema.as_deref(), &reference.relation);
        }

        let previous = ix.checked_sub(1).map(|ix| tokens[ix]);
        let next = tokens.get(ix + 1);

        // `qualifier.name`
        if previous.is_some_and(|t| t.lexeme() == Lexeme::Period)
            && let Some(qualifier) = ix
                .checked_sub(2)
                .and_then(|q| identifier(text, tokens[q], dialect))
        {
            if let Some(reference) = self.visible(&qualifier) {
                let relation =
                    self.resolve_relation(reference.schema.as_deref(), &reference.relation)?;
                return self.column(&relation.0, &relation.1, name);
            }
            // `schema.relation` or `schema.function(`
            if next.is_some_and(|t| t.lexeme() == Lexeme::OpenParen) {
                return self.routine(Some(&qualifier), name);
            }
            return self.relation(Some(&qualifier), name);
        }

        // `name.something`: an alias, a relation or a schema.
        if next.is_some_and(|t| t.lexeme() == Lexeme::Period) {
            if let Some(reference) = self.visible(name) {
                return Some(Target::Declaration(reference.declaration()));
            }
            if let Some(target) = self.relation(None, name) {
                return Some(target);
            }
            let schema = self.catalog.schema(name)?;
            return Some(Target::Schema {
                schema: schema.name(),
            });
        }

        // `name(`: a function.
        if next.is_some_and(|t| t.lexeme() == Lexeme::OpenParen) {
            return self.routine(None, name);
        }

        // A bare name: an alias, a column of a relation the statement
        // reads, or a relation.
        if let Some(reference) = self.visible(name) {
            return Some(Target::Declaration(reference.declaration()));
        }
        for reference in self.references {
            if let Some((schema, relation)) =
                self.resolve_relation(reference.schema.as_deref(), &reference.relation)
                && let Some(target) = self.column(&schema, &relation, name)
            {
                return Some(target);
            }
        }
        self.relation(None, name)
    }

    /// The reference the statement calls `name`.
    fn visible(&self, name: &str) -> Option<&Reference> {
        self.references
            .iter()
            .find(|reference| reference.visible_name() == name)
    }

    fn resolve_relation(&self, schema: Option<&str>, name: &str) -> Option<(Arc<str>, Arc<str>)> {
        match schema {
            Some(schema) => {
                let schema = self.catalog.schema(schema)?;
                let relation = schema.relation(name)?;
                Some((schema.name(), relation.name()))
            }
            None => self
                .catalog
                .search_path()
                .iter()
                .filter_map(|schema| self.catalog.schema(schema))
                .chain(self.catalog.schemas().iter())
                .find_map(|schema| {
                    schema
                        .relation(name)
                        .map(|relation| (schema.name(), relation.name()))
                }),
        }
    }

    fn relation(&self, schema: Option<&str>, name: &str) -> Option<Target> {
        let (schema, relation) = self.resolve_relation(schema, name)?;
        Some(Target::Relation { schema, relation })
    }

    fn column(&self, schema: &str, relation: &str, name: &str) -> Option<Target> {
        let found = self
            .catalog
            .schema(schema)?
            .relation(relation)?
            .column(name)?;
        Some(Target::Column {
            schema: schema.into(),
            relation: relation.into(),
            column: found.name(),
        })
    }

    fn routine(&self, schema: Option<&str>, name: &str) -> Option<Target> {
        let schemas: Vec<&datakit_catalog::Schema> = match schema {
            Some(schema) => vec![self.catalog.schema(schema)?],
            None => self
                .catalog
                .search_path()
                .iter()
                .filter_map(|schema| self.catalog.schema(schema))
                .collect(),
        };
        schemas.into_iter().find_map(|schema| {
            schema.routines_named(name).next().map(|_| Target::Routine {
                schema: schema.name(),
                name: name.into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Column, Relation, RelationType, Routine, RoutineType, Schema};

    use super::*;
    use crate::test_support::{Plain, caret};

    fn catalog() -> Catalog {
        Catalog::new("app")
            .with_schema(
                Schema::new("shop")
                    .with_relations([
                        Relation::new("orders", RelationType::Table).with_columns([
                            Column::new("id", "integer"),
                            Column::new("customer_id", "integer"),
                        ]),
                        Relation::new("customers", RelationType::Table).with_columns([
                            Column::new("id", "integer"),
                            Column::new("name", "text"),
                        ]),
                    ])
                    .with_routines([Routine::new("total", RoutineType::Function, "o integer")]),
            )
            .with_search_path(["shop".into()])
    }

    fn target(text: &str) -> Option<Target> {
        let (text, offset) = caret(text);
        resolve(&text, offset, &catalog(), &Plain).map(|resolution| resolution.target().clone())
    }

    fn relation(schema: &str, relation: &str) -> Option<Target> {
        Some(Target::Relation {
            schema: schema.into(),
            relation: relation.into(),
        })
    }

    #[test]
    fn a_relation_in_from_is_the_relation() {
        assert_eq!(
            target("select * from ord|ers o"),
            relation("shop", "orders")
        );
        assert_eq!(
            target("select * from shop.cust|omers"),
            relation("shop", "customers")
        );
        assert_eq!(
            target("select * from sh|op.customers"),
            Some(Target::Schema {
                schema: "shop".into()
            })
        );
    }

    #[test]
    fn an_alias_goes_to_its_declaration() {
        let text = "select o|.id from orders o";
        let (clean, _) = caret(text);
        let declared = clean.rfind('o').unwrap();
        assert_eq!(
            target(text),
            Some(Target::Declaration(declared..declared + 1))
        );
    }

    #[test]
    fn a_qualified_column_is_its_relations_column() {
        assert_eq!(
            target("select c.na|me from orders o join customers c on c.id = o.customer_id"),
            Some(Target::Column {
                schema: "shop".into(),
                relation: "customers".into(),
                column: "name".into()
            })
        );
    }

    #[test]
    fn a_bare_column_is_found_in_the_relations_read() {
        assert_eq!(
            target("select customer_|id from orders"),
            Some(Target::Column {
                schema: "shop".into(),
                relation: "orders".into(),
                column: "customer_id".into()
            })
        );
    }

    #[test]
    fn a_call_is_a_routine() {
        assert_eq!(
            target("select tot|al(id) from orders"),
            Some(Target::Routine {
                schema: "shop".into(),
                name: "total".into()
            })
        );
        assert_eq!(target("select nothing|_here"), None);
    }
}
