//! A way to name any object of any data source, and what DataKit can say
//! about it: its DDL, its documentation, its icon.
//!
//! The explorer, Search Everywhere, the console's hover and Go to
//! Declaration all speak in [`ObjectRef`]s, so "open", "show DDL" and
//! "quick documentation" mean the same thing wherever they are invoked.

use std::{fmt::Write as _, sync::Arc};

use datakit_catalog::{
    Catalog, Column, Constraint, ConstraintRule, Index, Relation, RelationType, Routine,
    RoutineType, Schema, Sequence, Trigger,
};
use datakit_driver::Dialect;
use gpui_kit::{App, Entity, SharedString, assets::IconName};
use rust_i18n::t;

use crate::datasource::DataSource;

/// Where an object is inside one data source's catalog.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ObjectPath {
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
    Index {
        schema: Arc<str>,
        relation: Arc<str>,
        index: Arc<str>,
    },
    Constraint {
        schema: Arc<str>,
        relation: Arc<str>,
        constraint: Arc<str>,
    },
    Trigger {
        schema: Arc<str>,
        relation: Arc<str>,
        trigger: Arc<str>,
    },
    Routine {
        schema: Arc<str>,
        /// `name(arguments)`: routines are told apart by their arguments.
        signature: Arc<str>,
    },
    Sequence {
        schema: Arc<str>,
        sequence: Arc<str>,
    },
}

impl ObjectPath {
    pub fn relation(schema: impl Into<Arc<str>>, relation: impl Into<Arc<str>>) -> Self {
        Self::Relation {
            schema: schema.into(),
            relation: relation.into(),
        }
    }

    pub fn schema(&self) -> &Arc<str> {
        match self {
            Self::Schema { schema }
            | Self::Relation { schema, .. }
            | Self::Column { schema, .. }
            | Self::Index { schema, .. }
            | Self::Constraint { schema, .. }
            | Self::Trigger { schema, .. }
            | Self::Routine { schema, .. }
            | Self::Sequence { schema, .. } => schema,
        }
    }

    /// The relation the object is, or belongs to.
    pub fn relation_name(&self) -> Option<&Arc<str>> {
        match self {
            Self::Relation { relation, .. }
            | Self::Column { relation, .. }
            | Self::Index { relation, .. }
            | Self::Constraint { relation, .. }
            | Self::Trigger { relation, .. } => Some(relation),
            _ => None,
        }
    }

    /// The object's own name, as the database spells it.
    pub fn name(&self) -> Arc<str> {
        match self {
            Self::Schema { schema } => schema.clone(),
            Self::Relation { relation, .. } => relation.clone(),
            Self::Column { column, .. } => column.clone(),
            Self::Index { index, .. } => index.clone(),
            Self::Constraint { constraint, .. } => constraint.clone(),
            Self::Trigger { trigger, .. } => trigger.clone(),
            Self::Routine { signature, .. } => signature.clone(),
            Self::Sequence { sequence, .. } => sequence.clone(),
        }
    }

    /// A stable string for tree ids and saved layouts.
    pub fn key(&self) -> String {
        match self {
            Self::Schema { schema } => format!("s/{schema}"),
            Self::Relation { schema, relation } => format!("s/{schema}/r/{relation}"),
            Self::Column {
                schema,
                relation,
                column,
            } => format!("s/{schema}/r/{relation}/c/{column}"),
            Self::Index {
                schema,
                relation,
                index,
            } => format!("s/{schema}/r/{relation}/i/{index}"),
            Self::Constraint {
                schema,
                relation,
                constraint,
            } => format!("s/{schema}/r/{relation}/k/{constraint}"),
            Self::Trigger {
                schema,
                relation,
                trigger,
            } => format!("s/{schema}/r/{relation}/t/{trigger}"),
            Self::Routine { schema, signature } => format!("s/{schema}/f/{signature}"),
            Self::Sequence { schema, sequence } => format!("s/{schema}/q/{sequence}"),
        }
    }

    /// The object in `catalog`, if it is loaded there.
    pub fn resolve<'a>(&self, catalog: &'a Catalog) -> Option<CatalogObject<'a>> {
        let schema = catalog.schema(self.schema())?;
        let relation = |name: &str| schema.relation(name);
        Some(match self {
            Self::Schema { .. } => CatalogObject::Schema(schema),
            Self::Relation { relation: name, .. } => CatalogObject::Relation(relation(name)?),
            Self::Column {
                relation: name,
                column,
                ..
            } => {
                let relation = relation(name)?;
                CatalogObject::Column(relation, relation.column(column)?)
            }
            Self::Index {
                relation: name,
                index,
                ..
            } => {
                let relation = relation(name)?;
                let index = relation
                    .indexes()
                    .iter()
                    .find(|candidate| &*candidate.name() == &**index)?;
                CatalogObject::Index(relation, index)
            }
            Self::Constraint {
                relation: name,
                constraint,
                ..
            } => {
                let relation = relation(name)?;
                let constraint = relation
                    .constraints()
                    .iter()
                    .find(|candidate| &*candidate.name() == &**constraint)?;
                CatalogObject::Constraint(relation, constraint)
            }
            Self::Trigger {
                relation: name,
                trigger,
                ..
            } => {
                let relation = relation(name)?;
                let trigger = relation
                    .triggers()
                    .iter()
                    .find(|candidate| &*candidate.name() == &**trigger)?;
                CatalogObject::Trigger(relation, trigger)
            }
            Self::Routine { signature, .. } => CatalogObject::Routine(
                schema
                    .routines()
                    .iter()
                    .find(|routine| routine.signature() == **signature)?,
            ),
            Self::Sequence { sequence, .. } => CatalogObject::Sequence(schema.sequence(sequence)?),
        })
    }
}

/// An object found in a catalog.
#[derive(Clone, Copy)]
pub enum CatalogObject<'a> {
    Schema(&'a Schema),
    Relation(&'a Relation),
    Column(&'a Relation, &'a Column),
    Index(&'a Relation, &'a Index),
    Constraint(&'a Relation, &'a Constraint),
    Trigger(&'a Relation, &'a Trigger),
    Routine(&'a Routine),
    Sequence(&'a Sequence),
}

/// An object of a particular data source.
#[derive(Clone)]
pub struct ObjectRef {
    data_source: Entity<DataSource>,
    path: ObjectPath,
}

impl PartialEq for ObjectRef {
    fn eq(&self, other: &Self) -> bool {
        self.data_source.entity_id() == other.data_source.entity_id() && self.path == other.path
    }
}

impl ObjectRef {
    pub fn new(data_source: Entity<DataSource>, path: ObjectPath) -> Self {
        Self { data_source, path }
    }

    pub fn data_source(&self) -> &Entity<DataSource> {
        &self.data_source
    }

    pub fn path(&self) -> &ObjectPath {
        &self.path
    }

    /// The statements that create the object, ready for a console.
    pub fn ddl(&self, cx: &App) -> Option<String> {
        let source = self.data_source.read(cx);
        let object = self.path.resolve(source.catalog())?;
        Some(ddl(&*source.dialect(), self.path.schema(), object))
    }

    /// A `SELECT` of the relation's rows, when the object is a relation.
    pub fn select(&self, limit: Option<u64>, cx: &App) -> Option<String> {
        let ObjectPath::Relation { schema, relation } = &self.path else {
            return None;
        };
        Some(
            self.data_source
                .read(cx)
                .dialect()
                .select_rows(schema, relation, limit),
        )
    }

    /// The name to type in SQL: qualified and quoted as needed.
    pub fn qualified_name(&self, cx: &App) -> String {
        let dialect = self.data_source.read(cx).dialect();
        match &self.path {
            ObjectPath::Schema { schema } => dialect.quote_identifier(schema),
            ObjectPath::Column { column, .. } => dialect.quote_identifier(column),
            ObjectPath::Routine { schema, signature } => {
                let name = signature.split('(').next().unwrap_or(signature);
                dialect.qualified_name(schema, name)
            }
            path => match path.relation_name() {
                Some(relation) if matches!(path, ObjectPath::Relation { .. }) => {
                    dialect.qualified_name(path.schema(), relation)
                }
                _ => dialect.qualified_name(path.schema(), &path.name()),
            },
        }
    }

    /// The statement that removes the object from the database.
    pub fn drop_statement(&self, cx: &App) -> Option<String> {
        let source = self.data_source.read(cx);
        let dialect = source.dialect();
        let schema = self.path.schema();
        Some(match self.path.resolve(source.catalog())? {
            CatalogObject::Schema(_) => {
                format!("DROP SCHEMA {}", dialect.quote_identifier(schema))
            }
            CatalogObject::Relation(relation) => dialect.drop_relation(schema, relation),
            CatalogObject::Column(relation, column) => format!(
                "ALTER TABLE {} DROP COLUMN {}",
                dialect.qualified_name(schema, &relation.name()),
                dialect.quote_identifier(&column.name())
            ),
            CatalogObject::Index(relation, index) => {
                dialect.drop_index(schema, &relation.name(), index)
            }
            CatalogObject::Constraint(relation, constraint) => format!(
                "ALTER TABLE {} DROP CONSTRAINT {}",
                dialect.qualified_name(schema, &relation.name()),
                dialect.quote_identifier(&constraint.name())
            ),
            CatalogObject::Trigger(relation, trigger) => {
                dialect.drop_trigger(schema, &relation.name(), trigger)
            }
            CatalogObject::Routine(routine) => dialect.drop_routine(schema, routine),
            CatalogObject::Sequence(sequence) => dialect.drop_sequence(schema, sequence),
        })
    }

    /// Markdown describing the object, for quick documentation.
    pub fn documentation(&self, cx: &App) -> Option<String> {
        let source = self.data_source.read(cx);
        let object = self.path.resolve(source.catalog())?;
        Some(documentation(
            &*source.dialect(),
            self.path.schema(),
            object,
            source.catalog(),
        ))
    }
}

/// The statements that create `object`, which is in `schema`.
pub fn ddl(dialect: &dyn Dialect, schema: &str, object: CatalogObject) -> String {
    let statements: Vec<String> = match object {
        CatalogObject::Schema(schema_object) => {
            let mut statements = vec![format!(
                "CREATE SCHEMA {}",
                dialect.quote_identifier(&schema_object.name())
            )];
            for sequence in schema_object.sequences() {
                statements.push(dialect.create_sequence(schema, sequence));
            }
            let relations = schema_object.relations().unwrap_or_default();
            // Tables before the views that read them.
            for relation in relations.iter().filter(|r| !r.relation_type().is_view()) {
                statements.extend(dialect.create_relation(schema, relation));
            }
            for relation in relations.iter().filter(|r| r.relation_type().is_view()) {
                statements.extend(dialect.create_relation(schema, relation));
            }
            for routine in schema_object.routines() {
                statements.extend(dialect.create_routine(schema, routine));
            }
            statements
        }
        CatalogObject::Relation(relation) => dialect.create_relation(schema, relation),
        CatalogObject::Column(relation, column) => {
            vec![dialect.add_column(schema, &relation.name(), column)]
        }
        CatalogObject::Index(relation, index) => {
            vec![dialect.create_index(schema, &relation.name(), index)]
        }
        CatalogObject::Constraint(relation, constraint) => vec![format!(
            "ALTER TABLE {} ADD {}",
            dialect.qualified_name(schema, &relation.name()),
            dialect.constraint_definition(constraint)
        )],
        CatalogObject::Trigger(relation, trigger) => dialect
            .create_trigger(schema, &relation.name(), trigger)
            .into_iter()
            .collect(),
        CatalogObject::Routine(routine) => dialect
            .create_routine(schema, routine)
            .into_iter()
            .collect(),
        CatalogObject::Sequence(sequence) => vec![dialect.create_sequence(schema, sequence)],
    };
    script(&statements)
}

/// Statements joined into a script, each ending with a semicolon.
pub fn script(statements: &[String]) -> String {
    let mut script = String::new();
    for statement in statements {
        let statement = statement.trim_end();
        script.push_str(statement);
        // A routine body may end with its own semicolon already.
        if !statement.ends_with(';') {
            script.push(';');
        }
        script.push_str("\n\n");
    }
    script.trim_end().to_string() + "\n"
}

fn documentation(
    dialect: &dyn Dialect,
    schema: &str,
    object: CatalogObject,
    catalog: &Catalog,
) -> String {
    let mut text = String::new();
    match object {
        CatalogObject::Schema(schema) => {
            let _ = writeln!(text, "**{}** · {}", schema.name(), t!("objects.schema"));
            if let Some(owner) = schema.owner() {
                let _ = writeln!(text, "\n{}: {owner}", t!("objects.owner"));
            }
            if let Some(comment) = schema.comment() {
                let _ = writeln!(text, "\n{comment}");
            }
        }
        CatalogObject::Relation(relation) => {
            let _ = writeln!(
                text,
                "**{}** · {}",
                dialect.qualified_name(schema, &relation.name()),
                relation_type_label(relation.relation_type())
            );
            if let Some(comment) = relation.comment() {
                let _ = writeln!(text, "\n{comment}");
            }
            let _ = writeln!(text);
            for column in relation.columns() {
                let _ = writeln!(text, "- {}", column_line(column));
            }
            let keys: Vec<String> = relation
                .foreign_keys()
                .map(|(_, key)| {
                    format!(
                        "- ({}) → {}.{} ({})",
                        key.columns().join(", "),
                        key.referenced_schema(),
                        key.referenced_relation(),
                        key.referenced_columns().join(", ")
                    )
                })
                .collect();
            if !keys.is_empty() {
                let _ = writeln!(text, "\n{}", t!("objects.foreign_keys"));
                for key in keys {
                    let _ = writeln!(text, "{key}");
                }
            }
            let referencing: Vec<String> = catalog
                .referencing(schema, &relation.name())
                .map(|(owner, relation, key)| {
                    format!(
                        "- {}.{} ({})",
                        owner.name(),
                        relation.name(),
                        key.columns().join(", ")
                    )
                })
                .collect();
            if !referencing.is_empty() {
                let _ = writeln!(text, "\n{}", t!("objects.referenced_by"));
                for line in referencing {
                    let _ = writeln!(text, "{line}");
                }
            }
        }
        CatalogObject::Column(relation, column) => {
            let _ = writeln!(
                text,
                "**{}.{}** · {}",
                relation.name(),
                column.name(),
                column.data_type()
            );
            let _ = writeln!(text, "\n{}", column_line(column));
            if let Some(comment) = column.comment() {
                let _ = writeln!(text, "\n{comment}");
            }
        }
        CatalogObject::Routine(routine) => {
            let _ = writeln!(
                text,
                "**{}** · {}",
                routine.signature(),
                routine_type_label(routine.routine_type())
            );
            if let Some(result) = routine.result() {
                let _ = writeln!(text, "\n{} {result}", t!("objects.returns"));
            }
            if let Some(language) = routine.language() {
                let _ = writeln!(text, "\n{}: {language}", t!("objects.language"));
            }
            if let Some(comment) = routine.comment() {
                let _ = writeln!(text, "\n{comment}");
            }
        }
        CatalogObject::Sequence(sequence) => {
            let _ = writeln!(text, "**{}** · {}", sequence.name(), t!("objects.sequence"));
            let _ = writeln!(
                text,
                "\n{} · start {} · increment {}",
                sequence.data_type(),
                sequence.start(),
                sequence.increment()
            );
            if let Some(owner) = sequence.owned_by() {
                let _ = writeln!(text, "\n{}: {owner}", t!("objects.owned_by"));
            }
        }
        CatalogObject::Index(_, index) => {
            let _ = writeln!(text, "**{}** · {}", index.name(), t!("objects.index"));
            let _ = writeln!(text, "\n({})", index.columns().join(", "));
        }
        CatalogObject::Constraint(_, constraint) => {
            let _ = writeln!(text, "**{}**", constraint.name());
            if let Some(definition) = constraint.definition() {
                let _ = writeln!(text, "\n`{definition}`");
            }
        }
        CatalogObject::Trigger(_, trigger) => {
            let _ = writeln!(text, "**{}** · {}", trigger.name(), trigger.timing());
        }
    }
    text
}

fn column_line(column: &Column) -> String {
    let mut line = format!("`{}` {}", column.name(), column.data_type());
    if column.is_primary_key() {
        line.push_str(" · PK");
    }
    if !column.is_nullable() {
        line.push_str(" · NOT NULL");
    }
    if let Some(default) = column.default() {
        let _ = write!(line, " · = {default}");
    }
    line
}

pub fn relation_type_label(relation_type: RelationType) -> SharedString {
    match relation_type {
        RelationType::Table => t!("objects.table"),
        RelationType::PartitionedTable => t!("objects.partitioned_table"),
        RelationType::ForeignTable => t!("objects.foreign_table"),
        RelationType::View => t!("objects.view"),
        RelationType::MaterializedView => t!("objects.materialized_view"),
    }
    .into()
}

pub fn routine_type_label(routine_type: RoutineType) -> SharedString {
    match routine_type {
        RoutineType::Function => t!("objects.function"),
        RoutineType::Procedure => t!("objects.procedure"),
        RoutineType::Aggregate => t!("objects.aggregate"),
        RoutineType::Window => t!("objects.window_function"),
    }
    .into()
}

pub fn relation_icon(relation_type: RelationType) -> IconName {
    match relation_type {
        RelationType::View => IconName::Eye,
        RelationType::MaterializedView => IconName::Layers,
        RelationType::ForeignTable => IconName::Link,
        _ => IconName::Table,
    }
}

pub fn constraint_icon(constraint: &Constraint) -> IconName {
    match constraint.rule() {
        ConstraintRule::PrimaryKey { .. } => IconName::Key,
        ConstraintRule::Unique { .. } => IconName::KeyRound,
        ConstraintRule::ForeignKey(_) => IconName::Link,
        ConstraintRule::Check { .. } | ConstraintRule::Exclusion => IconName::CircleCheck,
    }
}
