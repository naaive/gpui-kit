//! What differs between two versions of a schema.
//!
//! A diff is read in one direction: it lists what has to change for the
//! *target* to look like the *source*. The schema comparison window shows it
//! as a tree, and a dialect turns it into the statements that migrate the
//! target; modifying a table is a diff between the table as it is and as the
//! person edited it.

use std::collections::HashMap;

use crate::{Column, Constraint, Index, Relation, Routine, Schema, Sequence, Trigger};

/// How two schemas differ: what the target lacks, has too many of, or has
/// in another form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SchemaDiff {
    added_relations: Vec<Relation>,
    removed_relations: Vec<Relation>,
    changed_relations: Vec<RelationChange>,
    added_routines: Vec<Routine>,
    removed_routines: Vec<Routine>,
    /// `(target, source)` pairs with the same signature and another body.
    changed_routines: Vec<(Routine, Routine)>,
    added_sequences: Vec<Sequence>,
    removed_sequences: Vec<Sequence>,
}

impl SchemaDiff {
    /// Relations the source has and the target lacks.
    pub fn added_relations(&self) -> &[Relation] {
        &self.added_relations
    }

    /// Relations the target has and the source lacks.
    pub fn removed_relations(&self) -> &[Relation] {
        &self.removed_relations
    }

    pub fn changed_relations(&self) -> &[RelationChange] {
        &self.changed_relations
    }

    pub fn added_routines(&self) -> &[Routine] {
        &self.added_routines
    }

    pub fn removed_routines(&self) -> &[Routine] {
        &self.removed_routines
    }

    pub fn changed_routines(&self) -> &[(Routine, Routine)] {
        &self.changed_routines
    }

    pub fn added_sequences(&self) -> &[Sequence] {
        &self.added_sequences
    }

    pub fn removed_sequences(&self) -> &[Sequence] {
        &self.removed_sequences
    }

    /// Whether the schemas are the same.
    pub fn is_empty(&self) -> bool {
        self.added_relations.is_empty()
            && self.removed_relations.is_empty()
            && self.changed_relations.is_empty()
            && self.added_routines.is_empty()
            && self.removed_routines.is_empty()
            && self.changed_routines.is_empty()
            && self.added_sequences.is_empty()
            && self.removed_sequences.is_empty()
    }
}

/// How one relation differs between the target and the source.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationChange {
    target: Relation,
    source: Relation,
    added_columns: Vec<Column>,
    removed_columns: Vec<Column>,
    /// `(target, source)` pairs with the same name.
    changed_columns: Vec<(Column, Column)>,
    added_indexes: Vec<Index>,
    removed_indexes: Vec<Index>,
    added_constraints: Vec<Constraint>,
    removed_constraints: Vec<Constraint>,
    added_triggers: Vec<Trigger>,
    removed_triggers: Vec<Trigger>,
    comment_changed: bool,
    definition_changed: bool,
}

impl RelationChange {
    /// How `target` has to change to become `source`, or `None` when they
    /// are the same. The two must be the same relation: the same name, or
    /// one renamed by the caller.
    pub fn between(target: &Relation, source: &Relation) -> Option<Self> {
        let (added_columns, removed_columns, changed_columns) = compare(
            target.columns(),
            source.columns(),
            Column::name,
            columns_differ,
        );
        // An index or constraint that changed is dropped and created again.
        let (added_indexes, removed_indexes, changed_indexes) =
            compare(target.indexes(), source.indexes(), Index::name, |a, b| {
                !same_index(a, b)
            });
        let (added_constraints, removed_constraints, changed_constraints) = compare(
            target.constraints(),
            source.constraints(),
            Constraint::name,
            |a, b| !same_constraint(a, b),
        );
        let (added_triggers, removed_triggers, changed_triggers) = compare(
            target.triggers(),
            source.triggers(),
            Trigger::name,
            |a, b| a.definition() != b.definition() || a.timing() != b.timing(),
        );

        let change = Self {
            target: target.clone(),
            source: source.clone(),
            added_columns,
            removed_columns,
            changed_columns,
            added_indexes: added_indexes
                .into_iter()
                .chain(changed_indexes.iter().map(|(_, b)| b.clone()))
                .collect(),
            removed_indexes: removed_indexes
                .into_iter()
                .chain(changed_indexes.into_iter().map(|(a, _)| a))
                .collect(),
            added_constraints: added_constraints
                .into_iter()
                .chain(changed_constraints.iter().map(|(_, b)| b.clone()))
                .collect(),
            removed_constraints: removed_constraints
                .into_iter()
                .chain(changed_constraints.into_iter().map(|(a, _)| a))
                .collect(),
            added_triggers: added_triggers
                .into_iter()
                .chain(changed_triggers.iter().map(|(_, b)| b.clone()))
                .collect(),
            removed_triggers: removed_triggers
                .into_iter()
                .chain(changed_triggers.into_iter().map(|(a, _)| a))
                .collect(),
            comment_changed: target.comment() != source.comment(),
            definition_changed: normalize(target.definition()) != normalize(source.definition()),
        };
        (!change.is_empty()).then_some(change)
    }

    fn is_empty(&self) -> bool {
        self.added_columns.is_empty()
            && self.removed_columns.is_empty()
            && self.changed_columns.is_empty()
            && self.added_indexes.is_empty()
            && self.removed_indexes.is_empty()
            && self.added_constraints.is_empty()
            && self.removed_constraints.is_empty()
            && self.added_triggers.is_empty()
            && self.removed_triggers.is_empty()
            && !self.comment_changed
            && !self.definition_changed
    }

    /// The relation as it is in the target.
    pub fn target(&self) -> &Relation {
        &self.target
    }

    /// The relation as it is in the source.
    pub fn source(&self) -> &Relation {
        &self.source
    }

    pub fn added_columns(&self) -> &[Column] {
        &self.added_columns
    }

    pub fn removed_columns(&self) -> &[Column] {
        &self.removed_columns
    }

    pub fn changed_columns(&self) -> &[(Column, Column)] {
        &self.changed_columns
    }

    pub fn added_indexes(&self) -> &[Index] {
        &self.added_indexes
    }

    pub fn removed_indexes(&self) -> &[Index] {
        &self.removed_indexes
    }

    pub fn added_constraints(&self) -> &[Constraint] {
        &self.added_constraints
    }

    pub fn removed_constraints(&self) -> &[Constraint] {
        &self.removed_constraints
    }

    pub fn added_triggers(&self) -> &[Trigger] {
        &self.added_triggers
    }

    pub fn removed_triggers(&self) -> &[Trigger] {
        &self.removed_triggers
    }

    pub fn is_comment_changed(&self) -> bool {
        self.comment_changed
    }

    /// Whether a view's query changed.
    pub fn is_definition_changed(&self) -> bool {
        self.definition_changed
    }
}

/// What must change for `target` to look like `source`. Only loaded
/// contents are compared; a schema that was never introspected compares as
/// empty.
pub fn diff_schemas(target: &Schema, source: &Schema) -> SchemaDiff {
    let target_relations = target.relations().unwrap_or_default();
    let source_relations = source.relations().unwrap_or_default();
    let (added_relations, removed_relations, changed) = compare(
        target_relations,
        source_relations,
        Relation::name,
        |a, b| a != b,
    );
    let mut changed_relations = Vec::new();
    let mut retyped = Vec::new();
    for (target, source) in changed {
        if target.relation_type() != source.relation_type() {
            // A table that became a view is dropped and created.
            retyped.push((target, source));
        } else if let Some(change) = RelationChange::between(&target, &source) {
            changed_relations.push(change);
        }
    }
    let (added_routines, removed_routines, changed_routines) = compare(
        target.routines(),
        source.routines(),
        |routine| routine.signature().into(),
        |a, b| normalize(a.definition()) != normalize(b.definition()) || a.result() != b.result(),
    );
    let (added_sequences, removed_sequences, changed_sequences) = compare(
        target.sequences(),
        source.sequences(),
        Sequence::name,
        |a, b| {
            a.increment() != b.increment()
                || a.min_value() != b.min_value()
                || a.max_value() != b.max_value()
                || a.is_cycle() != b.is_cycle()
                || a.data_type() != b.data_type()
        },
    );
    SchemaDiff {
        added_relations: added_relations
            .into_iter()
            .chain(retyped.iter().map(|(_, source)| source.clone()))
            .collect(),
        removed_relations: removed_relations
            .into_iter()
            .chain(retyped.into_iter().map(|(target, _)| target))
            .collect(),
        changed_relations,
        added_routines,
        removed_routines,
        changed_routines,
        added_sequences: added_sequences
            .into_iter()
            .chain(changed_sequences.iter().map(|(_, b)| b.clone()))
            .collect(),
        removed_sequences: removed_sequences
            .into_iter()
            .chain(changed_sequences.into_iter().map(|(a, _)| a))
            .collect(),
    }
}

/// Match `target` and `source` by `key`: what only the source has, what
/// only the target has, and the `(target, source)` pairs that `differ`.
/// Each list keeps the order of the side it came from.
#[allow(clippy::type_complexity)]
fn compare<T: Clone>(
    target: &[T],
    source: &[T],
    key: impl Fn(&T) -> std::sync::Arc<str>,
    differ: impl Fn(&T, &T) -> bool,
) -> (Vec<T>, Vec<T>, Vec<(T, T)>) {
    let target_by_key: HashMap<std::sync::Arc<str>, &T> =
        target.iter().map(|item| (key(item), item)).collect();
    let source_keys: std::collections::HashSet<std::sync::Arc<str>> =
        source.iter().map(&key).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for item in source {
        match target_by_key.get(&key(item)) {
            None => added.push(item.clone()),
            Some(existing) if differ(existing, item) => {
                changed.push(((*existing).clone(), item.clone()))
            }
            Some(_) => {}
        }
    }
    let removed = target
        .iter()
        .filter(|item| !source_keys.contains(&key(item)))
        .cloned()
        .collect();
    (added, removed, changed)
}

fn columns_differ(a: &Column, b: &Column) -> bool {
    a.data_type() != b.data_type()
        || a.is_nullable() != b.is_nullable()
        || a.default() != b.default()
        || a.comment() != b.comment()
}

fn same_index(a: &Index, b: &Index) -> bool {
    a.columns() == b.columns()
        && a.is_unique() == b.is_unique()
        && a.method() == b.method()
        && a.predicate() == b.predicate()
}

fn same_constraint(a: &Constraint, b: &Constraint) -> bool {
    match (a.definition(), b.definition()) {
        (Some(a), Some(b)) => normalize(Some(a)) == normalize(Some(b)),
        _ => a.rule() == b.rule(),
    }
}

/// Text with runs of whitespace collapsed, so a reformatted body is not a
/// change.
fn normalize(text: Option<&str>) -> Option<String> {
    text.map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConstraintRule, RelationType};

    fn users(extra: Option<Column>) -> Relation {
        let mut columns = vec![
            Column::new("id", "integer").primary_key(true),
            Column::new("email", "text").nullable(true),
        ];
        columns.extend(extra);
        Relation::new("users", RelationType::Table)
            .with_columns(columns)
            .with_constraints([Constraint::new(
                "users_pkey",
                ConstraintRule::PrimaryKey {
                    columns: vec!["id".into()].into(),
                },
            )])
    }

    #[test]
    fn identical_schemas_have_no_diff() {
        let schema = Schema::new("public").with_relations([users(None)]);
        assert!(diff_schemas(&schema, &schema).is_empty());
    }

    #[test]
    fn relations_are_added_removed_and_changed() {
        let target = Schema::new("public")
            .with_relations([users(None), Relation::new("legacy", RelationType::Table)]);
        let source = Schema::new("public").with_relations([
            users(Some(Column::new("name", "text"))),
            Relation::new("orders", RelationType::Table),
        ]);
        let diff = diff_schemas(&target, &source);
        assert_eq!(&*diff.added_relations()[0].name(), "orders");
        assert_eq!(&*diff.removed_relations()[0].name(), "legacy");
        let change = &diff.changed_relations()[0];
        assert_eq!(&*change.added_columns()[0].name(), "name");
        assert!(change.removed_columns().is_empty());
    }

    #[test]
    fn a_changed_column_type_is_a_change_not_a_drop() {
        let target = users(None);
        let source = Relation::new("users", RelationType::Table)
            .with_columns([
                Column::new("id", "bigint").primary_key(true),
                Column::new("email", "text").nullable(true),
            ])
            .with_constraints(target.constraints().iter().cloned());
        let change = RelationChange::between(&target, &source).unwrap();
        let (old, new) = &change.changed_columns()[0];
        assert_eq!((old.data_type(), new.data_type()), ("integer", "bigint"));
    }

    #[test]
    fn a_reformatted_view_is_not_a_change() {
        let target = Relation::new("v", RelationType::View).with_definition("SELECT 1\n  AS a");
        let source = Relation::new("v", RelationType::View).with_definition("SELECT 1 AS a");
        assert!(RelationChange::between(&target, &source).is_none());
    }

    #[test]
    fn a_relation_that_changes_type_is_dropped_and_created() {
        let target = Schema::new("s").with_relations([Relation::new("x", RelationType::Table)]);
        let source = Schema::new("s").with_relations([Relation::new("x", RelationType::View)]);
        let diff = diff_schemas(&target, &source);
        assert_eq!(diff.removed_relations().len(), 1);
        assert_eq!(diff.added_relations().len(), 1);
        assert!(diff.changed_relations().is_empty());
    }
}
