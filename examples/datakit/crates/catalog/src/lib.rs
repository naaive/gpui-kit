//! What a database contains, as an immutable snapshot.
//!
//! A [`Catalog`] is the one description of a database's objects that every
//! part of DataKit reads: the explorer draws it, completion searches it, and
//! later the schema diff compares two of them. It holds no connection and does
//! no IO. A driver produces the pieces; the owner of the snapshot replaces it
//! with a new one ([`Catalog::with_schemas`], [`Catalog::with_relations`])
//! instead of mutating it, so a reader on another thread never sees half an
//! update and a clone costs a reference count.
//!
//! Introspection is lazy. A schema's relations are `None` until someone asks
//! for them, which is what lets the explorer open a server with thousands of
//! tables without reading all of them first.

mod diff;
mod object;

pub use diff::{RelationChange, SchemaDiff, diff_schemas};
pub use object::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationType, Routine, RoutineType, Schema, Sequence, Trigger,
};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The objects of one database, as far as they have been introspected.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    database: Arc<str>,
    schemas: Arc<[Schema]>,
    search_path: Arc<[Arc<str>]>,
}

impl Catalog {
    /// An empty catalog for `database`; nothing has been introspected yet.
    pub fn new(database: impl Into<Arc<str>>) -> Self {
        Self {
            database: database.into(),
            schemas: Arc::from([]),
            search_path: Arc::from([]),
        }
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    pub fn schemas(&self) -> &[Schema] {
        &self.schemas
    }

    pub fn schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.iter().find(|schema| &*schema.name() == name)
    }

    /// Whether the list of schemas has been read at least once.
    pub fn has_schemas(&self) -> bool {
        !self.schemas.is_empty()
    }

    /// The schemas an unqualified name is looked up in, in order.
    pub fn search_path(&self) -> &[Arc<str>] {
        &self.search_path
    }

    /// The same catalog with `search_path` as the lookup order for
    /// unqualified names.
    pub fn with_search_path(&self, search_path: impl IntoIterator<Item = Arc<str>>) -> Self {
        Self {
            search_path: search_path.into_iter().collect(),
            ..self.clone()
        }
    }

    /// The same catalog with `schemas` as its list of schemas.
    ///
    /// A schema that was already loaded keeps its relations when the new
    /// list still names it, so refreshing the list does not throw away what
    /// the user has expanded. A schema in `schemas` that carries relations of
    /// its own replaces the old one outright.
    pub fn with_schemas(&self, schemas: impl IntoIterator<Item = Schema>) -> Self {
        let schemas = schemas
            .into_iter()
            .map(|schema| {
                if schema.is_loaded() {
                    return schema;
                }
                match self.schema(&schema.name()) {
                    Some(previous) if previous.is_loaded() => schema.with_contents_of(previous),
                    _ => schema,
                }
            })
            .collect();
        Self {
            schemas,
            ..self.clone()
        }
    }

    /// The same catalog with `loaded` as the contents of the schema of its
    /// name: its relations, routines and sequences.
    ///
    /// A schema the catalog does not list yet is added, so a result that
    /// arrives before the schema list is not lost.
    pub fn with_schema(&self, loaded: Schema) -> Self {
        let mut schemas: Vec<Schema> = self.schemas.to_vec();
        match schemas.iter_mut().find(|s| s.name() == loaded.name()) {
            Some(existing) => *existing = existing.clone().with_contents_of(&loaded),
            None => schemas.push(loaded),
        }
        Self {
            schemas: schemas.into(),
            ..self.clone()
        }
    }

    /// The same catalog with `relations` as the contents of `schema`.
    ///
    /// A schema the catalog does not list yet is added, so a result that
    /// arrives before the schema list is not lost.
    pub fn with_relations(
        &self,
        schema: &str,
        relations: impl IntoIterator<Item = Relation>,
    ) -> Self {
        let relations: Vec<Relation> = relations.into_iter().collect();
        let mut schemas: Vec<Schema> = self.schemas.to_vec();
        match schemas.iter_mut().find(|s| &*s.name() == schema) {
            Some(existing) => *existing = existing.clone().with_relations(relations),
            None => schemas.push(Schema::new(schema).with_relations(relations)),
        }
        Self {
            schemas: schemas.into(),
            ..self.clone()
        }
    }

    /// The same catalog with the relations of `schema` forgotten, so the next
    /// read introspects them again.
    pub fn without_relations(&self, schema: &str) -> Self {
        let schemas: Vec<Schema> = self
            .schemas
            .iter()
            .map(|s| {
                if &*s.name() == schema {
                    s.clone().unloaded()
                } else {
                    s.clone()
                }
            })
            .collect();
        Self {
            schemas: schemas.into(),
            ..self.clone()
        }
    }

    /// The relation `name`, in `schema` when given, otherwise in the first
    /// schema of the search path that has one, otherwise in any loaded schema.
    ///
    /// Names compare as the database folds them: an unquoted identifier was
    /// already folded by the caller, so this compares exactly.
    pub fn resolve_relation(&self, schema: Option<&str>, name: &str) -> Option<&Relation> {
        if let Some(schema) = schema {
            return self.schema(schema)?.relation(name);
        }
        self.search_path
            .iter()
            .filter_map(|schema| self.schema(schema))
            .chain(self.schemas.iter())
            .find_map(|schema| schema.relation(name))
    }

    /// Every relation that references `schema.relation` through a foreign
    /// key, with the schema it is in.
    pub fn referencing<'a>(
        &'a self,
        schema: &'a str,
        relation: &'a str,
    ) -> impl Iterator<Item = (&'a Schema, &'a Relation, &'a ForeignKey)> + 'a {
        self.relations().flat_map(move |(owner, candidate)| {
            candidate.foreign_keys().filter_map(move |(_, key)| {
                (key.referenced_schema() == schema && key.referenced_relation() == relation)
                    .then_some((owner, candidate, key))
            })
        })
    }

    /// Every loaded relation, with the schema it belongs to.
    pub fn relations(&self) -> impl Iterator<Item = (&Schema, &Relation)> {
        self.schemas.iter().flat_map(|schema| {
            schema
                .relations()
                .into_iter()
                .flatten()
                .map(move |relation| (schema, relation))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users() -> Relation {
        Relation::new("users", RelationType::Table).with_columns([
            Column::new("id", "integer").primary_key(true),
            Column::new("email", "text").nullable(true),
        ])
    }

    #[test]
    fn refreshing_the_schema_list_keeps_loaded_relations() {
        let catalog = Catalog::new("app")
            .with_schemas([Schema::new("public"), Schema::new("audit")])
            .with_relations("public", [users()]);

        let refreshed = catalog.with_schemas([Schema::new("public"), Schema::new("billing")]);

        let public = refreshed.schema("public").unwrap();
        assert_eq!(public.relations().unwrap().len(), 1);
        assert!(refreshed.schema("audit").is_none());
        assert!(refreshed.schema("billing").unwrap().relations().is_none());
    }

    #[test]
    fn relations_for_an_unlisted_schema_add_it() {
        let catalog = Catalog::new("app").with_relations("public", [users()]);
        assert_eq!(catalog.schemas().len(), 1);
        assert!(catalog.resolve_relation(Some("public"), "users").is_some());
    }

    #[test]
    fn unqualified_names_follow_the_search_path() {
        let catalog = Catalog::new("app")
            .with_relations("audit", [Relation::new("users", RelationType::View)])
            .with_relations("public", [users()])
            .with_search_path(["public".into()]);

        let relation = catalog.resolve_relation(None, "users").unwrap();
        assert_eq!(relation.relation_type(), RelationType::Table);

        let audit = catalog.resolve_relation(Some("audit"), "users").unwrap();
        assert_eq!(audit.relation_type(), RelationType::View);
    }

    #[test]
    fn forgetting_relations_unloads_only_that_schema() {
        let catalog = Catalog::new("app")
            .with_relations("public", [users()])
            .with_relations("audit", [users()]);
        let catalog = catalog.without_relations("public");
        assert!(catalog.schema("public").unwrap().relations().is_none());
        assert!(catalog.schema("audit").unwrap().relations().is_some());
    }
}
