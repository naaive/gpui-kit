//! Reading the catalog from `information_schema`.
//!
//! MySQL has no functions that print an object's definition the way
//! PostgreSQL's `pg_get_*def` do, only `SHOW CREATE`, one object at a time.
//! Definitions of views, triggers and routines are therefore assembled from
//! the parts `information_schema` lists, in the syntax the server accepts.
//!
//! The queries run on the connection's pool of catalog sessions, several at
//! once.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_catalog::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationType, Routine, RoutineType, Schema, Trigger,
};
use datakit_driver::Dialect as _;
use mysql_async::{Pool, Row, prelude::Queryable as _};

use crate::{dialect::MySqlDialect, types};

/// The databases MySQL itself keeps.
const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

const SCHEMAS: &str = "
    SELECT SCHEMA_NAME
    FROM information_schema.SCHEMATA
    ORDER BY SCHEMA_NAME";

const RELATIONS: &str = "
    SELECT TABLE_NAME, TABLE_TYPE, TABLE_COMMENT, TABLE_ROWS, CREATE_OPTIONS
    FROM information_schema.TABLES
    WHERE TABLE_SCHEMA = ?
    ORDER BY TABLE_NAME";

const VIEWS: &str = "
    SELECT TABLE_NAME, VIEW_DEFINITION, DEFINER
    FROM information_schema.VIEWS
    WHERE TABLE_SCHEMA = ?";

const COLUMNS: &str = "
    SELECT TABLE_NAME, COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT,
           EXTRA, COLUMN_KEY, COLUMN_COMMENT, GENERATION_EXPRESSION
    FROM information_schema.COLUMNS
    WHERE TABLE_SCHEMA = ?
    ORDER BY TABLE_NAME, ORDINAL_POSITION";

/// `EXPRESSION` names a functional key part, MySQL 8.0.13 and later.
const INDEXES: &str = "
    SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SUB_PART, INDEX_TYPE, COLLATION,
           EXPRESSION
    FROM information_schema.STATISTICS
    WHERE TABLE_SCHEMA = ?
    ORDER BY TABLE_NAME, INDEX_NAME = 'PRIMARY' DESC, INDEX_NAME, SEQ_IN_INDEX";

/// [`INDEXES`] for a server without functional key parts (MariaDB).
const INDEXES_WITHOUT_EXPRESSIONS: &str = "
    SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SUB_PART, INDEX_TYPE, COLLATION,
           NULL
    FROM information_schema.STATISTICS
    WHERE TABLE_SCHEMA = ?
    ORDER BY TABLE_NAME, INDEX_NAME = 'PRIMARY' DESC, INDEX_NAME, SEQ_IN_INDEX";

const CONSTRAINTS: &str = "
    SELECT tc.TABLE_NAME, tc.CONSTRAINT_NAME, tc.CONSTRAINT_TYPE, k.COLUMN_NAME,
           k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME,
           r.UPDATE_RULE, r.DELETE_RULE
    FROM information_schema.TABLE_CONSTRAINTS tc
    LEFT JOIN information_schema.KEY_COLUMN_USAGE k
           ON k.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA
          AND k.TABLE_NAME = tc.TABLE_NAME
          AND k.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    LEFT JOIN information_schema.REFERENTIAL_CONSTRAINTS r
           ON r.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA
          AND r.TABLE_NAME = tc.TABLE_NAME
          AND r.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.CONSTRAINT_SCHEMA = ?
      AND tc.CONSTRAINT_TYPE IN ('PRIMARY KEY', 'UNIQUE', 'FOREIGN KEY')
    ORDER BY tc.TABLE_NAME, tc.CONSTRAINT_TYPE = 'PRIMARY KEY' DESC, tc.CONSTRAINT_NAME,
             k.ORDINAL_POSITION";

/// `CHECK_CONSTRAINTS` exists from MySQL 8.0.16 and MariaDB 10.2.
const CHECKS: &str = "
    SELECT tc.TABLE_NAME, cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE
    FROM information_schema.TABLE_CONSTRAINTS tc
    JOIN information_schema.CHECK_CONSTRAINTS cc
      ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA
     AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.CONSTRAINT_SCHEMA = ?
      AND tc.CONSTRAINT_TYPE = 'CHECK'
    ORDER BY tc.TABLE_NAME, cc.CONSTRAINT_NAME";

const TRIGGERS: &str = "
    SELECT EVENT_OBJECT_TABLE, TRIGGER_NAME, ACTION_TIMING, EVENT_MANIPULATION,
           ACTION_ORIENTATION, ACTION_STATEMENT
    FROM information_schema.TRIGGERS
    WHERE TRIGGER_SCHEMA = ?
    ORDER BY EVENT_OBJECT_TABLE, ACTION_TIMING, EVENT_MANIPULATION, ACTION_ORDER";

const ROUTINES: &str = "
    SELECT SPECIFIC_NAME, ROUTINE_NAME, ROUTINE_TYPE, DTD_IDENTIFIER, ROUTINE_BODY,
           ROUTINE_DEFINITION, IS_DETERMINISTIC, SQL_DATA_ACCESS, SECURITY_TYPE,
           ROUTINE_COMMENT
    FROM information_schema.ROUTINES
    WHERE ROUTINE_SCHEMA = ?
    ORDER BY ROUTINE_NAME, ROUTINE_TYPE";

const PARAMETERS: &str = "
    SELECT SPECIFIC_NAME, ROUTINE_TYPE, PARAMETER_MODE, PARAMETER_NAME, DTD_IDENTIFIER
    FROM information_schema.PARAMETERS
    WHERE SPECIFIC_SCHEMA = ?
      AND ORDINAL_POSITION > 0
    ORDER BY SPECIFIC_NAME, ROUTINE_TYPE, ORDINAL_POSITION";

/// Server errors that mean an older server lacks a table or column the
/// query reads: unknown column, unknown table.
const MISSING: &[u16] = &[1054, 1109, 1146];

pub(crate) async fn schemas(pool: Pool) -> Result<Vec<Schema>> {
    let names: Vec<String> = async {
        let mut conn = pool.get_conn().await?;
        conn.query(SCHEMAS).await
    }
    .await
    .context("Couldn’t read the databases")?;
    let mut schemas: Vec<Schema> = names
        .into_iter()
        .map(|name| {
            let system = is_system(&name);
            Schema::new(name).system(system)
        })
        .collect();
    // Like the PostgreSQL driver: the user's schemas first.
    schemas.sort_by_key(Schema::is_system);
    Ok(schemas)
}

pub(crate) async fn schema(pool: Pool, name: Arc<str>, is_mariadb: bool) -> Result<Schema> {
    let indexes = if is_mariadb {
        INDEXES_WITHOUT_EXPRESSIONS
    } else {
        INDEXES
    };
    let (relations, views, columns, indexes, constraints, checks, triggers, routines, parameters) =
        futures::try_join!(
            query(&pool, RELATIONS, &name, None),
            query(&pool, VIEWS, &name, None),
            query(&pool, COLUMNS, &name, None),
            query(&pool, indexes, &name, Some(INDEXES_WITHOUT_EXPRESSIONS)),
            query(&pool, CONSTRAINTS, &name, None),
            query(&pool, CHECKS, &name, Some("")),
            query(&pool, TRIGGERS, &name, None),
            query(&pool, ROUTINES, &name, None),
            query(&pool, PARAMETERS, &name, None),
        )
        .with_context(|| format!("Couldn’t read the objects of {name}"))?;
    let dialect = MySqlDialect;

    let mut columns_by_relation: HashMap<String, Vec<Column>> = HashMap::new();
    for row in &columns {
        let extra = string(row, 6);
        let generation = text(row, 9).filter(|expression| !expression.is_empty());
        let mut column = Column::new(string(row, 1), string(row, 2))
            .nullable(string(row, 4) == "YES")
            .primary_key(string(row, 7) == "PRI")
            .generated(generation.is_some())
            .auto_increment(extra.to_ascii_lowercase().contains("auto_increment"));
        let default = match generation {
            Some(expression) => Some(unescape(&expression)),
            None => column_default(text(row, 5).as_deref(), &string(row, 3), &extra, is_mariadb),
        };
        if let Some(default) = default {
            column = column.with_default(default);
        }
        if let Some(comment) = text(row, 8).filter(|comment| !comment.is_empty()) {
            column = column.with_comment(comment);
        }
        columns_by_relation
            .entry(string(row, 0))
            .or_default()
            .push(column);
    }

    let mut indexes_by_relation: HashMap<String, Vec<Index>> = HashMap::new();
    for group in grouped(&indexes) {
        let first = group[0];
        let parts: Vec<String> = group
            .iter()
            .map(|row| {
                let part = match (text(row, 3), text(row, 7)) {
                    (_, Some(expression)) => format!("({})", unescape(&expression)),
                    (Some(column), None) => match number(row, 4) {
                        Some(length) => format!("`{}`({length})", column.replace('`', "``")),
                        None => column,
                    },
                    (None, None) => String::new(),
                };
                if string(row, 6) == "D" {
                    if part.starts_with(['(', '`']) {
                        format!("{part} DESC")
                    } else {
                        format!("`{}` DESC", part.replace('`', "``"))
                    }
                } else {
                    part
                }
            })
            .collect();
        let index_name = string(first, 1);
        let index = Index::new(index_name.clone(), parts)
            .unique(number(first, 2) == Some(0))
            .primary(index_name == "PRIMARY")
            .with_method(string(first, 5).to_ascii_lowercase());
        indexes_by_relation
            .entry(string(first, 0))
            .or_default()
            .push(index);
    }

    let mut constraints_by_relation: HashMap<String, Vec<Constraint>> = HashMap::new();
    for group in grouped(&constraints) {
        let first = group[0];
        let columns: Arc<[Arc<str>]> = group
            .iter()
            .filter_map(|row| text(row, 3))
            .map(Arc::from)
            .collect();
        let rule = match string(first, 2).as_str() {
            "PRIMARY KEY" => ConstraintRule::PrimaryKey { columns },
            "UNIQUE" => ConstraintRule::Unique { columns },
            _ => ConstraintRule::ForeignKey(
                ForeignKey::new(
                    columns.iter().cloned(),
                    string(first, 4),
                    string(first, 5),
                    group.iter().filter_map(|row| text(row, 6)),
                )
                .with_on_update(referential_action(&string(first, 7)))
                .with_on_delete(referential_action(&string(first, 8))),
            ),
        };
        constraints_by_relation
            .entry(string(first, 0))
            .or_default()
            .push(Constraint::new(string(first, 1), rule));
    }
    for row in &checks {
        constraints_by_relation
            .entry(string(row, 0))
            .or_default()
            .push(Constraint::new(
                string(row, 1),
                ConstraintRule::Check {
                    expression: unescape(&string(row, 2)).into(),
                },
            ));
    }

    let mut triggers_by_relation: HashMap<String, Vec<Trigger>> = HashMap::new();
    for row in &triggers {
        let table = string(row, 0);
        let trigger_name = string(row, 1);
        let (timing, event, orientation) = (string(row, 2), string(row, 3), string(row, 4));
        let definition = format!(
            "CREATE TRIGGER {} {timing} {event} ON {} FOR EACH {orientation}\n{}",
            dialect.qualified_name(&name, &trigger_name),
            dialect.qualified_name(&name, &table),
            string(row, 5)
        );
        triggers_by_relation.entry(table).or_default().push(
            Trigger::new(
                trigger_name,
                format!("{timing} {event} FOR EACH {orientation}"),
            )
            .with_definition(definition),
        );
    }

    let mut views_by_name: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for row in &views {
        views_by_name.insert(
            string(row, 0),
            (
                text(row, 1).filter(|definition| !definition.is_empty()),
                text(row, 2),
            ),
        );
    }

    let relations = relations.iter().filter_map(|row| {
        let relation_name = string(row, 0);
        let relation_type = match string(row, 1).as_str() {
            "VIEW" | "SYSTEM VIEW" => RelationType::View,
            // MariaDB's sequences are tables to `information_schema`.
            "SEQUENCE" => return None,
            _ if string(row, 4).contains("partitioned") => RelationType::PartitionedTable,
            _ => RelationType::Table,
        };
        let mut relation = Relation::new(relation_name.clone(), relation_type)
            .with_columns(
                columns_by_relation
                    .remove(&relation_name)
                    .unwrap_or_default(),
            )
            .with_indexes(
                indexes_by_relation
                    .remove(&relation_name)
                    .unwrap_or_default(),
            )
            .with_constraints(
                constraints_by_relation
                    .remove(&relation_name)
                    .unwrap_or_default(),
            )
            .with_triggers(
                triggers_by_relation
                    .remove(&relation_name)
                    .unwrap_or_default(),
            );
        if relation_type.is_view() {
            // MySQL gives every view the comment `VIEW`.
            if let Some((definition, definer)) = views_by_name.remove(&relation_name) {
                if let Some(definition) = definition {
                    relation = relation.with_definition(definition);
                }
                if let Some(definer) = definer {
                    relation = relation.with_owner(definer);
                }
            }
        } else {
            if let Some(comment) = text(row, 2).filter(|comment| !comment.is_empty()) {
                relation = relation.with_comment(comment);
            }
            if let Some(rows) = number(row, 3) {
                relation = relation.with_estimated_rows(rows);
            }
        }
        Some(relation)
    });

    let mut parameters_by_routine: HashMap<(String, String), Vec<String>> = HashMap::new();
    for row in &parameters {
        // MySQL lists functions' parameters as `IN`, which a function
        // cannot say.
        let mode = text(row, 2).filter(|_| string(row, 1) == "PROCEDURE");
        let parameter = [
            mode,
            text(row, 3).map(|name| dialect.quote_identifier(&name)),
            text(row, 4),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        parameters_by_routine
            .entry((string(row, 0), string(row, 1)))
            .or_default()
            .push(parameter);
    }

    let routines = routines.iter().map(|row| {
        let routine_name = string(row, 1);
        let object = string(row, 2);
        let arguments = parameters_by_routine
            .remove(&(string(row, 0), object.clone()))
            .unwrap_or_default()
            .join(", ");
        let is_procedure = object == "PROCEDURE";
        let result = text(row, 3).filter(|_| !is_procedure);
        let comment = text(row, 9).filter(|comment| !comment.is_empty());
        let mut routine = Routine::new(
            routine_name.clone(),
            if is_procedure {
                RoutineType::Procedure
            } else {
                RoutineType::Function
            },
            arguments.clone(),
        )
        .with_language(string(row, 4));
        if let Some(body) = text(row, 5) {
            let mut definition = format!(
                "CREATE {object} {}({arguments})",
                dialect.qualified_name(&name, &routine_name)
            );
            if let Some(result) = &result {
                definition.push_str(&format!(" RETURNS {result}"));
            }
            if let Some(comment) = &comment {
                definition.push_str(&format!(
                    "\n    COMMENT {}",
                    dialect.string_literal(comment)
                ));
            }
            if string(row, 6) == "YES" {
                definition.push_str("\n    DETERMINISTIC");
            }
            let access = string(row, 7);
            if !access.is_empty() && access != "CONTAINS SQL" {
                definition.push_str(&format!("\n    {access}"));
            }
            if string(row, 8) == "INVOKER" {
                definition.push_str("\n    SQL SECURITY INVOKER");
            }
            definition.push('\n');
            definition.push_str(&body);
            routine = routine.with_definition(definition);
        }
        if let Some(result) = result {
            routine = routine.with_result(result);
        }
        if let Some(comment) = comment {
            routine = routine.with_comment(comment);
        }
        routine
    });

    let system = is_system(&name);
    Ok(Schema::new(name.clone())
        .system(system)
        .with_relations(relations.collect::<Vec<_>>())
        .with_routines(routines.collect::<Vec<_>>()))
}

fn is_system(name: &str) -> bool {
    SYSTEM_SCHEMAS
        .iter()
        .any(|system| system.eq_ignore_ascii_case(name))
}

/// The rows of `sql` for `schema`, from one of the pool's sessions. When an
/// older server lacks what `sql` reads, `fallback` runs instead; an empty
/// fallback means there is nothing to read.
async fn query(
    pool: &Pool,
    sql: &str,
    schema: &str,
    fallback: Option<&str>,
) -> mysql_async::Result<Vec<Row>> {
    let mut conn = pool.get_conn().await?;
    match conn.exec(sql, (schema,)).await {
        Err(mysql_async::Error::Server(error)) if MISSING.contains(&error.code) => match fallback {
            Some("") => Ok(Vec::new()),
            Some(fallback) => conn.exec(fallback, (schema,)).await,
            None => Err(mysql_async::Error::Server(error)),
        },
        result => result,
    }
}

/// Consecutive rows that share their first two columns, the relation and
/// the object's name.
fn grouped(rows: &[Row]) -> Vec<Vec<&Row>> {
    let mut groups: Vec<Vec<&Row>> = Vec::new();
    for row in rows {
        let key = (text(row, 0), text(row, 1));
        match groups.last_mut() {
            Some(group) if (text(group[0], 0), text(group[0], 1)) == key => group.push(row),
            _ => groups.push(vec![row]),
        }
    }
    groups
}

fn text(row: &Row, ix: usize) -> Option<String> {
    row.as_ref(ix).and_then(types::text)
}

fn string(row: &Row, ix: usize) -> String {
    text(row, ix).unwrap_or_default()
}

fn number(row: &Row, ix: usize) -> Option<i64> {
    text(row, ix)?.parse().ok()
}

/// A column's default as SQL that sets it.
///
/// MySQL lists a literal default as its bare value — `it's` for the
/// default `'it''s'` — and an expression default (`DEFAULT_GENERATED`) as
/// the expression, with its quotes escaped by backslashes. MariaDB lists
/// every default as SQL already, and a `NULL` default as `NULL`. An
/// `ON UPDATE` clause, which MySQL lists among the column's extras, rides
/// along after the default so the column definition restates it.
fn column_default(
    default: Option<&str>,
    data_type: &str,
    extra: &str,
    is_mariadb: bool,
) -> Option<String> {
    let extra_lower = extra.to_ascii_lowercase();
    let default = match default {
        None => None,
        Some(default) if is_mariadb => (default != "NULL").then(|| default.to_string()),
        Some(default) if extra_lower.contains("default_generated") => {
            if is_current_time(default) {
                Some(default.to_string())
            } else {
                Some(format!("({})", unescape(default)))
            }
        }
        Some(default) if is_numeric(data_type) => Some(default.to_string()),
        Some(default) => Some(MySqlDialect.string_literal(default)),
    };
    match extra_lower.find("on update ") {
        Some(start) => {
            let on_update = &extra[start + "on update ".len()..];
            Some(format!(
                "{} ON UPDATE {on_update}",
                default.as_deref().unwrap_or("NULL")
            ))
        }
        None => default,
    }
}

/// Whether a default is the current time, which MySQL accepts without the
/// parentheses an expression default needs.
fn is_current_time(default: &str) -> bool {
    let lower = default.to_ascii_lowercase();
    let name = lower.split('(').next().unwrap_or("");
    matches!(
        name,
        "current_timestamp" | "now" | "localtime" | "localtimestamp"
    )
}

/// Whether a column of `data_type` (`information_schema.COLUMNS.DATA_TYPE`)
/// takes a literal default unquoted.
fn is_numeric(data_type: &str) -> bool {
    matches!(
        data_type.to_ascii_lowercase().as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "numeric"
            | "float"
            | "double"
            | "real"
            | "bit"
    )
}

/// `text` with the backslash escapes `information_schema` puts in an
/// expression undone: `concat(_utf8mb4\'a\')` is `concat(_utf8mb4'a')`.
fn unescape(text: &str) -> String {
    let mut unescaped = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(&next) = chars.peek()
            && matches!(next, '\'' | '\\')
        {
            unescaped.push(next);
            chars.next();
        } else {
            unescaped.push(c);
        }
    }
    unescaped
}

fn referential_action(rule: &str) -> ReferentialAction {
    match rule {
        "RESTRICT" => ReferentialAction::Restrict,
        "CASCADE" => ReferentialAction::Cascade,
        "SET NULL" => ReferentialAction::SetNull,
        "SET DEFAULT" => ReferentialAction::SetDefault,
        _ => ReferentialAction::NoAction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_defaults_become_sql() {
        assert_eq!(
            column_default(Some(r"it's \ x"), "varchar", "", false).as_deref(),
            Some(r"'it''s \\ x'")
        );
        assert_eq!(
            column_default(Some("1.50"), "decimal", "", false).as_deref(),
            Some("1.50")
        );
        assert_eq!(
            column_default(Some("b'101'"), "bit", "", false).as_deref(),
            Some("b'101'")
        );
        assert_eq!(column_default(None, "int", "", false), None);
    }

    #[test]
    fn expression_defaults_keep_their_expression() {
        assert_eq!(
            column_default(
                Some(r"concat(_utf8mb4\'a\',_utf8mb4\'b\')"),
                "varchar",
                "DEFAULT_GENERATED",
                false
            )
            .as_deref(),
            Some("(concat(_utf8mb4'a',_utf8mb4'b'))")
        );
        assert_eq!(
            column_default(
                Some("CURRENT_TIMESTAMP(3)"),
                "timestamp",
                "DEFAULT_GENERATED on update CURRENT_TIMESTAMP(3)",
                false
            )
            .as_deref(),
            Some("CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3)")
        );
    }

    #[test]
    fn mariadb_defaults_are_already_sql() {
        assert_eq!(
            column_default(Some("'x'"), "varchar", "", true).as_deref(),
            Some("'x'")
        );
        assert_eq!(column_default(Some("NULL"), "varchar", "", true), None);
    }
}
