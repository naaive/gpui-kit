//! A table as the designer edits it, and the statements that make the
//! database match.
//!
//! Each column, index and foreign key remembers what it was called when the
//! designer opened, so a renamed column is renamed — its data kept — rather
//! than dropped and added again.

use std::sync::Arc;

use datakit_catalog::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationChange, RelationType,
};
use datakit_driver::Dialect;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColumnDraft {
    /// The column's name when the designer opened; `None` for a new one.
    pub original: Option<Arc<str>>,
    pub name: String,
    pub data_type: String,
    pub not_null: bool,
    pub default: String,
    pub comment: String,
    pub primary_key: bool,
    /// Kept as they were: the designer does not edit them.
    pub auto_increment: bool,
    pub generated: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexDraft {
    pub original: Option<Index>,
    pub name: String,
    /// Column names or expressions, separated by commas.
    pub columns: String,
    pub unique: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ForeignKeyDraft {
    pub original: Option<Constraint>,
    pub name: String,
    pub columns: String,
    /// `schema.relation`, or a relation of the table's own schema.
    pub referenced: String,
    pub referenced_columns: String,
    pub on_delete: ReferentialAction,
    pub on_update: ReferentialAction,
}

/// Why the draft cannot become statements yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftProblem {
    NoName,
    NoColumns,
    ColumnWithoutName(usize),
    ColumnWithoutType(String),
    DuplicateColumn(String),
    IndexWithoutColumns(String),
    ForeignKeyIncomplete(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableDraft {
    /// The relation when the designer opened; `None` for a new table.
    pub original: Option<Relation>,
    pub name: String,
    pub comment: String,
    pub columns: Vec<ColumnDraft>,
    pub indexes: Vec<IndexDraft>,
    pub foreign_keys: Vec<ForeignKeyDraft>,
}

fn list(text: &str) -> Vec<Arc<str>> {
    text.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(Arc::from)
        .collect()
}

impl TableDraft {
    /// A new table with an `id` key to start from.
    pub fn new_table(id_type: &str) -> Self {
        Self {
            columns: vec![ColumnDraft {
                name: "id".into(),
                data_type: id_type.into(),
                not_null: true,
                primary_key: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// The draft of an existing relation.
    pub fn from_relation(relation: &Relation) -> Self {
        let primary_key: Vec<Arc<str>> = relation
            .primary_key()
            .iter()
            .map(|column| column.name())
            .collect();
        let columns = relation
            .columns()
            .iter()
            .map(|column| ColumnDraft {
                original: Some(column.name()),
                name: column.name().to_string(),
                data_type: column.data_type().to_string(),
                not_null: !column.is_nullable(),
                default: column.default().unwrap_or_default().to_string(),
                comment: column.comment().unwrap_or_default().to_string(),
                primary_key: primary_key.contains(&column.name()),
                auto_increment: column.is_auto_increment(),
                generated: column.is_generated(),
            })
            .collect();
        let indexes = relation
            .indexes()
            .iter()
            .filter(|index| !index.is_primary() && !backs_constraint(relation, index))
            .map(|index| IndexDraft {
                original: Some(index.clone()),
                name: index.name().to_string(),
                columns: index.columns().join(", "),
                unique: index.is_unique(),
            })
            .collect();
        let foreign_keys = relation
            .foreign_keys()
            .map(|(constraint, key)| ForeignKeyDraft {
                original: Some(constraint.clone()),
                name: constraint.name().to_string(),
                columns: key.columns().join(", "),
                referenced: format!("{}.{}", key.referenced_schema(), key.referenced_relation()),
                referenced_columns: key.referenced_columns().join(", "),
                on_delete: key.on_delete(),
                on_update: key.on_update(),
            })
            .collect();
        Self {
            original: Some(relation.clone()),
            name: relation.name().to_string(),
            comment: relation.comment().unwrap_or_default().to_string(),
            columns,
            indexes,
            foreign_keys,
        }
    }

    fn check(&self) -> Result<(), DraftProblem> {
        if self.name.trim().is_empty() {
            return Err(DraftProblem::NoName);
        }
        if self.columns.is_empty() {
            return Err(DraftProblem::NoColumns);
        }
        let mut seen = std::collections::HashSet::new();
        for (ix, column) in self.columns.iter().enumerate() {
            let name = column.name.trim();
            if name.is_empty() {
                return Err(DraftProblem::ColumnWithoutName(ix + 1));
            }
            if column.data_type.trim().is_empty() {
                return Err(DraftProblem::ColumnWithoutType(name.into()));
            }
            if !seen.insert(name.to_string()) {
                return Err(DraftProblem::DuplicateColumn(name.into()));
            }
        }
        for index in &self.indexes {
            if list(&index.columns).is_empty() {
                return Err(DraftProblem::IndexWithoutColumns(index.name.clone()));
            }
        }
        for key in &self.foreign_keys {
            if list(&key.columns).is_empty()
                || key.referenced.trim().is_empty()
                || list(&key.referenced_columns).is_empty()
            {
                return Err(DraftProblem::ForeignKeyIncomplete(key.name.clone()));
            }
        }
        Ok(())
    }

    /// The relation the draft describes, named `name`, in `schema`.
    fn relation(&self, schema: &str, name: &str) -> Relation {
        let table = name.trim();
        let columns: Vec<Column> = self
            .columns
            .iter()
            .map(|draft| {
                let mut column = Column::new(draft.name.trim(), draft.data_type.trim())
                    .nullable(!draft.not_null && !draft.primary_key)
                    .primary_key(draft.primary_key)
                    .auto_increment(draft.auto_increment)
                    .generated(draft.generated);
                if !draft.default.trim().is_empty() {
                    column = column.with_default(draft.default.trim());
                }
                if !draft.comment.trim().is_empty() {
                    column = column.with_comment(draft.comment.trim());
                }
                column
            })
            .collect();

        let original = self.original.as_ref();
        let mut constraints = Vec::new();
        let key: Arc<[Arc<str>]> = self
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| Arc::from(column.name.trim()))
            .collect();
        if !key.is_empty() {
            // The key keeps its name, so the database's own index stays.
            let name = original
                .and_then(|relation| {
                    relation.constraints().iter().find(|constraint| {
                        matches!(constraint.rule(), ConstraintRule::PrimaryKey { .. })
                    })
                })
                .map(|constraint| constraint.name().to_string())
                .unwrap_or_else(|| format!("{table}_pkey"));
            constraints.push(Constraint::new(
                name,
                ConstraintRule::PrimaryKey { columns: key },
            ));
        }
        // Unique and check constraints are kept as they were.
        if let Some(original) = original {
            constraints.extend(
                original
                    .constraints()
                    .iter()
                    .filter(|constraint| {
                        !matches!(
                            constraint.rule(),
                            ConstraintRule::PrimaryKey { .. } | ConstraintRule::ForeignKey(_)
                        )
                    })
                    .map(|constraint| rename_in_constraint(constraint, &self.renames())),
            );
        }
        for draft in &self.foreign_keys {
            let (referenced_schema, referenced) = match draft.referenced.trim().split_once('.') {
                Some((schema, relation)) => (schema.to_string(), relation.to_string()),
                None => (schema.to_string(), draft.referenced.trim().to_string()),
            };
            let rule = ConstraintRule::ForeignKey(
                ForeignKey::new(
                    list(&draft.columns),
                    referenced_schema,
                    referenced,
                    list(&draft.referenced_columns),
                )
                .with_on_delete(draft.on_delete)
                .with_on_update(draft.on_update),
            );
            let name = if draft.name.trim().is_empty() {
                format!("{table}_{}_fkey", list(&draft.columns).join("_"))
            } else {
                draft.name.trim().to_string()
            };
            // An untouched key keeps the database's own definition.
            let unchanged = draft
                .original
                .as_ref()
                .filter(|original| original.rule() == &rule && *original.name() == *name);
            constraints.push(match unchanged {
                Some(original) => original.clone(),
                None => Constraint::new(name, rule),
            });
        }

        let mut indexes: Vec<Index> = original
            .map(|relation| {
                relation
                    .indexes()
                    .iter()
                    .filter(|index| index.is_primary() || backs_constraint(relation, index))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for draft in &self.indexes {
            let columns = list(&draft.columns);
            let unchanged = draft.original.as_ref().filter(|original| {
                *original.name() == *draft.name.trim()
                    && original.columns() == &columns[..]
                    && original.is_unique() == draft.unique
            });
            indexes.push(match unchanged {
                Some(original) => original.clone(),
                None => {
                    let name = if draft.name.trim().is_empty() {
                        format!(
                            "{table}_{}_idx",
                            columns
                                .iter()
                                .map(|c| c.replace(|ch: char| !ch.is_alphanumeric(), "_"))
                                .collect::<Vec<_>>()
                                .join("_")
                        )
                    } else {
                        draft.name.trim().to_string()
                    };
                    Index::new(name, columns.iter().cloned()).unique(draft.unique)
                }
            });
        }

        let mut relation = Relation::new(table, RelationType::Table)
            .with_columns(columns)
            .with_constraints(constraints)
            .with_indexes(indexes);
        if let Some(original) = original {
            relation = relation.with_triggers(original.triggers().iter().cloned());
        }
        if !self.comment.trim().is_empty() {
            relation = relation.with_comment(self.comment.trim());
        }
        let _ = schema;
        relation
    }

    /// `(old, new)` names of the columns renamed.
    fn renames(&self) -> Vec<(Arc<str>, Arc<str>)> {
        self.columns
            .iter()
            .filter_map(|column| {
                let original = column.original.clone()?;
                let name: Arc<str> = column.name.trim().into();
                (original != name).then_some((original, name))
            })
            .collect()
    }

    /// The statements that create the table, or change it into the draft.
    pub fn statements(
        &self,
        dialect: &dyn Dialect,
        schema: &str,
    ) -> Result<Vec<String>, DraftProblem> {
        self.check()?;
        let Some(original) = &self.original else {
            return Ok(dialect.create_relation(schema, &self.relation(schema, &self.name)));
        };
        let table = original.name();
        let renames = self.renames();
        let mut statements: Vec<String> = renames
            .iter()
            .map(|(old, new)| dialect.rename_column(schema, &table, old, new))
            .collect();
        // Compare with the table as it is once the renames are done.
        let renamed = original
            .clone()
            .with_columns(original.columns().iter().map(|column| {
                match renames.iter().find(|(old, _)| *old == column.name()) {
                    Some((_, new)) => column.clone().with_name(new.clone()),
                    None => column.clone(),
                }
            }))
            .with_constraints(
                original
                    .constraints()
                    .iter()
                    .map(|constraint| rename_in_constraint(constraint, &renames)),
            );
        let edited = self.relation(schema, &table);
        if let Some(change) = RelationChange::between(&renamed, &edited) {
            statements.extend(dialect.alter_relation(schema, &change));
        }
        if self.name.trim() != &*table {
            statements.push(dialect.rename_relation(schema, original, self.name.trim()));
        }
        Ok(statements)
    }
}

/// Whether `index` is the index a constraint of `relation` created.
fn backs_constraint(relation: &Relation, index: &Index) -> bool {
    relation
        .constraints()
        .iter()
        .any(|constraint| constraint.name() == index.name())
}

/// `constraint` with renamed columns in its rule. Its printed definition
/// names the old columns, so it is dropped when anything was renamed.
fn rename_in_constraint(constraint: &Constraint, renames: &[(Arc<str>, Arc<str>)]) -> Constraint {
    let rename = |columns: &[Arc<str>]| -> Arc<[Arc<str>]> {
        columns
            .iter()
            .map(|column| {
                renames
                    .iter()
                    .find(|(old, _)| old == column)
                    .map_or(column.clone(), |(_, new)| new.clone())
            })
            .collect()
    };
    let touched = constraint
        .rule()
        .columns()
        .iter()
        .any(|column| renames.iter().any(|(old, _)| old == column));
    if !touched {
        return constraint.clone();
    }
    let rule = match constraint.rule() {
        ConstraintRule::PrimaryKey { columns } => ConstraintRule::PrimaryKey {
            columns: rename(columns),
        },
        ConstraintRule::Unique { columns } => ConstraintRule::Unique {
            columns: rename(columns),
        },
        ConstraintRule::ForeignKey(key) => ConstraintRule::ForeignKey(
            ForeignKey::new(
                rename(key.columns()).iter().cloned(),
                key.referenced_schema(),
                key.referenced_relation(),
                key.referenced_columns().iter().cloned(),
            )
            .with_on_delete(key.on_delete())
            .with_on_update(key.on_update()),
        ),
        rule => rule.clone(),
    };
    Constraint::new(constraint.name(), rule)
}

#[cfg(test)]
mod tests {
    use datakit_driver_postgres::PostgresDialect;

    use super::*;

    fn orders() -> Relation {
        Relation::new("orders", RelationType::Table)
            .with_columns([
                Column::new("id", "bigint").primary_key(true),
                Column::new("total", "numeric").nullable(true),
            ])
            .with_constraints([Constraint::new(
                "orders_pkey",
                ConstraintRule::PrimaryKey {
                    columns: vec!["id".into()].into(),
                },
            )
            .with_definition("PRIMARY KEY (id)")])
            .with_indexes([Index::new("orders_pkey", ["id"]).unique(true).primary(true)])
    }

    #[test]
    fn an_untouched_table_needs_no_statements() {
        let draft = TableDraft::from_relation(&orders());
        assert_eq!(draft.statements(&PostgresDialect, "shop"), Ok(vec![]));
    }

    #[test]
    fn a_renamed_column_is_renamed_and_then_altered() {
        let mut draft = TableDraft::from_relation(&orders());
        draft.columns[1].name = "amount".into();
        draft.columns[1].not_null = true;
        draft.columns.push(ColumnDraft {
            name: "note".into(),
            data_type: "text".into(),
            ..Default::default()
        });
        assert_eq!(
            draft.statements(&PostgresDialect, "shop").unwrap(),
            vec![
                "ALTER TABLE shop.orders RENAME COLUMN total TO amount",
                "ALTER TABLE shop.orders ADD COLUMN note text",
                "ALTER TABLE shop.orders ALTER COLUMN amount SET NOT NULL",
            ]
        );
    }

    #[test]
    fn renaming_a_key_column_keeps_the_key() {
        let mut draft = TableDraft::from_relation(&orders());
        draft.columns[0].name = "order_id".into();
        assert_eq!(
            draft.statements(&PostgresDialect, "shop").unwrap(),
            vec!["ALTER TABLE shop.orders RENAME COLUMN id TO order_id"]
        );
    }

    #[test]
    fn a_new_table_is_created_with_its_key_index_and_foreign_key() {
        let mut draft = TableDraft::new_table("bigint");
        draft.name = "items".into();
        draft.columns.push(ColumnDraft {
            name: "order_id".into(),
            data_type: "bigint".into(),
            not_null: true,
            ..Default::default()
        });
        draft.indexes.push(IndexDraft {
            columns: "order_id".into(),
            ..Default::default()
        });
        draft.foreign_keys.push(ForeignKeyDraft {
            columns: "order_id".into(),
            referenced: "orders".into(),
            referenced_columns: "id".into(),
            on_delete: ReferentialAction::Cascade,
            ..Default::default()
        });
        let statements = draft.statements(&PostgresDialect, "shop").unwrap();
        assert_eq!(
            statements[0],
            "CREATE TABLE shop.items (\n    id bigint NOT NULL,\n    order_id bigint NOT NULL,\n    \
             CONSTRAINT items_pkey PRIMARY KEY (id),\n    CONSTRAINT items_order_id_fkey FOREIGN KEY \
             (order_id) REFERENCES shop.orders (id) ON DELETE CASCADE\n)"
        );
        assert_eq!(
            statements[1],
            "CREATE INDEX items_order_id_idx ON shop.items (order_id)"
        );
    }

    #[test]
    fn problems_are_reported_before_any_statement() {
        let mut draft = TableDraft::new_table("bigint");
        assert_eq!(
            draft.statements(&PostgresDialect, "s"),
            Err(DraftProblem::NoName)
        );
        draft.name = "t".into();
        draft.columns.push(ColumnDraft {
            name: "id".into(),
            data_type: "int".into(),
            ..Default::default()
        });
        assert_eq!(
            draft.statements(&PostgresDialect, "s"),
            Err(DraftProblem::DuplicateColumn("id".into()))
        );
    }
}
