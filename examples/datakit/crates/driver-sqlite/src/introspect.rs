//! Reading the catalog from `sqlite_schema` and the schema pragmas.
//!
//! A SQLite connection has a schema per database: `main`, `temp` and each
//! attached database. The table-valued pragmas (`pragma_table_xinfo`,
//! `pragma_index_list`, `pragma_foreign_key_list`) describe columns, indexes
//! and foreign keys; what only the `CREATE` statement says — constraint
//! names, `CHECK` expressions, `AUTOINCREMENT`, generation expressions — is
//! read from its text by [`crate::parse`].
//!
//! SQLite keeps no name for an unnamed constraint. Those get PostgreSQL's
//! default names (`orders_pkey`, `orders_email_key`, `orders_customer_id_fkey`,
//! `orders_check`), so that DDL written from the catalog names them and
//! reading it back gives the same catalog.

use std::collections::HashMap;

use anyhow::{Context as _, Result};
use datakit_catalog::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationType, Schema, Trigger,
};
use rusqlite::{Connection, params};

use crate::{
    error::convert,
    parse::{self, RuleText, TableText},
};

/// `main`, every attached database, and `temp` when it holds anything.
pub(crate) fn schemas(connection: &Connection) -> Result<Vec<Schema>> {
    let names = query(
        connection,
        "SELECT name FROM pragma_database_list ORDER BY seq",
        [],
        |row| row.get::<_, String>(0),
    )
    .context("Couldn’t read the databases")?;
    let mut schemas = Vec::new();
    for name in names {
        if name == "temp" {
            let objects: i64 = connection
                .query_row("SELECT count(*) FROM temp.sqlite_schema", [], |row| {
                    row.get(0)
                })
                .map_err(|error| convert(error, ""))?;
            if objects == 0 {
                continue;
            }
        }
        schemas.push(Schema::new(name));
    }
    Ok(schemas)
}

/// One object from `sqlite_schema`.
struct Object {
    object_type: String,
    name: String,
    table: String,
    sql: Option<String>,
}

/// Every table and view of the database `name`, with their columns,
/// indexes, constraints and triggers. SQLite has no routines or sequences.
pub(crate) fn schema(connection: &Connection, name: &str) -> Result<Schema> {
    let schema = quote(name);
    let objects = query(
        connection,
        &format!(
            "SELECT type, name, tbl_name, sql FROM {schema}.sqlite_schema \
             WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name"
        ),
        [],
        |row| {
            Ok(Object {
                object_type: row.get(0)?,
                name: row.get(1)?,
                table: row.get(2)?,
                sql: row.get(3)?,
            })
        },
    )
    .with_context(|| format!("Couldn’t read the objects of {name}"))?;
    let estimates = estimated_rows(connection, &schema).unwrap_or_else(|error| {
        tracing::debug!("couldn’t read sqlite_stat1: {error}");
        HashMap::new()
    });
    let index_sql: HashMap<&str, &str> = objects
        .iter()
        .filter(|object| object.object_type == "index")
        .filter_map(|object| Some((object.name.as_str(), object.sql.as_deref()?)))
        .collect();

    let mut relations = Vec::new();
    for object in &objects {
        let relation_type = match object.object_type.as_str() {
            "table" => RelationType::Table,
            "view" => RelationType::View,
            _ => continue,
        };
        let sql = object.sql.as_deref().unwrap_or_default();
        let mut relation = if relation_type == RelationType::View {
            let columns = columns(connection, name, &object.name, &TableText::default())
                .unwrap_or_else(|error| {
                    // A view over a table that no longer exists cannot be
                    // described, but it is still there to be fixed.
                    tracing::debug!("couldn’t read the columns of {}: {error}", object.name);
                    Vec::new()
                });
            let mut view = Relation::new(object.name.as_str(), relation_type).with_columns(columns);
            if let Some(query) = parse::view_query(sql) {
                view = view.with_definition(query);
            }
            view
        } else {
            table(connection, name, &object.name, sql, &index_sql)
                .with_context(|| format!("Couldn’t read the table {}", object.name))?
        };
        let triggers: Vec<Trigger> = objects
            .iter()
            .filter(|trigger| {
                trigger.object_type == "trigger" && trigger.table.eq_ignore_ascii_case(&object.name)
            })
            .map(|trigger| {
                let sql = trigger.sql.as_deref().unwrap_or_default();
                Trigger::new(trigger.name.as_str(), parse::trigger_timing(sql)).with_definition(sql)
            })
            .collect();
        relation = relation.with_triggers(triggers);
        if let Some(rows) = estimates.get(&object.name.to_lowercase()) {
            relation = relation.with_estimated_rows(*rows);
        }
        relations.push(relation);
    }
    Ok(Schema::new(name).with_relations(relations))
}

/// A table with its columns, indexes and constraints.
fn table(
    connection: &Connection,
    schema: &str,
    name: &str,
    sql: &str,
    index_sql: &HashMap<&str, &str>,
) -> Result<Relation> {
    let text = parse::table(sql);
    let columns = columns(connection, schema, name, &text)?;
    let mut constraints = Vec::new();
    let mut indexes = Vec::new();

    let mut key: Vec<(i64, String)> = query(
        connection,
        "SELECT pk, name FROM pragma_table_xinfo(?1, ?2) WHERE pk > 0",
        params![name, schema],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    key.sort();
    let key: Vec<String> = key.into_iter().map(|(_, column)| column).collect();
    let key_name = (!key.is_empty()).then(|| {
        text.constraint_name(&key, |rule| match rule {
            RuleText::PrimaryKey { columns } => Some(columns),
            _ => None,
        })
        .map_or_else(|| format!("{name}_pkey"), String::from)
    });
    if let Some(key_name) = &key_name {
        constraints.push(Constraint::new(
            key_name.as_str(),
            ConstraintRule::PrimaryKey {
                columns: key.iter().map(|column| column.as_str().into()).collect(),
            },
        ));
    }

    let listed = query(
        connection,
        "SELECT name, \"unique\", origin, partial FROM pragma_index_list(?1, ?2)",
        params![name, schema],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        },
    )?;
    let mut unique_constraints = Vec::new();
    for (index_name, unique, origin, partial) in listed {
        let index_text = index_sql
            .get(index_name.as_str())
            .map(|sql| parse::index(sql))
            .unwrap_or_default();
        let keys = query(
            connection,
            "SELECT seqno, cid, name FROM pragma_index_xinfo(?1, ?2) WHERE key = 1 ORDER BY seqno",
            params![index_name, schema],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )?;
        let keys: Vec<String> = keys
            .into_iter()
            .map(|(seqno, cid, column)| match (column, cid) {
                (Some(column), _) => column,
                (None, -1) => "rowid".to_string(),
                (None, _) => index_text
                    .keys
                    .get(usize::try_from(seqno).unwrap_or(usize::MAX))
                    .cloned()
                    .unwrap_or_else(|| "?".to_string()),
            })
            .collect();
        match origin.as_str() {
            "pk" => indexes.push(
                Index::new(key_name.clone().unwrap_or(index_name), keys)
                    .unique(true)
                    .primary(true),
            ),
            "u" => {
                let constraint_name = text
                    .constraint_name(&keys, |rule| match rule {
                        RuleText::Unique { columns } => Some(columns),
                        _ => None,
                    })
                    .map_or_else(|| default_name(name, &keys, "key"), String::from);
                indexes.push(Index::new(constraint_name.as_str(), keys.clone()).unique(true));
                unique_constraints.push(Constraint::new(
                    constraint_name,
                    ConstraintRule::Unique {
                        columns: keys.iter().map(|key| key.as_str().into()).collect(),
                    },
                ));
            }
            _ => {
                let mut index = Index::new(index_name.as_str(), keys).unique(unique);
                if let Some(sql) = index_sql.get(index_name.as_str()) {
                    index = index.with_definition(*sql);
                }
                if partial && let Some(predicate) = index_text.predicate {
                    index = index.with_predicate(predicate);
                }
                indexes.push(index);
            }
        }
    }
    // The primary key's index first, then by name.
    indexes.sort_by_key(|index| (!index.is_primary(), index.name()));
    unique_constraints.sort_by_key(Constraint::name);
    constraints.extend(unique_constraints);
    constraints.extend(foreign_keys(connection, schema, name, &text)?);

    let mut checks = 0;
    for constraint in &text.constraints {
        let RuleText::Check { expression } = &constraint.rule else {
            continue;
        };
        let check_name = constraint.name.clone().unwrap_or_else(|| {
            checks += 1;
            if checks == 1 {
                format!("{name}_check")
            } else {
                format!("{name}_check{}", checks - 1)
            }
        });
        constraints.push(Constraint::new(
            check_name,
            ConstraintRule::Check {
                expression: expression.as_str().into(),
            },
        ));
    }

    Ok(Relation::new(name, RelationType::Table)
        .with_columns(columns)
        .with_indexes(indexes)
        .with_constraints(constraints))
}

/// The columns of a table or view, from `pragma_table_xinfo`. `text` is the
/// table's statement, read for what the pragma leaves out.
fn columns(
    connection: &Connection,
    schema: &str,
    relation: &str,
    text: &TableText,
) -> Result<Vec<Column>> {
    struct Row {
        name: String,
        data_type: String,
        not_null: bool,
        default: Option<String>,
        key: i64,
        hidden: i64,
    }
    let rows = query(
        connection,
        "SELECT name, type, \"notnull\", dflt_value, pk, hidden \
         FROM pragma_table_xinfo(?1, ?2) ORDER BY cid",
        params![relation, schema],
        |row| {
            Ok(Row {
                name: row.get(0)?,
                data_type: row.get(1)?,
                not_null: row.get(2)?,
                default: row.get(3)?,
                key: row.get(4)?,
                hidden: row.get(5)?,
            })
        },
    )?;
    // A single `INTEGER` key column is the rowid, which is never `NULL`; so
    // is every key column of a table `WITHOUT ROWID`.
    let key_columns: Vec<&Row> = rows.iter().filter(|row| row.key > 0).collect();
    let rowid_key = !text.without_rowid
        && key_columns.len() == 1
        && key_columns[0].data_type.eq_ignore_ascii_case("INTEGER");
    Ok(rows
        .iter()
        // Hidden columns of a virtual table are not part of it.
        .filter(|row| row.hidden != 1)
        .map(|row| {
            let parsed = text.column(&row.name);
            let generated = matches!(row.hidden, 2 | 3);
            let not_null = row.not_null || (row.key > 0 && (rowid_key || text.without_rowid));
            let mut column = Column::new(row.name.as_str(), row.data_type.as_str())
                .nullable(!not_null)
                .primary_key(row.key > 0)
                .generated(generated)
                .auto_increment(parsed.is_some_and(|parsed| parsed.autoincrement));
            let default = if generated {
                parsed.and_then(|parsed| parsed.generated.clone())
            } else {
                row.default.clone()
            };
            if let Some(default) = default {
                column = column.with_default(default);
            }
            column
        })
        .collect())
}

/// The foreign keys of a table, in the order they were declared.
fn foreign_keys(
    connection: &Connection,
    schema: &str,
    table: &str,
    text: &TableText,
) -> Result<Vec<Constraint>> {
    struct Reference {
        id: i64,
        table: String,
        from: String,
        to: Option<String>,
        on_update: String,
        on_delete: String,
    }
    let references = query(
        connection,
        "SELECT id, \"table\", \"from\", \"to\", on_update, on_delete \
         FROM pragma_foreign_key_list(?1, ?2) ORDER BY id DESC, seq",
        params![table, schema],
        |row| {
            Ok(Reference {
                id: row.get(0)?,
                table: row.get(1)?,
                from: row.get(2)?,
                to: row.get(3)?,
                on_update: row.get(4)?,
                on_delete: row.get(5)?,
            })
        },
    )?;
    let mut keys: Vec<Vec<&Reference>> = Vec::new();
    for reference in &references {
        match keys.last_mut() {
            Some(key) if key[0].id == reference.id => key.push(reference),
            _ => keys.push(vec![reference]),
        }
    }
    let mut constraints = Vec::new();
    for key in keys {
        let first = key[0];
        let columns: Vec<String> = key.iter().map(|reference| reference.from.clone()).collect();
        // A key that names no columns references the other table's
        // primary key.
        let referenced: Vec<String> = if key.iter().all(|reference| reference.to.is_some()) {
            key.iter()
                .filter_map(|reference| reference.to.clone())
                .collect()
        } else {
            let mut primary: Vec<(i64, String)> = query(
                connection,
                "SELECT pk, name FROM pragma_table_info(?1, ?2) WHERE pk > 0",
                params![first.table, schema],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            primary.sort();
            primary.into_iter().map(|(_, column)| column).collect()
        };
        let name = text
            .constraint_name(&columns, |rule| match rule {
                RuleText::ForeignKey { columns } => Some(columns),
                _ => None,
            })
            .map_or_else(|| default_name(table, &columns, "fkey"), String::from);
        constraints.push(Constraint::new(
            name,
            ConstraintRule::ForeignKey(
                ForeignKey::new(columns, schema, first.table.as_str(), referenced)
                    .with_on_update(referential_action(&first.on_update))
                    .with_on_delete(referential_action(&first.on_delete)),
            ),
        ));
    }
    Ok(constraints)
}

/// The row count `ANALYZE` recorded for each table, by lower-case name, when
/// the database has statistics.
fn estimated_rows(connection: &Connection, schema: &str) -> Result<HashMap<String, i64>> {
    let analyzed: i64 = connection
        .query_row(
            &format!("SELECT count(*) FROM {schema}.sqlite_schema WHERE name = 'sqlite_stat1'"),
            [],
            |row| row.get(0),
        )
        .map_err(|error| convert(error, ""))?;
    if analyzed == 0 {
        return Ok(HashMap::new());
    }
    let statistics = query(
        connection,
        &format!("SELECT tbl, stat FROM {schema}.sqlite_stat1"),
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
    )?;
    let mut estimates: HashMap<String, i64> = HashMap::new();
    for (table, statistic) in statistics {
        // Every row's statistic starts with the table's row count.
        let Some(rows) = statistic
            .as_deref()
            .and_then(|statistic| statistic.split_whitespace().next())
            .and_then(|rows| rows.parse::<i64>().ok())
        else {
            continue;
        };
        let estimate = estimates.entry(table.to_lowercase()).or_default();
        *estimate = (*estimate).max(rows);
    }
    Ok(estimates)
}

fn query<T, P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    params: P,
    read: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    let run = || -> rusqlite::Result<Vec<T>> {
        let mut statement = connection.prepare(sql)?;
        let rows = statement.query_map(params, read)?;
        rows.collect()
    };
    run().map_err(|error| convert(error, sql))
}

/// `name` quoted as an identifier, always.
fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// PostgreSQL's name for an unnamed constraint: `orders_customer_id_fkey`.
fn default_name(table: &str, columns: &[String], suffix: &str) -> String {
    let mut name = table.to_string();
    for column in columns {
        name.push('_');
        name.push_str(column);
    }
    name.push('_');
    name.push_str(suffix);
    name
}

fn referential_action(action: &str) -> ReferentialAction {
    match action.to_ascii_uppercase().as_str() {
        "RESTRICT" => ReferentialAction::Restrict,
        "CASCADE" => ReferentialAction::Cascade,
        "SET NULL" => ReferentialAction::SetNull,
        "SET DEFAULT" => ReferentialAction::SetDefault,
        _ => ReferentialAction::NoAction,
    }
}
