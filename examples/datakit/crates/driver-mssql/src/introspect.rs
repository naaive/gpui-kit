//! Reading the catalog from SQL Server's `sys` views.
//!
//! `INFORMATION_SCHEMA` would be portable but leaves out identity columns,
//! filtered and included index columns, triggers and descriptions. The
//! `sys.all_*` views list system objects too, so browsing the `sys` schema
//! works like browsing any other. Every query of a schema runs in one batch
//! with the schema's name as its only parameter: one round trip, several
//! result sets.
//!
//! Definitions of views, routines and triggers are `OBJECT_DEFINITION`, the
//! full `CREATE` statement as it was written; definitions of indexes and
//! constraints are composed from the catalog, since SQL Server keeps no text
//! for them. Expressions (defaults, checks, computed columns, filters) are
//! SQL Server's normalized text with its redundant outer parentheses
//! removed: `((0))` reads `0`.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_catalog::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationType, Role, Routine, RoutineType, Schema, Sequence, Trigger,
};
use datakit_driver::{Dialect as _, Value};

use crate::{SqlServerDialect, connection::Client, types};

/// One row of a catalog query, as values.
type Values = Box<[Value]>;

/// The description of a schema, for every schema or for the one named by
/// the batch's parameter.
const SCHEMA_COLUMNS: &str = "
    SELECT s.name,
           USER_NAME(s.principal_id),
           CAST(CASE WHEN s.name IN (N'sys', N'INFORMATION_SCHEMA', N'guest')
                       OR s.schema_id BETWEEN 16384 AND 16399
                     THEN 1 ELSE 0 END AS bit),
           CAST(ep.value AS nvarchar(4000))
    FROM sys.schemas s
    LEFT JOIN sys.extended_properties ep
           ON ep.class = 3 AND ep.major_id = s.schema_id AND ep.minor_id = 0
          AND ep.name = N'MS_Description'";

/// Everything in the schema named `@P1`, one result set per kind of object.
const SCHEMA_OBJECTS: &str = "
    SELECT o.object_id,
           o.name,
           RTRIM(o.type),
           CAST(ep.value AS nvarchar(4000)),
           (SELECT SUM(p.rows) FROM sys.partitions p
            WHERE p.object_id = o.object_id AND p.index_id IN (0, 1)),
           USER_NAME(OBJECTPROPERTY(o.object_id, 'OwnerId')),
           CASE WHEN o.type = 'V' AND o.is_ms_shipped = 0
                THEN OBJECT_DEFINITION(o.object_id) END
    FROM sys.all_objects o
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    LEFT JOIN sys.extended_properties ep
           ON ep.class = 1 AND ep.major_id = o.object_id AND ep.minor_id = 0
          AND ep.name = N'MS_Description'
    WHERE s.name = @P1 AND o.type IN ('U', 'V')
    ORDER BY o.name;

    SELECT c.object_id,
           c.name,
           t.name,
           CAST(c.max_length AS int),
           CAST(c.precision AS int),
           CAST(c.scale AS int),
           c.is_nullable,
           c.is_identity,
           c.is_computed,
           cc.definition,
           dc.definition,
           CAST(ep.value AS nvarchar(4000)),
           CAST(CASE WHEN EXISTS (
                    SELECT 1 FROM sys.indexes i
                    JOIN sys.index_columns ic
                      ON ic.object_id = i.object_id AND ic.index_id = i.index_id
                    WHERE i.object_id = c.object_id AND i.is_primary_key = 1
                      AND ic.column_id = c.column_id)
                THEN 1 ELSE 0 END AS bit),
           t.is_user_defined
    FROM sys.all_columns c
    JOIN sys.all_objects o ON o.object_id = c.object_id
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    JOIN sys.types t ON t.user_type_id = c.user_type_id
    LEFT JOIN sys.computed_columns cc
           ON cc.object_id = c.object_id AND cc.column_id = c.column_id
    LEFT JOIN sys.default_constraints dc ON dc.object_id = c.default_object_id
    LEFT JOIN sys.extended_properties ep
           ON ep.class = 1 AND ep.major_id = c.object_id AND ep.minor_id = c.column_id
          AND ep.name = N'MS_Description'
    WHERE s.name = @P1 AND o.type IN ('U', 'V')
    ORDER BY c.object_id, c.column_id;

    SELECT i.object_id,
           CAST(i.index_id AS int),
           i.name,
           i.is_unique,
           i.is_primary_key,
           CAST(i.type AS int),
           i.type_desc,
           i.filter_definition
    FROM sys.indexes i
    JOIN sys.all_objects o ON o.object_id = i.object_id
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    WHERE s.name = @P1 AND o.type IN ('U', 'V') AND i.type > 0 AND i.is_hypothetical = 0
    ORDER BY i.object_id, i.is_primary_key DESC, i.name;

    SELECT ic.object_id,
           CAST(ic.index_id AS int),
           c.name,
           ic.is_descending_key,
           ic.is_included_column,
           CAST(ic.key_ordinal AS int)
    FROM sys.index_columns ic
    JOIN sys.all_objects o ON o.object_id = ic.object_id
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    JOIN sys.all_columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
    WHERE s.name = @P1 AND o.type IN ('U', 'V')
    ORDER BY ic.object_id, ic.index_id, ic.is_included_column, ic.key_ordinal,
             ic.index_column_id;

    SELECT k.parent_object_id,
           k.name,
           RTRIM(k.type),
           CAST(k.unique_index_id AS int)
    FROM sys.key_constraints k
    JOIN sys.schemas s ON s.schema_id = k.schema_id
    WHERE s.name = @P1
    ORDER BY k.parent_object_id, k.type, k.name;

    SELECT fk.parent_object_id,
           fk.object_id,
           fk.name,
           rs.name,
           ro.name,
           CAST(fk.update_referential_action AS int),
           CAST(fk.delete_referential_action AS int)
    FROM sys.foreign_keys fk
    JOIN sys.schemas s ON s.schema_id = fk.schema_id
    JOIN sys.all_objects ro ON ro.object_id = fk.referenced_object_id
    JOIN sys.schemas rs ON rs.schema_id = ro.schema_id
    WHERE s.name = @P1
    ORDER BY fk.parent_object_id, fk.name;

    SELECT fkc.constraint_object_id,
           pc.name,
           rc.name
    FROM sys.foreign_key_columns fkc
    JOIN sys.foreign_keys fk ON fk.object_id = fkc.constraint_object_id
    JOIN sys.schemas s ON s.schema_id = fk.schema_id
    JOIN sys.all_columns pc
      ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
    JOIN sys.all_columns rc
      ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
    WHERE s.name = @P1
    ORDER BY fkc.constraint_object_id, fkc.constraint_column_id;

    SELECT cc.parent_object_id,
           cc.name,
           cc.definition
    FROM sys.check_constraints cc
    JOIN sys.schemas s ON s.schema_id = cc.schema_id
    WHERE s.name = @P1
    ORDER BY cc.parent_object_id, cc.name;

    SELECT t.parent_id,
           t.name,
           t.is_disabled,
           t.is_instead_of_trigger,
           CAST(OBJECTPROPERTY(t.object_id, 'ExecIsInsertTrigger') AS int),
           CAST(OBJECTPROPERTY(t.object_id, 'ExecIsUpdateTrigger') AS int),
           CAST(OBJECTPROPERTY(t.object_id, 'ExecIsDeleteTrigger') AS int),
           OBJECT_DEFINITION(t.object_id)
    FROM sys.triggers t
    JOIN sys.all_objects o ON o.object_id = t.parent_id
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    WHERE s.name = @P1 AND t.parent_class = 1
    ORDER BY t.parent_id, t.name;

    SELECT o.object_id,
           o.name,
           RTRIM(o.type),
           CASE WHEN o.is_ms_shipped = 0 THEN OBJECT_DEFINITION(o.object_id) END,
           CAST(ep.value AS nvarchar(4000))
    FROM sys.all_objects o
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    LEFT JOIN sys.extended_properties ep
           ON ep.class = 1 AND ep.major_id = o.object_id AND ep.minor_id = 0
          AND ep.name = N'MS_Description'
    WHERE s.name = @P1 AND o.type IN ('P', 'PC', 'X', 'FN', 'IF', 'TF', 'FS', 'FT', 'AF')
    ORDER BY o.name;

    SELECT p.object_id,
           CAST(p.parameter_id AS int),
           p.name,
           t.name,
           CAST(p.max_length AS int),
           CAST(p.precision AS int),
           CAST(p.scale AS int),
           p.is_output,
           p.is_readonly,
           t.is_user_defined
    FROM sys.all_parameters p
    JOIN sys.all_objects o ON o.object_id = p.object_id
    JOIN sys.schemas s ON s.schema_id = o.schema_id
    JOIN sys.types t ON t.user_type_id = p.user_type_id
    WHERE s.name = @P1 AND o.type IN ('P', 'PC', 'X', 'FN', 'IF', 'TF', 'FS', 'FT', 'AF')
    ORDER BY p.object_id, p.parameter_id;

    SELECT q.name,
           TYPE_NAME(q.user_type_id),
           TRY_CAST(q.start_value AS bigint),
           TRY_CAST(q.increment AS bigint),
           TRY_CAST(q.minimum_value AS bigint),
           TRY_CAST(q.maximum_value AS bigint),
           q.is_cycling
    FROM sys.sequences q
    JOIN sys.schemas s ON s.schema_id = q.schema_id
    WHERE s.name = @P1
    ORDER BY q.name";

/// The result sets of `sql` run with `parameter`, as values.
async fn query(client: &mut Client, sql: &str, parameter: &str) -> Result<Vec<Vec<Values>>> {
    let results = client
        .query(sql, &[&parameter])
        .await?
        .into_results()
        .await?;
    Ok(results
        .iter()
        .map(|rows| rows.iter().map(types::row_values).collect())
        .collect())
}

fn text(row: &[Value], ix: usize) -> Option<&str> {
    match row.get(ix) {
        Some(Value::Text(text)) => Some(text),
        _ => None,
    }
}

fn string(row: &[Value], ix: usize) -> String {
    text(row, ix).unwrap_or_default().to_string()
}

fn int(row: &[Value], ix: usize) -> Option<i64> {
    match row.get(ix) {
        Some(Value::Int(value)) => Some(*value),
        _ => None,
    }
}

fn flag(row: &[Value], ix: usize) -> bool {
    matches!(row.get(ix), Some(Value::Bool(true)) | Some(Value::Int(1)))
}

fn schema_from_row(row: &[Value]) -> Schema {
    let mut schema = Schema::new(string(row, 0)).system(flag(row, 2));
    if let Some(owner) = text(row, 1) {
        schema = schema.with_owner(owner);
    }
    if let Some(comment) = text(row, 3) {
        schema = schema.with_comment(comment);
    }
    schema
}

pub(crate) async fn schemas(client: &mut Client) -> Result<Vec<Schema>> {
    let rows = async {
        client
            .simple_query(format!("{SCHEMA_COLUMNS} ORDER BY 3, 1"))
            .await?
            .into_first_result()
            .await
    }
    .await
    .context("Couldn’t read the schemas")?;
    Ok(rows
        .iter()
        .map(|row| schema_from_row(&types::row_values(row)))
        .collect())
}

const PRINCIPALS: &str = "
    SELECT p.name, p.type_desc, p.is_disabled,
           STUFF((SELECT ',' + r.name
                  FROM sys.server_role_members m
                  JOIN sys.server_principals r ON r.principal_id = m.role_principal_id
                  WHERE m.member_principal_id = p.principal_id
                  ORDER BY r.name
                  FOR XML PATH('')), 1, 1, '')
    FROM sys.server_principals p
    WHERE p.type IN ('S', 'U', 'G', 'R', 'E', 'X')
      AND p.name NOT LIKE '##%'
    ORDER BY p.name";

/// The server's logins and server roles, as far as the login may see them.
pub(crate) async fn roles(client: &mut Client) -> Result<Vec<Role>> {
    let rows = async {
        client
            .simple_query(PRINCIPALS)
            .await?
            .into_first_result()
            .await
    }
    .await
    .context("Couldn’t read the logins")?;
    Ok(rows
        .iter()
        .map(|row| {
            let row = types::row_values(row);
            let name = string(&row, 0);
            let kind = string(&row, 1);
            let disabled = flag(&row, 2);
            let member_of: Vec<Arc<str>> = text(&row, 3)
                .unwrap_or_default()
                .split(',')
                .filter(|role| !role.is_empty())
                .map(Arc::from)
                .collect();
            let quoted = SqlServerDialect.quote_identifier(&name);
            let is_role = kind == "SERVER_ROLE";
            let mut definition = match kind.as_str() {
                "SERVER_ROLE" => format!("CREATE SERVER ROLE {quoted};"),
                "SQL_LOGIN" => format!("CREATE LOGIN {quoted} WITH PASSWORD = N'…';"),
                _ => format!("CREATE LOGIN {quoted} FROM WINDOWS;"),
            };
            if disabled && !is_role {
                definition.push_str(&format!("\nALTER LOGIN {quoted} DISABLE;"));
            }
            for role in &member_of {
                definition.push_str(&format!(
                    "\nALTER SERVER ROLE {} ADD MEMBER {quoted};",
                    SqlServerDialect.quote_identifier(role)
                ));
            }
            let mut attributes = vec![Arc::from(kind.replace('_', " "))];
            if disabled {
                attributes.push("DISABLE".into());
            }
            Role::new(name, !is_role && !disabled)
                .with_attributes(attributes)
                .with_member_of(member_of)
                .with_definition(definition)
        })
        .collect())
}

pub(crate) async fn search_path(client: &mut Client) -> Result<Vec<Arc<str>>> {
    let row = client
        .simple_query("SELECT SCHEMA_NAME()")
        .await?
        .into_row()
        .await
        .context("Couldn’t read the default schema")?;
    Ok(row
        .map(|row| types::row_values(&row))
        .and_then(|row| text(&row, 0).map(Arc::from))
        .into_iter()
        .collect())
}

/// An index as the catalog lists it, before it becomes an [`Index`].
struct IndexEntry {
    name: String,
    unique: bool,
    primary: bool,
    /// `sys.indexes.type`: 1 clustered, 2 nonclustered, 5 and 6 columnstore.
    kind: i64,
    method: String,
    filter: Option<String>,
    /// Key columns with whether each is descending.
    keys: Vec<(String, bool)>,
    included: Vec<String>,
}

pub(crate) async fn schema(client: &mut Client, name: Arc<str>) -> Result<Schema> {
    let sql = format!(
        "{SCHEMA_COLUMNS} WHERE s.name = @P1;
{SCHEMA_OBJECTS}"
    );
    let results = query(client, &sql, &name)
        .await
        .with_context(|| format!("Couldn’t read the objects of {name}"))?;
    let [
        header,
        relations,
        columns,
        indexes,
        index_columns,
        keys,
        foreign_keys,
        foreign_key_columns,
        checks,
        triggers,
        routines,
        parameters,
        sequences,
    ] = <[Vec<Values>; 13]>::try_from(results).map_err(|results| {
        anyhow::anyhow!("SQL Server returned {} results, not 13", results.len())
    })?;
    let schema = header
        .first()
        .map_or_else(|| Schema::new(name.clone()), |row| schema_from_row(row));
    let dialect = SqlServerDialect;

    let mut columns_by_relation: HashMap<i64, Vec<Column>> = HashMap::new();
    for row in &columns {
        let data_type = type_spelling(
            &string(row, 2),
            int(row, 3).unwrap_or(0),
            int(row, 4).unwrap_or(0),
            int(row, 5).unwrap_or(0),
            flag(row, 13),
        );
        let computed = flag(row, 8);
        let mut column = Column::new(string(row, 1), data_type)
            .nullable(flag(row, 6))
            .auto_increment(flag(row, 7))
            .generated(computed)
            .primary_key(flag(row, 12));
        let default = if computed {
            text(row, 9)
        } else {
            text(row, 10)
        };
        if let Some(default) = default {
            column = column.with_default(strip_parentheses(default));
        }
        if let Some(comment) = text(row, 11) {
            column = column.with_comment(comment);
        }
        columns_by_relation
            .entry(int(row, 0).unwrap_or_default())
            .or_default()
            .push(column);
    }

    let mut entries: HashMap<(i64, i64), IndexEntry> = HashMap::new();
    let mut index_order: Vec<(i64, i64)> = Vec::new();
    for row in &indexes {
        let key = (
            int(row, 0).unwrap_or_default(),
            int(row, 1).unwrap_or_default(),
        );
        index_order.push(key);
        entries.insert(
            key,
            IndexEntry {
                name: string(row, 2),
                unique: flag(row, 3),
                primary: flag(row, 4),
                kind: int(row, 5).unwrap_or_default(),
                method: string(row, 6).to_lowercase(),
                filter: text(row, 7).map(|filter| strip_parentheses(filter).to_string()),
                keys: Vec::new(),
                included: Vec::new(),
            },
        );
    }
    for row in &index_columns {
        let key = (
            int(row, 0).unwrap_or_default(),
            int(row, 1).unwrap_or_default(),
        );
        let Some(entry) = entries.get_mut(&key) else {
            continue;
        };
        let column = string(row, 2);
        if flag(row, 4) {
            entry.included.push(column);
        } else if int(row, 5).unwrap_or(0) > 0 || matches!(entry.kind, 5 | 6) {
            entry.keys.push((column, flag(row, 3)));
        }
    }

    // Constraints, with the index each key constraint is enforced by.
    let mut constraints_by_relation: HashMap<i64, Vec<Constraint>> = HashMap::new();
    for row in &keys {
        let relation = int(row, 0).unwrap_or_default();
        let Some(entry) = entries.get(&(relation, int(row, 3).unwrap_or_default())) else {
            continue;
        };
        let columns: Arc<[Arc<str>]> = entry
            .keys
            .iter()
            .map(|(column, _)| Arc::from(column.as_str()))
            .collect();
        let primary = text(row, 2) == Some("PK");
        let rule = if primary {
            ConstraintRule::PrimaryKey { columns }
        } else {
            ConstraintRule::Unique { columns }
        };
        let definition = format!(
            "{} {} ({})",
            if primary { "PRIMARY KEY" } else { "UNIQUE" },
            entry.method.to_uppercase(),
            key_list(&dialect, &entry.keys)
        );
        constraints_by_relation
            .entry(relation)
            .or_default()
            .push(Constraint::new(string(row, 1), rule).with_definition(definition));
    }

    let mut key_columns: HashMap<i64, (Vec<String>, Vec<String>)> = HashMap::new();
    for row in &foreign_key_columns {
        let (columns, referenced) = key_columns
            .entry(int(row, 0).unwrap_or_default())
            .or_default();
        columns.push(string(row, 1));
        referenced.push(string(row, 2));
    }
    for row in &foreign_keys {
        let (columns, referenced) = key_columns
            .remove(&int(row, 1).unwrap_or_default())
            .unwrap_or_default();
        let key = ForeignKey::new(columns, string(row, 3), string(row, 4), referenced)
            .with_on_update(referential_action(int(row, 5)))
            .with_on_delete(referential_action(int(row, 6)));
        let constraint = Constraint::new(string(row, 2), ConstraintRule::ForeignKey(key));
        let definition = dialect.constraint_definition(&constraint);
        let body = definition
            .split_once(' ')
            .and_then(|(_, rest)| rest.split_once(' '))
            .map_or(definition.as_str(), |(_, body)| body)
            .to_string();
        constraints_by_relation
            .entry(int(row, 0).unwrap_or_default())
            .or_default()
            .push(constraint.with_definition(body));
    }

    for row in &checks {
        let expression = strip_parentheses(text(row, 2).unwrap_or_default()).to_string();
        let definition = format!("CHECK ({expression})");
        constraints_by_relation
            .entry(int(row, 0).unwrap_or_default())
            .or_default()
            .push(
                Constraint::new(
                    string(row, 1),
                    ConstraintRule::Check {
                        expression: expression.into(),
                    },
                )
                .with_definition(definition),
            );
    }

    let mut triggers_by_relation: HashMap<i64, Vec<Trigger>> = HashMap::new();
    for row in &triggers {
        let events: Vec<&str> = [(4, "INSERT"), (5, "UPDATE"), (6, "DELETE")]
            .into_iter()
            .filter(|(ix, _)| int(row, *ix) == Some(1))
            .map(|(_, event)| event)
            .collect();
        let timing = format!(
            "{} {}",
            if flag(row, 3) { "INSTEAD OF" } else { "AFTER" },
            events.join(", ")
        );
        let mut trigger = Trigger::new(string(row, 1), timing).enabled(!flag(row, 2));
        if let Some(definition) = text(row, 7) {
            trigger = trigger.with_definition(definition.trim());
        }
        triggers_by_relation
            .entry(int(row, 0).unwrap_or_default())
            .or_default()
            .push(trigger);
    }

    let mut indexes_by_relation: HashMap<i64, Vec<Index>> = HashMap::new();
    for key in index_order {
        let Some(entry) = entries.remove(&key) else {
            continue;
        };
        let relation = relations
            .iter()
            .find(|row| int(row, 0) == Some(key.0))
            .map(|row| string(row, 1))
            .unwrap_or_default();
        let definition = index_definition(&dialect, &name, &relation, &entry);
        let columns: Vec<String> = entry
            .keys
            .iter()
            .map(|(column, descending)| {
                let column = dialect.quote_identifier(column);
                if *descending {
                    format!("{column} DESC")
                } else {
                    column
                }
            })
            .collect();
        let mut index = Index::new(entry.name, columns)
            .unique(entry.unique)
            .primary(entry.primary)
            .with_method(entry.method);
        if let Some(filter) = entry.filter {
            index = index.with_predicate(filter);
        }
        if let Some(definition) = definition {
            index = index.with_definition(definition);
        }
        indexes_by_relation.entry(key.0).or_default().push(index);
    }

    let relations: Vec<Relation> = relations
        .iter()
        .filter_map(|row| {
            let id = int(row, 0)?;
            let relation_type = match text(row, 2)? {
                "U" => RelationType::Table,
                "V" => RelationType::View,
                _ => return None,
            };
            let mut relation = Relation::new(string(row, 1), relation_type)
                .with_columns(columns_by_relation.remove(&id).unwrap_or_default())
                .with_indexes(indexes_by_relation.remove(&id).unwrap_or_default())
                .with_constraints(constraints_by_relation.remove(&id).unwrap_or_default())
                .with_triggers(triggers_by_relation.remove(&id).unwrap_or_default());
            if let Some(comment) = text(row, 3) {
                relation = relation.with_comment(comment);
            }
            if let Some(rows) = int(row, 4) {
                relation = relation.with_estimated_rows(rows);
            }
            if let Some(owner) = text(row, 5) {
                relation = relation.with_owner(owner);
            }
            if let Some(definition) = text(row, 6) {
                relation = relation.with_definition(definition.trim().trim_end_matches(';'));
            }
            Some(relation)
        })
        .collect();

    let mut parameters_by_routine: HashMap<i64, Vec<&Values>> = HashMap::new();
    for row in &parameters {
        parameters_by_routine
            .entry(int(row, 0).unwrap_or_default())
            .or_default()
            .push(row);
    }
    let routines: Vec<Routine> = routines
        .iter()
        .map(|row| {
            let code = text(row, 2).unwrap_or_default();
            let (routine_type, language) = match code {
                "P" => (RoutineType::Procedure, "SQL"),
                "PC" => (RoutineType::Procedure, "CLR"),
                "X" => (RoutineType::Procedure, "extended"),
                "AF" => (RoutineType::Aggregate, "CLR"),
                "FS" | "FT" => (RoutineType::Function, "CLR"),
                _ => (RoutineType::Function, "SQL"),
            };
            let parameters = parameters_by_routine
                .remove(&int(row, 0).unwrap_or_default())
                .unwrap_or_default();
            let arguments: Vec<String> = parameters
                .iter()
                .filter(|parameter| int(parameter, 1).unwrap_or(0) > 0)
                .map(|parameter| {
                    let mut argument =
                        format!("{} {}", string(parameter, 2), parameter_type(parameter));
                    if flag(parameter, 7) {
                        argument.push_str(" OUTPUT");
                    }
                    if flag(parameter, 8) {
                        argument.push_str(" READONLY");
                    }
                    argument
                })
                .collect();
            let result = match code {
                "IF" | "TF" | "FT" => Some("TABLE".to_string()),
                _ => parameters
                    .iter()
                    .find(|parameter| int(parameter, 1) == Some(0))
                    .map(|parameter| parameter_type(parameter)),
            };
            let mut routine = Routine::new(string(row, 1), routine_type, arguments.join(", "))
                .with_language(language);
            if let Some(result) = result {
                routine = routine.with_result(result);
            }
            if let Some(definition) = text(row, 3) {
                routine = routine.with_definition(definition.trim());
            }
            if let Some(comment) = text(row, 4) {
                routine = routine.with_comment(comment);
            }
            routine
        })
        .collect();

    let sequences: Vec<Sequence> = sequences
        .iter()
        .map(|row| {
            Sequence::new(string(row, 0), string(row, 1))
                .with_start(int(row, 2).unwrap_or(1))
                .with_increment(int(row, 3).unwrap_or(1))
                .with_range(
                    int(row, 4).unwrap_or(i64::MIN),
                    int(row, 5).unwrap_or(i64::MAX),
                )
                .cycle(flag(row, 6))
        })
        .collect();

    Ok(schema
        .with_relations(relations)
        .with_routines(routines)
        .with_sequences(sequences))
}

/// A parameter's type, from a row of the parameters query.
fn parameter_type(row: &[Value]) -> String {
    type_spelling(
        &string(row, 3),
        int(row, 4).unwrap_or(0),
        int(row, 5).unwrap_or(0),
        int(row, 6).unwrap_or(0),
        flag(row, 9),
    )
}

/// A type as a column definition spells it: `nvarchar(50)`,
/// `varchar(max)`, `decimal(10,2)`, `datetime2(3)`.
///
/// `max_length` is in bytes, as `sys.columns` reports it, and `-1` for
/// `max`; a Unicode character takes two. An alias type is spelled by its
/// name alone, since its length is part of it.
pub(crate) fn type_spelling(
    name: &str,
    max_length: i64,
    precision: i64,
    scale: i64,
    user_defined: bool,
) -> String {
    if user_defined {
        return name.to_string();
    }
    let length = |characters: i64| {
        if max_length == -1 {
            format!("{name}(max)")
        } else {
            format!("{name}({characters})")
        }
    };
    match name {
        "varchar" | "char" | "varbinary" | "binary" => length(max_length),
        "nvarchar" | "nchar" => length(max_length / 2),
        "decimal" | "numeric" => format!("{name}({precision},{scale})"),
        // Seven digits is the default and goes unsaid.
        "datetime2" | "time" | "datetimeoffset" if scale != 7 => format!("{name}({scale})"),
        _ => name.to_string(),
    }
}

/// The `CREATE INDEX` statement for `entry`; `None` for kinds of index
/// whose options the catalog query does not read (XML, spatial, hash).
fn index_definition(
    dialect: &SqlServerDialect,
    schema: &str,
    relation: &str,
    entry: &IndexEntry,
) -> Option<String> {
    let table = dialect.qualified_name(schema, relation);
    let name = dialect.quote_identifier(&entry.name);
    let unique = if entry.unique { "UNIQUE " } else { "" };
    let mut sql = match entry.kind {
        1 | 2 => format!(
            "CREATE {unique}{} INDEX {name} ON {table} ({})",
            entry.method.to_uppercase(),
            key_list(dialect, &entry.keys)
        ),
        5 => format!("CREATE CLUSTERED COLUMNSTORE INDEX {name} ON {table}"),
        6 => format!(
            "CREATE NONCLUSTERED COLUMNSTORE INDEX {name} ON {table} ({})",
            key_list(dialect, &entry.keys)
        ),
        _ => return None,
    };
    if !entry.included.is_empty() {
        let included: Vec<String> = entry
            .included
            .iter()
            .map(|column| dialect.quote_identifier(column))
            .collect();
        sql.push_str(&format!(" INCLUDE ({})", included.join(", ")));
    }
    if let Some(filter) = &entry.filter {
        sql.push_str(&format!(" WHERE {filter}"));
    }
    Some(sql)
}

/// Key columns, quoted, each with `DESC` when descending.
fn key_list(dialect: &SqlServerDialect, keys: &[(String, bool)]) -> String {
    keys.iter()
        .map(|(column, descending)| {
            let column = dialect.quote_identifier(column);
            if *descending {
                format!("{column} DESC")
            } else {
                column
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `sys.foreign_keys.*_referential_action`.
fn referential_action(code: Option<i64>) -> ReferentialAction {
    match code {
        Some(1) => ReferentialAction::Cascade,
        Some(2) => ReferentialAction::SetNull,
        Some(3) => ReferentialAction::SetDefault,
        _ => ReferentialAction::NoAction,
    }
}

/// `text` without the parentheses SQL Server wraps around every expression
/// it stores: `((0))` is `0`, `([a]>(1))` is `[a]>(1)`. Parentheses that do
/// not enclose the whole text, like those of `(a)+(b)`, stay.
pub(crate) fn strip_parentheses(text: &str) -> &str {
    let mut text = text.trim();
    while text.starts_with('(') && text.ends_with(')') && encloses(text) {
        text = text[1..text.len() - 1].trim();
    }
    text
}

/// Whether the `(` that starts `text` is closed by the `)` that ends it,
/// skipping parentheses in string literals and bracketed names.
fn encloses(text: &str) -> bool {
    let mut depth = 0usize;
    let mut quoted = None;
    for (ix, c) in text.char_indices() {
        if let Some(close) = quoted {
            if c == close {
                quoted = None;
            }
            continue;
        }
        match c {
            '\'' => quoted = Some('\''),
            '[' => quoted = Some(']'),
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return ix == text.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_spelled_with_their_length_and_precision() {
        assert_eq!(type_spelling("nvarchar", 100, 0, 0, false), "nvarchar(50)");
        assert_eq!(type_spelling("varchar", -1, 0, 0, false), "varchar(max)");
        assert_eq!(
            type_spelling("varbinary", -1, 0, 0, false),
            "varbinary(max)"
        );
        assert_eq!(type_spelling("decimal", 9, 10, 2, false), "decimal(10,2)");
        assert_eq!(type_spelling("datetime2", 8, 27, 7, false), "datetime2");
        assert_eq!(type_spelling("datetime2", 7, 23, 3, false), "datetime2(3)");
        assert_eq!(type_spelling("int", 4, 10, 0, false), "int");
        assert_eq!(type_spelling("Phone", 20, 0, 0, true), "Phone");
    }

    #[test]
    fn redundant_parentheses_are_removed() {
        assert_eq!(strip_parentheses("((0))"), "0");
        assert_eq!(strip_parentheses("(getdate())"), "getdate()");
        assert_eq!(strip_parentheses("([total]>=(0))"), "[total]>=(0)");
        assert_eq!(strip_parentheses("(a)+(b)"), "(a)+(b)");
        assert_eq!(strip_parentheses("(N'(')"), "N'('");
        assert_eq!(strip_parentheses("([a)]>(1))"), "[a)]>(1)");
    }
}
