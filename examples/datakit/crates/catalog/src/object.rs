use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// A namespace of objects: a PostgreSQL schema, a MySQL database, the `main`
/// database of SQLite.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    name: Arc<str>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<Arc<str>>,
    #[serde(default)]
    system: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
    /// `None` until the schema's contents have been introspected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relations: Option<Arc<[Relation]>>,
    #[serde(default, skip_serializing_if = "slice_is_empty")]
    routines: Arc<[Routine]>,
    #[serde(default, skip_serializing_if = "slice_is_empty")]
    sequences: Arc<[Sequence]>,
}

impl Schema {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            owner: None,
            system: false,
            comment: None,
            relations: None,
            routines: Arc::from([]),
            sequences: Arc::from([]),
        }
    }

    /// Mark the schema as one the database itself maintains
    /// (`pg_catalog`, `information_schema`).
    pub fn system(mut self, system: bool) -> Self {
        self.system = system;
        self
    }

    pub fn with_owner(mut self, owner: impl Into<Arc<str>>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_comment(mut self, comment: impl Into<Arc<str>>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// The schema's relations, which also marks its contents as loaded.
    pub fn with_relations(mut self, relations: impl IntoIterator<Item = Relation>) -> Self {
        let mut relations: Vec<Relation> = relations.into_iter().collect();
        relations.sort_by(|a, b| a.name.cmp(&b.name));
        self.relations = Some(relations.into());
        self
    }

    pub fn with_routines(mut self, routines: impl IntoIterator<Item = Routine>) -> Self {
        let mut routines: Vec<Routine> = routines.into_iter().collect();
        routines.sort_by(|a, b| (&a.name, &a.arguments).cmp(&(&b.name, &b.arguments)));
        self.routines = routines.into();
        self
    }

    pub fn with_sequences(mut self, sequences: impl IntoIterator<Item = Sequence>) -> Self {
        let mut sequences: Vec<Sequence> = sequences.into_iter().collect();
        sequences.sort_by(|a, b| a.name.cmp(&b.name));
        self.sequences = sequences.into();
        self
    }

    /// The same schema with its contents forgotten, so the next read
    /// introspects them again.
    pub(crate) fn unloaded(mut self) -> Self {
        self.relations = None;
        self.routines = Arc::from([]);
        self.sequences = Arc::from([]);
        self
    }

    /// The same schema with the contents of `loaded`, keeping what this one
    /// knows about the schema itself when `loaded` does not say.
    pub(crate) fn with_contents_of(mut self, loaded: &Schema) -> Self {
        self.relations = loaded.relations.clone();
        self.routines = loaded.routines.clone();
        self.sequences = loaded.sequences.clone();
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn is_system(&self) -> bool {
        self.system
    }

    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    /// Whether the schema's contents have been introspected.
    pub fn is_loaded(&self) -> bool {
        self.relations.is_some()
    }

    /// The schema's relations sorted by name, or `None` before they have been
    /// introspected. An empty slice means the schema really is empty.
    pub fn relations(&self) -> Option<&[Relation]> {
        self.relations.as_deref()
    }

    pub fn relation(&self, name: &str) -> Option<&Relation> {
        let relations = self.relations.as_deref()?;
        relations
            .binary_search_by(|relation| (*relation.name).cmp(name))
            .ok()
            .map(|ix| &relations[ix])
    }

    /// Functions and procedures, sorted by name and then by signature.
    pub fn routines(&self) -> &[Routine] {
        &self.routines
    }

    /// Every overload of the routine `name`.
    pub fn routines_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Routine> + 'a {
        self.routines
            .iter()
            .filter(move |routine| &*routine.name == name)
    }

    pub fn sequences(&self) -> &[Sequence] {
        &self.sequences
    }

    pub fn sequence(&self, name: &str) -> Option<&Sequence> {
        self.sequences
            .iter()
            .find(|sequence| &*sequence.name == name)
    }
}

/// What a relation is. The variants share columns and can all be queried;
/// they differ in what can be written and how they are created.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RelationType {
    Table,
    PartitionedTable,
    ForeignTable,
    View,
    MaterializedView,
    /// A key of a key-value store such as Redis: a value of some type,
    /// shown as rows but not a table.
    Key,
}

impl RelationType {
    pub fn is_table(self) -> bool {
        matches!(
            self,
            Self::Table | Self::PartitionedTable | Self::ForeignTable
        )
    }

    pub fn is_view(self) -> bool {
        matches!(self, Self::View | Self::MaterializedView)
    }
}

/// A table, view or anything else that has columns and can be selected from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relation {
    name: Arc<str>,
    relation_type: RelationType,
    columns: Arc<[Column]>,
    #[serde(default, skip_serializing_if = "slice_is_empty")]
    indexes: Arc<[Index]>,
    #[serde(default, skip_serializing_if = "slice_is_empty")]
    constraints: Arc<[Constraint]>,
    #[serde(default, skip_serializing_if = "slice_is_empty")]
    triggers: Arc<[Trigger]>,
    /// The query of a view, as the database prints it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    estimated_rows: Option<i64>,
}

impl Relation {
    pub fn new(name: impl Into<Arc<str>>, relation_type: RelationType) -> Self {
        Self {
            name: name.into(),
            relation_type,
            columns: Arc::from([]),
            indexes: Arc::from([]),
            constraints: Arc::from([]),
            triggers: Arc::from([]),
            definition: None,
            owner: None,
            comment: None,
            estimated_rows: None,
        }
    }

    /// The relation's columns, in their declared order.
    pub fn with_columns(mut self, columns: impl IntoIterator<Item = Column>) -> Self {
        self.columns = columns.into_iter().collect();
        self
    }

    pub fn with_indexes(mut self, indexes: impl IntoIterator<Item = Index>) -> Self {
        self.indexes = indexes.into_iter().collect();
        self
    }

    pub fn with_constraints(mut self, constraints: impl IntoIterator<Item = Constraint>) -> Self {
        self.constraints = constraints.into_iter().collect();
        self
    }

    pub fn with_triggers(mut self, triggers: impl IntoIterator<Item = Trigger>) -> Self {
        self.triggers = triggers.into_iter().collect();
        self
    }

    /// The query of a view.
    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn with_owner(mut self, owner: impl Into<Arc<str>>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    pub fn with_comment(mut self, comment: impl Into<Arc<str>>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// The planner's row estimate, when the database keeps one. It is a
    /// statistic, not a count, and is never shown as one.
    pub fn with_estimated_rows(mut self, rows: i64) -> Self {
        self.estimated_rows = Some(rows);
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn relation_type(&self) -> RelationType {
        self.relation_type
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| &*column.name == name)
    }

    pub fn indexes(&self) -> &[Index] {
        &self.indexes
    }

    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    pub fn triggers(&self) -> &[Trigger] {
        &self.triggers
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    pub fn estimated_rows(&self) -> Option<i64> {
        self.estimated_rows
    }

    /// The columns of the primary key, in key order.
    ///
    /// The primary key constraint decides the order when the relation has
    /// one; otherwise the columns marked as part of it, in table order.
    pub fn primary_key(&self) -> Vec<&Column> {
        match self
            .constraints
            .iter()
            .find_map(|constraint| match constraint.rule() {
                ConstraintRule::PrimaryKey { columns } => Some(columns),
                _ => None,
            }) {
            Some(columns) => columns
                .iter()
                .filter_map(|name| self.column(name))
                .collect(),
            None => self
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .collect(),
        }
    }

    /// The foreign keys of this relation.
    pub fn foreign_keys(&self) -> impl Iterator<Item = (&Constraint, &ForeignKey)> {
        self.constraints
            .iter()
            .filter_map(|constraint| match constraint.rule() {
                ConstraintRule::ForeignKey(key) => Some((constraint, key)),
                _ => None,
            })
    }

    /// Columns that identify one row: the primary key, or else the columns
    /// of the first unique constraint whose columns are all `NOT NULL`.
    pub fn row_identity(&self) -> Vec<&Column> {
        let primary_key = self.primary_key();
        if !primary_key.is_empty() {
            return primary_key;
        }
        self.constraints
            .iter()
            .find_map(|constraint| match constraint.rule() {
                ConstraintRule::Unique { columns } => {
                    let columns: Vec<&Column> = columns
                        .iter()
                        .filter_map(|name| self.column(name))
                        .collect();
                    columns
                        .iter()
                        .all(|column| !column.is_nullable())
                        .then_some(columns)
                }
                _ => None,
            })
            .unwrap_or_default()
    }
}

/// One column of a [`Relation`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    name: Arc<str>,
    data_type: Arc<str>,
    #[serde(default)]
    nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<Arc<str>>,
    #[serde(default)]
    primary_key: bool,
    /// Whether the database computes the value (`GENERATED ALWAYS AS`).
    #[serde(default)]
    generated: bool,
    /// Whether the database assigns the value (`serial`, `IDENTITY`,
    /// `AUTO_INCREMENT`).
    #[serde(default)]
    auto_increment: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
}

impl Column {
    /// A column named `name` of `data_type`, spelled as the database spells
    /// it (`character varying(255)`, `timestamp with time zone`).
    pub fn new(name: impl Into<Arc<str>>, data_type: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: false,
            default: None,
            primary_key: false,
            generated: false,
            auto_increment: false,
            comment: None,
        }
    }

    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    pub fn primary_key(mut self, primary_key: bool) -> Self {
        self.primary_key = primary_key;
        self
    }

    pub fn generated(mut self, generated: bool) -> Self {
        self.generated = generated;
        self
    }

    pub fn auto_increment(mut self, auto_increment: bool) -> Self {
        self.auto_increment = auto_increment;
        self
    }

    /// The default expression, as SQL text. For a generated column, the
    /// expression it is computed from.
    pub fn with_default(mut self, default: impl Into<Arc<str>>) -> Self {
        self.default = Some(default.into());
        self
    }

    pub fn with_comment(mut self, comment: impl Into<Arc<str>>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    /// The same column with `data_type`.
    pub fn with_data_type(mut self, data_type: impl Into<Arc<str>>) -> Self {
        self.data_type = data_type.into();
        self
    }

    /// The same column called `name`.
    pub fn with_name(mut self, name: impl Into<Arc<str>>) -> Self {
        self.name = name.into();
        self
    }

    /// The same column without a default.
    pub fn without_default(mut self) -> Self {
        self.default = None;
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn data_type(&self) -> &str {
        &self.data_type
    }

    pub fn is_nullable(&self) -> bool {
        self.nullable
    }

    pub fn is_primary_key(&self) -> bool {
        self.primary_key
    }

    pub fn is_generated(&self) -> bool {
        self.generated
    }

    pub fn is_auto_increment(&self) -> bool {
        self.auto_increment
    }

    pub fn default(&self) -> Option<&str> {
        self.default.as_deref()
    }

    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }
}

/// An index of a relation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Index {
    name: Arc<str>,
    /// The indexed columns or expressions, in key order.
    columns: Arc<[Arc<str>]>,
    #[serde(default)]
    unique: bool,
    /// Whether the index enforces the primary key.
    #[serde(default)]
    primary: bool,
    /// The access method (`btree`, `gin`), when the database has several.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    method: Option<Arc<str>>,
    /// The partial index's `WHERE` condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    predicate: Option<Arc<str>>,
    /// The statement that creates the index, as the database prints it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
}

impl Index {
    pub fn new(
        name: impl Into<Arc<str>>,
        columns: impl IntoIterator<Item = impl Into<Arc<str>>>,
    ) -> Self {
        Self {
            name: name.into(),
            columns: columns.into_iter().map(Into::into).collect(),
            unique: false,
            primary: false,
            method: None,
            predicate: None,
            definition: None,
        }
    }

    pub fn unique(mut self, unique: bool) -> Self {
        self.unique = unique;
        self
    }

    pub fn primary(mut self, primary: bool) -> Self {
        self.primary = primary;
        self
    }

    pub fn with_method(mut self, method: impl Into<Arc<str>>) -> Self {
        self.method = Some(method.into());
        self
    }

    pub fn with_predicate(mut self, predicate: impl Into<Arc<str>>) -> Self {
        self.predicate = Some(predicate.into());
        self
    }

    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn columns(&self) -> &[Arc<str>] {
        &self.columns
    }

    pub fn is_unique(&self) -> bool {
        self.unique
    }

    pub fn is_primary(&self) -> bool {
        self.primary
    }

    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    pub fn predicate(&self) -> Option<&str> {
        self.predicate.as_deref()
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }
}

/// A named rule a relation's rows must follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    name: Arc<str>,
    rule: ConstraintRule,
    /// The constraint as it appears in `CREATE TABLE`, after its name, as
    /// the database prints it (`FOREIGN KEY (a) REFERENCES t(id)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
}

impl Constraint {
    pub fn new(name: impl Into<Arc<str>>, rule: ConstraintRule) -> Self {
        Self {
            name: name.into(),
            rule,
            definition: None,
        }
    }

    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn rule(&self) -> &ConstraintRule {
        &self.rule
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }
}

/// What a [`Constraint`] requires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "rule")]
pub enum ConstraintRule {
    PrimaryKey {
        columns: Arc<[Arc<str>]>,
    },
    Unique {
        columns: Arc<[Arc<str>]>,
    },
    ForeignKey(ForeignKey),
    Check {
        expression: Arc<str>,
    },
    /// PostgreSQL's `EXCLUDE`; the definition says how.
    Exclusion,
}

impl ConstraintRule {
    /// The columns the rule is about, when it names columns.
    pub fn columns(&self) -> &[Arc<str>] {
        match self {
            Self::PrimaryKey { columns } | Self::Unique { columns } => columns,
            Self::ForeignKey(key) => key.columns(),
            Self::Check { .. } | Self::Exclusion => &[],
        }
    }
}

/// A reference from some columns to the key of another relation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ForeignKey {
    columns: Arc<[Arc<str>]>,
    referenced_schema: Arc<str>,
    referenced_relation: Arc<str>,
    referenced_columns: Arc<[Arc<str>]>,
    #[serde(default)]
    on_update: ReferentialAction,
    #[serde(default)]
    on_delete: ReferentialAction,
}

impl ForeignKey {
    pub fn new(
        columns: impl IntoIterator<Item = impl Into<Arc<str>>>,
        referenced_schema: impl Into<Arc<str>>,
        referenced_relation: impl Into<Arc<str>>,
        referenced_columns: impl IntoIterator<Item = impl Into<Arc<str>>>,
    ) -> Self {
        Self {
            columns: columns.into_iter().map(Into::into).collect(),
            referenced_schema: referenced_schema.into(),
            referenced_relation: referenced_relation.into(),
            referenced_columns: referenced_columns.into_iter().map(Into::into).collect(),
            on_update: ReferentialAction::default(),
            on_delete: ReferentialAction::default(),
        }
    }

    pub fn with_on_update(mut self, action: ReferentialAction) -> Self {
        self.on_update = action;
        self
    }

    pub fn with_on_delete(mut self, action: ReferentialAction) -> Self {
        self.on_delete = action;
        self
    }

    pub fn columns(&self) -> &[Arc<str>] {
        &self.columns
    }

    pub fn referenced_schema(&self) -> &str {
        &self.referenced_schema
    }

    pub fn referenced_relation(&self) -> &str {
        &self.referenced_relation
    }

    pub fn referenced_columns(&self) -> &[Arc<str>] {
        &self.referenced_columns
    }

    pub fn on_update(&self) -> ReferentialAction {
        self.on_update
    }

    pub fn on_delete(&self) -> ReferentialAction {
        self.on_delete
    }
}

/// What happens to referencing rows when the referenced row changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferentialAction {
    #[default]
    NoAction,
    Restrict,
    Cascade,
    SetNull,
    SetDefault,
}

impl ReferentialAction {
    pub const ALL: [ReferentialAction; 5] = [
        Self::NoAction,
        Self::Restrict,
        Self::Cascade,
        Self::SetNull,
        Self::SetDefault,
    ];

    /// The action as SQL spells it.
    pub fn sql(self) -> &'static str {
        match self {
            Self::NoAction => "NO ACTION",
            Self::Restrict => "RESTRICT",
            Self::Cascade => "CASCADE",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
        }
    }
}

/// A trigger on a relation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    name: Arc<str>,
    /// When it fires, as SQL says it: `BEFORE INSERT OR UPDATE`.
    timing: Arc<str>,
    #[serde(default = "enabled")]
    enabled: bool,
    /// The statement that creates the trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
}

fn enabled() -> bool {
    true
}

impl Trigger {
    pub fn new(name: impl Into<Arc<str>>, timing: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            timing: timing.into(),
            enabled: true,
            definition: None,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn timing(&self) -> &str {
        &self.timing
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }
}

/// What a [`Routine`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoutineType {
    Function,
    Procedure,
    Aggregate,
    Window,
}

/// A function or procedure stored in the database.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    name: Arc<str>,
    routine_type: RoutineType,
    /// The argument list, as the database prints it: `a integer, b text`.
    arguments: Arc<str>,
    /// What it returns, for a function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    result: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    language: Option<Arc<str>>,
    /// The statement that creates it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    definition: Option<Arc<str>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    comment: Option<Arc<str>>,
}

impl Routine {
    pub fn new(
        name: impl Into<Arc<str>>,
        routine_type: RoutineType,
        arguments: impl Into<Arc<str>>,
    ) -> Self {
        Self {
            name: name.into(),
            routine_type,
            arguments: arguments.into(),
            result: None,
            language: None,
            definition: None,
            comment: None,
        }
    }

    pub fn with_result(mut self, result: impl Into<Arc<str>>) -> Self {
        self.result = Some(result.into());
        self
    }

    pub fn with_language(mut self, language: impl Into<Arc<str>>) -> Self {
        self.language = Some(language.into());
        self
    }

    pub fn with_definition(mut self, definition: impl Into<Arc<str>>) -> Self {
        self.definition = Some(definition.into());
        self
    }

    pub fn with_comment(mut self, comment: impl Into<Arc<str>>) -> Self {
        self.comment = Some(comment.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn routine_type(&self) -> RoutineType {
        self.routine_type
    }

    pub fn arguments(&self) -> &str {
        &self.arguments
    }

    /// `name(arguments)`, which tells overloads apart.
    pub fn signature(&self) -> String {
        format!("{}({})", self.name, self.arguments)
    }

    pub fn result(&self) -> Option<&str> {
        self.result.as_deref()
    }

    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }

    pub fn definition(&self) -> Option<&str> {
        self.definition.as_deref()
    }

    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }
}

/// A sequence generator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    name: Arc<str>,
    data_type: Arc<str>,
    start: i64,
    increment: i64,
    min_value: i64,
    max_value: i64,
    #[serde(default)]
    cycle: bool,
    /// `table.column` when the sequence belongs to a column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owned_by: Option<Arc<str>>,
}

impl Sequence {
    pub fn new(name: impl Into<Arc<str>>, data_type: impl Into<Arc<str>>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            start: 1,
            increment: 1,
            min_value: 1,
            max_value: i64::MAX,
            cycle: false,
            owned_by: None,
        }
    }

    pub fn with_start(mut self, start: i64) -> Self {
        self.start = start;
        self
    }

    pub fn with_increment(mut self, increment: i64) -> Self {
        self.increment = increment;
        self
    }

    pub fn with_range(mut self, min_value: i64, max_value: i64) -> Self {
        self.min_value = min_value;
        self.max_value = max_value;
        self
    }

    pub fn cycle(mut self, cycle: bool) -> Self {
        self.cycle = cycle;
        self
    }

    pub fn with_owned_by(mut self, owned_by: impl Into<Arc<str>>) -> Self {
        self.owned_by = Some(owned_by.into());
        self
    }

    pub fn name(&self) -> Arc<str> {
        self.name.clone()
    }

    pub fn data_type(&self) -> &str {
        &self.data_type
    }

    pub fn start(&self) -> i64 {
        self.start
    }

    pub fn increment(&self) -> i64 {
        self.increment
    }

    pub fn min_value(&self) -> i64 {
        self.min_value
    }

    pub fn max_value(&self) -> i64 {
        self.max_value
    }

    pub fn is_cycle(&self) -> bool {
        self.cycle
    }

    pub fn owned_by(&self) -> Option<&str> {
        self.owned_by.as_deref()
    }
}

fn slice_is_empty<T>(slice: &Arc<[T]>) -> bool {
    slice.is_empty()
}
