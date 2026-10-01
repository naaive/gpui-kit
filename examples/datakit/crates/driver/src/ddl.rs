//! The standard-SQL bodies of [`Dialect`]'s data definition methods.
//!
//! They are free functions so a dialect that overrides one method can still
//! call the standard version of another.

use datakit_catalog::{
    Column, Constraint, ConstraintRule, Index, Relation, RelationChange, RelationType, SchemaDiff,
    Sequence,
};

use crate::{Dialect, RowChange};

pub fn create_relation<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    relation: &Relation,
) -> Vec<String> {
    let name = dialect.qualified_name(schema, &relation.name());
    let mut statements = Vec::new();
    match relation.relation_type() {
        RelationType::View | RelationType::MaterializedView => {
            let object = if relation.relation_type() == RelationType::View {
                "VIEW"
            } else {
                "MATERIALIZED VIEW"
            };
            let query = relation
                .definition()
                .unwrap_or("SELECT 1")
                .trim()
                .trim_end_matches(';');
            statements.push(format!("CREATE {object} {name} AS\n{query}"));
        }
        _ => {
            let mut lines: Vec<String> = relation
                .columns()
                .iter()
                .map(|column| format!("    {}", dialect.column_definition(column)))
                .collect();
            lines.extend(
                relation
                    .constraints()
                    .iter()
                    .map(|constraint| format!("    {}", dialect.constraint_definition(constraint))),
            );
            statements.push(format!("CREATE TABLE {name} (\n{}\n)", lines.join(",\n")));
        }
    }
    if relation.comment().is_some()
        && let Some(comment) = dialect.comment_on_relation(schema, relation)
    {
        statements.push(comment);
    }
    for column in relation.columns() {
        if column.comment().is_some()
            && let Some(comment) = dialect.comment_on_column(schema, &relation.name(), column)
        {
            statements.push(comment);
        }
    }
    for index in standalone_indexes(relation) {
        statements.push(dialect.create_index(schema, &relation.name(), index));
    }
    for trigger in relation.triggers() {
        statements.extend(dialect.create_trigger(schema, &relation.name(), trigger));
    }
    statements
}

/// Indexes that are not created by a constraint of the relation.
pub fn standalone_indexes(relation: &Relation) -> impl Iterator<Item = &Index> {
    relation.indexes().iter().filter(|index| {
        !index.is_primary()
            && !relation
                .constraints()
                .iter()
                .any(|constraint| constraint.name() == index.name())
    })
}

pub fn column_definition<D: Dialect + ?Sized>(dialect: &D, column: &Column) -> String {
    let mut definition = format!(
        "{} {}",
        dialect.quote_identifier(&column.name()),
        column.data_type()
    );
    if let Some(default) = column.default() {
        if column.is_generated() {
            definition.push_str(&format!(" GENERATED ALWAYS AS ({default}) STORED"));
        } else {
            definition.push_str(&format!(" DEFAULT {default}"));
        }
    }
    if !column.is_nullable() {
        definition.push_str(" NOT NULL");
    }
    definition
}

pub fn constraint_definition<D: Dialect + ?Sized>(dialect: &D, constraint: &Constraint) -> String {
    let body = match constraint.definition() {
        Some(definition) => definition.to_string(),
        None => match constraint.rule() {
            ConstraintRule::PrimaryKey { columns } => {
                format!("PRIMARY KEY ({})", identifier_list(dialect, columns))
            }
            ConstraintRule::Unique { columns } => {
                format!("UNIQUE ({})", identifier_list(dialect, columns))
            }
            ConstraintRule::ForeignKey(key) => {
                let mut text = format!(
                    "FOREIGN KEY ({}) REFERENCES {} ({})",
                    identifier_list(dialect, key.columns()),
                    dialect.qualified_name(key.referenced_schema(), key.referenced_relation()),
                    identifier_list(dialect, key.referenced_columns())
                );
                if key.on_update() != Default::default() {
                    text.push_str(&format!(" ON UPDATE {}", key.on_update().sql()));
                }
                if key.on_delete() != Default::default() {
                    text.push_str(&format!(" ON DELETE {}", key.on_delete().sql()));
                }
                text
            }
            ConstraintRule::Check { expression } => format!("CHECK ({expression})"),
            ConstraintRule::Exclusion => "EXCLUDE ()".into(),
        },
    };
    format!(
        "CONSTRAINT {} {body}",
        dialect.quote_identifier(&constraint.name())
    )
}

pub fn create_index<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    relation: &str,
    index: &Index,
) -> String {
    if let Some(definition) = index.definition() {
        return definition.trim().trim_end_matches(';').to_string();
    }
    let mut sql = format!(
        "CREATE {}INDEX {} ON {}",
        if index.is_unique() { "UNIQUE " } else { "" },
        dialect.quote_identifier(&index.name()),
        dialect.qualified_name(schema, relation)
    );
    if let Some(method) = index.method() {
        sql.push_str(&format!(" USING {method}"));
    }
    let columns: Vec<String> = index
        .columns()
        .iter()
        .map(|column| {
            // An expression is kept as written; a name is quoted.
            if column.contains(['(', ' ', ':']) {
                column.to_string()
            } else {
                dialect.quote_identifier(column)
            }
        })
        .collect();
    sql.push_str(&format!(" ({})", columns.join(", ")));
    if let Some(predicate) = index.predicate() {
        sql.push_str(&format!(" WHERE {predicate}"));
    }
    sql
}

pub fn create_sequence<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    sequence: &Sequence,
) -> String {
    let mut sql = format!(
        "CREATE SEQUENCE {} AS {} INCREMENT BY {} MINVALUE {} MAXVALUE {} START WITH {}",
        dialect.qualified_name(schema, &sequence.name()),
        sequence.data_type(),
        sequence.increment(),
        sequence.min_value(),
        sequence.max_value(),
        sequence.start()
    );
    if sequence.is_cycle() {
        sql.push_str(" CYCLE");
    }
    sql
}

pub fn alter_column<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    relation: &str,
    old: &Column,
    new: &Column,
) -> Vec<String> {
    let table = dialect.qualified_name(schema, relation);
    let column = dialect.quote_identifier(&new.name());
    let mut statements = Vec::new();
    if old.data_type() != new.data_type() {
        statements.push(format!(
            "ALTER TABLE {table} ALTER COLUMN {column} TYPE {}",
            new.data_type()
        ));
    }
    if old.default() != new.default() {
        statements.push(match new.default() {
            Some(default) => {
                format!("ALTER TABLE {table} ALTER COLUMN {column} SET DEFAULT {default}")
            }
            None => format!("ALTER TABLE {table} ALTER COLUMN {column} DROP DEFAULT"),
        });
    }
    if old.is_nullable() != new.is_nullable() {
        statements.push(format!(
            "ALTER TABLE {table} ALTER COLUMN {column} {} NOT NULL",
            if new.is_nullable() { "DROP" } else { "SET" }
        ));
    }
    if old.comment() != new.comment() {
        statements.extend(dialect.comment_on_column(schema, relation, new));
    }
    statements
}

pub fn alter_relation<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    change: &RelationChange,
) -> Vec<String> {
    let relation = change.source();
    let name = relation.name();
    let table = dialect.qualified_name(schema, &name);
    let mut statements = Vec::new();

    if relation.relation_type().is_view() {
        if change.is_definition_changed() {
            statements.push(dialect.drop_relation(schema, change.target()));
            statements.extend(dialect.create_relation(schema, relation));
            return statements;
        }
    } else {
        for trigger in change.removed_triggers() {
            statements.push(dialect.drop_trigger(schema, &name, trigger));
        }
        for index in change.removed_indexes() {
            if !change
                .removed_constraints()
                .iter()
                .any(|constraint| constraint.name() == index.name())
                && !index.is_primary()
            {
                statements.push(dialect.drop_index(schema, &name, index));
            }
        }
        for constraint in change.removed_constraints() {
            statements.push(format!(
                "ALTER TABLE {table} DROP CONSTRAINT {}",
                dialect.quote_identifier(&constraint.name())
            ));
        }
        for column in change.added_columns() {
            statements.push(dialect.add_column(schema, &name, column));
            if column.comment().is_some() {
                statements.extend(dialect.comment_on_column(schema, &name, column));
            }
        }
        for (old, new) in change.changed_columns() {
            statements.extend(dialect.alter_column(schema, &name, old, new));
        }
        for constraint in change.added_constraints() {
            statements.push(format!(
                "ALTER TABLE {table} ADD {}",
                dialect.constraint_definition(constraint)
            ));
        }
        for index in change.added_indexes() {
            if !change
                .added_constraints()
                .iter()
                .any(|constraint| constraint.name() == index.name())
                && !index.is_primary()
            {
                statements.push(dialect.create_index(schema, &name, index));
            }
        }
        for trigger in change.added_triggers() {
            statements.extend(dialect.create_trigger(schema, &name, trigger));
        }
        for column in change.removed_columns() {
            statements.push(format!(
                "ALTER TABLE {table} DROP COLUMN {}",
                dialect.quote_identifier(&column.name())
            ));
        }
    }
    if change.is_comment_changed() {
        statements.extend(dialect.comment_on_relation(schema, relation));
    }
    statements
}

pub fn migrate<D: Dialect + ?Sized>(dialect: &D, schema: &str, diff: &SchemaDiff) -> Vec<String> {
    let mut statements = Vec::new();
    for sequence in diff.added_sequences() {
        statements.push(dialect.create_sequence(schema, sequence));
    }
    // Tables before views, which may read them; foreign keys after every
    // table exists.
    let (views, tables): (Vec<&Relation>, Vec<&Relation>) = diff
        .added_relations()
        .iter()
        .partition(|relation| relation.relation_type().is_view());
    let mut foreign_keys = Vec::new();
    for relation in &tables {
        let (keys, others): (Vec<Constraint>, Vec<Constraint>) = relation
            .constraints()
            .iter()
            .cloned()
            .partition(|constraint| matches!(constraint.rule(), ConstraintRule::ForeignKey(_)));
        let without_keys = (*relation).clone().with_constraints(others);
        statements.extend(dialect.create_relation(schema, &without_keys));
        foreign_keys.extend(keys.into_iter().map(|key| (relation.name(), key)));
    }
    for change in diff.changed_relations() {
        statements.extend(dialect.alter_relation(schema, change));
    }
    for (relation, key) in foreign_keys {
        statements.push(format!(
            "ALTER TABLE {} ADD {}",
            dialect.qualified_name(schema, &relation),
            dialect.constraint_definition(&key)
        ));
    }
    for relation in views {
        statements.extend(dialect.create_relation(schema, relation));
    }
    for routine in diff.added_routines() {
        statements.extend(dialect.create_routine(schema, routine));
    }
    for (_, routine) in diff.changed_routines() {
        statements.extend(dialect.create_routine(schema, routine));
    }
    // Destructive changes last.
    for routine in diff.removed_routines() {
        statements.push(dialect.drop_routine(schema, routine));
    }
    let (views, tables): (Vec<&Relation>, Vec<&Relation>) = diff
        .removed_relations()
        .iter()
        .partition(|relation| relation.relation_type().is_view());
    for relation in views.into_iter().chain(tables) {
        statements.push(dialect.drop_relation(schema, relation));
    }
    for sequence in diff.removed_sequences() {
        statements.push(dialect.drop_sequence(schema, sequence));
    }
    statements
}

pub fn row_change<D: Dialect + ?Sized>(
    dialect: &D,
    schema: &str,
    relation: &str,
    change: &RowChange,
) -> String {
    let table = dialect.qualified_name(schema, relation);
    let condition = |key: &[(std::sync::Arc<str>, crate::Value)]| {
        key.iter()
            .map(|(column, value)| {
                let column = dialect.quote_identifier(column);
                if value.is_null() {
                    format!("{column} IS NULL")
                } else {
                    format!("{column} = {}", dialect.literal(value))
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    match change {
        RowChange::Insert { values } => {
            if values.is_empty() {
                return format!("INSERT INTO {table} DEFAULT VALUES");
            }
            let columns: Vec<String> = values
                .iter()
                .map(|(column, _)| dialect.quote_identifier(column))
                .collect();
            let literals: Vec<String> = values
                .iter()
                .map(|(_, value)| dialect.literal(value))
                .collect();
            format!(
                "INSERT INTO {table} ({}) VALUES ({})",
                columns.join(", "),
                literals.join(", ")
            )
        }
        RowChange::Update { key, values } => {
            let assignments: Vec<String> = values
                .iter()
                .map(|(column, value)| {
                    format!(
                        "{} = {}",
                        dialect.quote_identifier(column),
                        dialect.literal(value)
                    )
                })
                .collect();
            format!(
                "UPDATE {table} SET {} WHERE {}",
                assignments.join(", "),
                condition(key)
            )
        }
        RowChange::Delete { key } => format!("DELETE FROM {table} WHERE {}", condition(key)),
    }
}

fn identifier_list<D: Dialect + ?Sized>(dialect: &D, columns: &[std::sync::Arc<str>]) -> String {
    columns
        .iter()
        .map(|column| dialect.quote_identifier(column))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{ForeignKey, ReferentialAction, Schema, diff_schemas};

    use super::*;
    use crate::Value;

    struct Standard;

    impl Dialect for Standard {
        fn reserved_words(&self) -> &'static [&'static str] {
            &["USER", "ORDER"]
        }

        fn keywords(&self) -> &'static [&'static str] {
            &[]
        }

        fn functions(&self) -> &'static [&'static str] {
            &[]
        }
    }

    fn orders() -> Relation {
        Relation::new("orders", RelationType::Table)
            .with_columns([
                Column::new("id", "bigint").with_default("nextval('orders_id_seq')"),
                Column::new("customer_id", "integer"),
                Column::new("note", "text")
                    .nullable(true)
                    .with_comment("Free text"),
            ])
            .with_constraints([
                Constraint::new(
                    "orders_pkey",
                    ConstraintRule::PrimaryKey {
                        columns: vec!["id".into()].into(),
                    },
                ),
                Constraint::new(
                    "orders_customer_fk",
                    ConstraintRule::ForeignKey(
                        ForeignKey::new(["customer_id"], "shop", "customers", ["id"])
                            .with_on_delete(ReferentialAction::Cascade),
                    ),
                ),
            ])
            .with_indexes([
                Index::new("orders_pkey", ["id"]).unique(true).primary(true),
                Index::new("orders_customer_idx", ["customer_id"]),
            ])
    }

    #[test]
    fn a_table_is_created_with_its_constraints_comments_and_indexes() {
        let statements = create_relation(&Standard, "shop", &orders());
        assert_eq!(
            statements[0],
            "CREATE TABLE shop.orders (\n    id bigint DEFAULT nextval('orders_id_seq') NOT NULL,\n    \
             customer_id integer NOT NULL,\n    note text,\n    \
             CONSTRAINT orders_pkey PRIMARY KEY (id),\n    \
             CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id) REFERENCES shop.customers (id) ON DELETE CASCADE\n)"
        );
        assert_eq!(
            statements[1],
            "COMMENT ON COLUMN shop.orders.note IS 'Free text'"
        );
        assert_eq!(
            statements[2],
            "CREATE INDEX orders_customer_idx ON shop.orders (customer_id)"
        );
        assert_eq!(
            statements.len(),
            3,
            "the primary key's index is not repeated"
        );
    }

    #[test]
    fn a_migration_adds_before_it_drops() {
        let target = Schema::new("shop").with_relations([
            Relation::new("legacy", RelationType::Table)
                .with_columns([Column::new("id", "integer")]),
            orders(),
        ]);
        let mut changed = orders().columns().to_vec();
        changed[1] = changed[1].clone().with_data_type("bigint");
        changed.push(Column::new("total", "numeric(12,2)").with_default("0"));
        let source = Schema::new("shop").with_relations([orders().with_columns(changed)]);
        let statements = migrate(&Standard, "shop", &diff_schemas(&target, &source));
        assert_eq!(
            statements,
            vec![
                "ALTER TABLE shop.orders ADD COLUMN total numeric(12,2) DEFAULT 0 NOT NULL",
                "ALTER TABLE shop.orders ALTER COLUMN customer_id TYPE bigint",
                "DROP TABLE shop.legacy",
            ]
        );
    }

    #[test]
    fn row_changes_quote_names_and_values() {
        let key = vec![("id".into(), Value::Int(7))];
        assert_eq!(
            row_change(
                &Standard,
                "s",
                "user",
                &RowChange::Update {
                    key: key.clone(),
                    values: vec![("name".into(), Value::Text("O'Hara".into()))],
                }
            ),
            "UPDATE s.\"user\" SET name = 'O''Hara' WHERE id = 7"
        );
        assert_eq!(
            row_change(
                &Standard,
                "s",
                "t",
                &RowChange::Delete {
                    key: vec![("a".into(), Value::Null), ("b".into(), Value::Int(1))]
                }
            ),
            "DELETE FROM s.t WHERE a IS NULL AND b = 1"
        );
        assert_eq!(
            row_change(&Standard, "s", "t", &RowChange::Insert { values: vec![] }),
            "INSERT INTO s.t DEFAULT VALUES"
        );
    }
}
