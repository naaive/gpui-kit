//! Reading the catalog from `pg_catalog`.
//!
//! `information_schema` would be portable but is slow on large databases and
//! hides objects the user does not own; `pg_catalog` is what psql reads.
//! Definitions come from the server's own `pg_get_*def` functions, so DDL
//! shown to people is exactly what the server would accept.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_catalog::{
    Column, Constraint, ConstraintRule, ForeignKey, Index, ReferentialAction, Relation,
    RelationType, Role, Routine, RoutineType, Schema, Sequence, Trigger,
};
use datakit_driver::Dialect as _;
use tokio_postgres::{Client, types::ToSql};

const SCHEMAS: &str = "
    SELECT n.nspname::text,
           pg_get_userbyid(n.nspowner)::text,
           n.nspname IN ('pg_catalog', 'information_schema'),
           obj_description(n.oid, 'pg_namespace')
    FROM pg_namespace n
    WHERE n.nspname NOT LIKE 'pg\\_toast%'
      AND n.nspname NOT LIKE 'pg\\_temp\\_%'
    ORDER BY 3, 1";

const SCHEMA: &str = "
    SELECT n.nspname::text,
           pg_get_userbyid(n.nspowner)::text,
           n.nspname IN ('pg_catalog', 'information_schema'),
           obj_description(n.oid, 'pg_namespace')
    FROM pg_namespace n
    WHERE n.nspname = $1";

const RELATIONS: &str = "
    SELECT c.oid,
           c.relname::text,
           c.relkind::text,
           obj_description(c.oid, 'pg_class'),
           c.reltuples::float8,
           pg_get_userbyid(c.relowner)::text,
           CASE WHEN c.relkind IN ('v', 'm') THEN pg_get_viewdef(c.oid, true) END
    FROM pg_class c
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = $1
      AND c.relkind IN ('r', 'p', 'f', 'v', 'm')
      AND NOT c.relispartition
    ORDER BY c.relname";

const COLUMNS: &str = "
    SELECT a.attrelid,
           a.attname::text,
           format_type(a.atttypid, a.atttypmod),
           NOT a.attnotnull,
           pg_get_expr(d.adbin, d.adrelid),
           col_description(a.attrelid, a.attnum),
           coalesce(a.attnum = ANY (i.indkey), false),
           a.attidentity::text,
           a.attgenerated::text
    FROM pg_attribute a
    JOIN pg_class c ON c.oid = a.attrelid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
    LEFT JOIN pg_index i ON i.indrelid = a.attrelid AND i.indisprimary
    WHERE n.nspname = $1
      AND c.relkind IN ('r', 'p', 'f', 'v', 'm')
      AND a.attnum > 0
      AND NOT a.attisdropped
    ORDER BY a.attrelid, a.attnum";

const INDEXES: &str = "
    SELECT i.indrelid,
           ci.relname::text,
           i.indisunique,
           i.indisprimary,
           am.amname::text,
           pg_get_indexdef(i.indexrelid),
           pg_get_expr(i.indpred, i.indrelid),
           ARRAY(SELECT pg_get_indexdef(i.indexrelid, k, true)
                 FROM generate_series(1, i.indnkeyatts) k
                 ORDER BY k)::text[]
    FROM pg_index i
    JOIN pg_class ci ON ci.oid = i.indexrelid
    JOIN pg_class c ON c.oid = i.indrelid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    JOIN pg_am am ON am.oid = ci.relam
    WHERE n.nspname = $1
    ORDER BY i.indrelid, i.indisprimary DESC, ci.relname";

const CONSTRAINTS: &str = "
    SELECT c.conrelid,
           c.conname::text,
           c.contype::text,
           pg_get_constraintdef(c.oid, true),
           ARRAY(SELECT a.attname
                 FROM unnest(c.conkey) WITH ORDINALITY k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum
                 ORDER BY k.ord)::text[],
           fn.nspname::text,
           fc.relname::text,
           ARRAY(SELECT a.attname
                 FROM unnest(c.confkey) WITH ORDINALITY k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.attnum
                 ORDER BY k.ord)::text[],
           c.confupdtype::text,
           c.confdeltype::text,
           pg_get_expr(c.conbin, c.conrelid)
    FROM pg_constraint c
    JOIN pg_class r ON r.oid = c.conrelid
    JOIN pg_namespace n ON n.oid = r.relnamespace
    LEFT JOIN pg_class fc ON fc.oid = c.confrelid
    LEFT JOIN pg_namespace fn ON fn.oid = fc.relnamespace
    WHERE n.nspname = $1
      AND c.contype IN ('p', 'u', 'f', 'c', 'x')
    ORDER BY c.conrelid, c.contype = 'p' DESC, c.conname";

const TRIGGERS: &str = "
    SELECT t.tgrelid,
           t.tgname::text,
           t.tgtype::int4,
           t.tgenabled::text,
           pg_get_triggerdef(t.oid, true)
    FROM pg_trigger t
    JOIN pg_class c ON c.oid = t.tgrelid
    JOIN pg_namespace n ON n.oid = c.relnamespace
    WHERE n.nspname = $1
      AND NOT t.tgisinternal
    ORDER BY t.tgrelid, t.tgname";

const ROUTINES: &str = "
    SELECT p.proname::text,
           p.prokind::text,
           pg_get_function_identity_arguments(p.oid),
           CASE WHEN p.prokind <> 'p' THEN pg_get_function_result(p.oid) END,
           l.lanname::text,
           CASE WHEN p.prokind <> 'a' AND n.nspname NOT IN ('pg_catalog', 'information_schema')
                THEN pg_get_functiondef(p.oid) END,
           obj_description(p.oid, 'pg_proc')
    FROM pg_proc p
    JOIN pg_namespace n ON n.oid = p.pronamespace
    JOIN pg_language l ON l.oid = p.prolang
    WHERE n.nspname = $1
    ORDER BY p.proname, 3";

const SEQUENCES: &str = "
    SELECT s.relname::text,
           format_type(q.seqtypid, NULL),
           q.seqstart,
           q.seqincrement,
           q.seqmin,
           q.seqmax,
           q.seqcycle,
           (SELECT t.relname || '.' || a.attname
            FROM pg_depend d
            JOIN pg_class t ON t.oid = d.refobjid
            JOIN pg_attribute a ON a.attrelid = d.refobjid AND a.attnum = d.refobjsubid
            WHERE d.objid = s.oid
              AND d.classid = 'pg_class'::regclass
              AND d.refclassid = 'pg_class'::regclass
              AND d.deptype IN ('a', 'i')
            LIMIT 1)
    FROM pg_class s
    JOIN pg_sequence q ON q.seqrelid = s.oid
    JOIN pg_namespace n ON n.oid = s.relnamespace
    WHERE n.nspname = $1
      AND s.relkind = 'S'
    ORDER BY s.relname";

fn schema_from_row(row: &tokio_postgres::Row) -> Schema {
    let mut schema = Schema::new(row.get::<_, String>(0)).system(row.get(2));
    if let Some(owner) = row.get::<_, Option<String>>(1) {
        schema = schema.with_owner(owner);
    }
    if let Some(comment) = row.get::<_, Option<String>>(3) {
        schema = schema.with_comment(comment);
    }
    schema
}

pub(crate) async fn schemas(client: Arc<Client>) -> Result<Vec<Schema>> {
    let rows = client
        .query(SCHEMAS, &[])
        .await
        .context("Couldn’t read the schemas")?;
    Ok(rows.iter().map(schema_from_row).collect())
}

pub(crate) async fn schema(client: Arc<Client>, name: Arc<str>) -> Result<Schema> {
    let text: &str = &name;
    let parameters: [&(dyn ToSql + Sync); 1] = [&text];
    let (schema, relations, columns, indexes, constraints, triggers, routines, sequences) =
        futures::try_join!(
            client.query_opt(SCHEMA, &parameters),
            client.query(RELATIONS, &parameters),
            client.query(COLUMNS, &parameters),
            client.query(INDEXES, &parameters),
            client.query(CONSTRAINTS, &parameters),
            client.query(TRIGGERS, &parameters),
            client.query(ROUTINES, &parameters),
            client.query(SEQUENCES, &parameters),
        )
        .with_context(|| format!("Couldn’t read the objects of {name}"))?;

    let mut columns_by_relation: HashMap<u32, Vec<Column>> = HashMap::new();
    for row in &columns {
        let default = row.get::<_, Option<String>>(4);
        let identity = row.get::<_, String>(7);
        let generated = !row.get::<_, String>(8).is_empty();
        let serial = default
            .as_deref()
            .is_some_and(|default| default.starts_with("nextval("));
        let mut column = Column::new(row.get::<_, String>(1), row.get::<_, String>(2))
            .nullable(row.get(3))
            .primary_key(row.get(6))
            .generated(generated)
            .auto_increment(serial || !identity.is_empty());
        if let Some(default) = default {
            column = column.with_default(default);
        }
        if let Some(comment) = row.get::<_, Option<String>>(5) {
            column = column.with_comment(comment);
        }
        columns_by_relation
            .entry(row.get(0))
            .or_default()
            .push(column);
    }

    let mut indexes_by_relation: HashMap<u32, Vec<Index>> = HashMap::new();
    for row in &indexes {
        let mut index = Index::new(row.get::<_, String>(1), row.get::<_, Vec<String>>(7))
            .unique(row.get(2))
            .primary(row.get(3))
            .with_method(row.get::<_, String>(4))
            .with_definition(row.get::<_, String>(5));
        if let Some(predicate) = row.get::<_, Option<String>>(6) {
            index = index.with_predicate(predicate);
        }
        indexes_by_relation
            .entry(row.get(0))
            .or_default()
            .push(index);
    }

    let mut constraints_by_relation: HashMap<u32, Vec<Constraint>> = HashMap::new();
    for row in &constraints {
        let columns: Vec<String> = row.get(4);
        let columns: Arc<[Arc<str>]> = columns.into_iter().map(Arc::from).collect();
        let rule = match row.get::<_, String>(2).as_str() {
            "p" => ConstraintRule::PrimaryKey { columns },
            "u" => ConstraintRule::Unique { columns },
            "f" => ConstraintRule::ForeignKey(
                ForeignKey::new(
                    columns.iter().cloned(),
                    row.get::<_, Option<String>>(5).unwrap_or_default(),
                    row.get::<_, Option<String>>(6).unwrap_or_default(),
                    row.get::<_, Vec<String>>(7),
                )
                .with_on_update(referential_action(&row.get::<_, String>(8)))
                .with_on_delete(referential_action(&row.get::<_, String>(9))),
            ),
            "c" => ConstraintRule::Check {
                expression: row.get::<_, Option<String>>(10).unwrap_or_default().into(),
            },
            _ => ConstraintRule::Exclusion,
        };
        constraints_by_relation.entry(row.get(0)).or_default().push(
            Constraint::new(row.get::<_, String>(1), rule).with_definition(row.get::<_, String>(3)),
        );
    }

    let mut triggers_by_relation: HashMap<u32, Vec<Trigger>> = HashMap::new();
    for row in &triggers {
        let trigger = Trigger::new(row.get::<_, String>(1), trigger_timing(row.get(2)))
            .enabled(row.get::<_, String>(3) != "D")
            .with_definition(row.get::<_, String>(4));
        triggers_by_relation
            .entry(row.get(0))
            .or_default()
            .push(trigger);
    }

    let relations = relations.iter().filter_map(|row| {
        let oid: u32 = row.get(0);
        let relation_type = match row.get::<_, String>(2).as_str() {
            "r" => RelationType::Table,
            "p" => RelationType::PartitionedTable,
            "f" => RelationType::ForeignTable,
            "v" => RelationType::View,
            "m" => RelationType::MaterializedView,
            _ => return None,
        };
        let mut relation = Relation::new(row.get::<_, String>(1), relation_type)
            .with_columns(columns_by_relation.remove(&oid).unwrap_or_default())
            .with_indexes(indexes_by_relation.remove(&oid).unwrap_or_default())
            .with_constraints(constraints_by_relation.remove(&oid).unwrap_or_default())
            .with_triggers(triggers_by_relation.remove(&oid).unwrap_or_default());
        if let Some(comment) = row.get::<_, Option<String>>(3) {
            relation = relation.with_comment(comment);
        }
        // PostgreSQL 14 and later report -1 for a table never analyzed.
        let estimate: f64 = row.get(4);
        if estimate >= 0.0 {
            relation = relation.with_estimated_rows(estimate as i64);
        }
        if let Some(owner) = row.get::<_, Option<String>>(5) {
            relation = relation.with_owner(owner);
        }
        if let Some(definition) = row.get::<_, Option<String>>(6) {
            relation = relation.with_definition(definition.trim().trim_end_matches(';'));
        }
        Some(relation)
    });

    let routines = routines.iter().map(|row| {
        let routine_type = match row.get::<_, String>(1).as_str() {
            "p" => RoutineType::Procedure,
            "a" => RoutineType::Aggregate,
            "w" => RoutineType::Window,
            _ => RoutineType::Function,
        };
        let mut routine = Routine::new(
            row.get::<_, String>(0),
            routine_type,
            row.get::<_, Option<String>>(2).unwrap_or_default(),
        )
        .with_language(row.get::<_, String>(4));
        if let Some(result) = row.get::<_, Option<String>>(3) {
            routine = routine.with_result(result);
        }
        if let Some(definition) = row.get::<_, Option<String>>(5) {
            routine = routine.with_definition(definition);
        }
        if let Some(comment) = row.get::<_, Option<String>>(6) {
            routine = routine.with_comment(comment);
        }
        routine
    });

    let sequences = sequences.iter().map(|row| {
        let mut sequence = Sequence::new(row.get::<_, String>(0), row.get::<_, String>(1))
            .with_start(row.get(2))
            .with_increment(row.get(3))
            .with_range(row.get(4), row.get(5))
            .cycle(row.get(6));
        if let Some(owned_by) = row.get::<_, Option<String>>(7) {
            sequence = sequence.with_owned_by(owned_by);
        }
        sequence
    });

    let schema = match schema {
        Some(row) => schema_from_row(&row),
        None => Schema::new(name.clone()),
    };
    Ok(schema
        .with_relations(relations.collect::<Vec<_>>())
        .with_routines(routines.collect::<Vec<_>>())
        .with_sequences(sequences.collect::<Vec<_>>()))
}

const ROLES: &str = "
    SELECT r.rolname::text,
           r.rolcanlogin,
           r.rolsuper,
           r.rolinherit,
           r.rolcreaterole,
           r.rolcreatedb,
           r.rolreplication,
           r.rolbypassrls,
           r.rolconnlimit,
           r.rolvaliduntil::text,
           ARRAY(SELECT b.rolname::text
                 FROM pg_auth_members m
                 JOIN pg_roles b ON b.oid = m.roleid
                 WHERE m.member = r.oid
                 ORDER BY 1)
    FROM pg_roles r
    WHERE r.rolname !~ '^pg_'
    ORDER BY 1";

/// The server's roles, but not its predefined `pg_*` ones.
pub(crate) async fn roles(client: Arc<Client>) -> Result<Vec<Role>> {
    let rows = client
        .query(ROLES, &[])
        .await
        .context("Couldn’t read the roles")?;
    Ok(rows
        .iter()
        .map(|row| {
            let name: String = row.get(0);
            let can_login: bool = row.get(1);
            // Each flag, what CREATE ROLE assumes, and its two spellings.
            let flags = [
                (row.get::<_, bool>(2), false, "SUPERUSER", "NOSUPERUSER"),
                (row.get::<_, bool>(3), true, "INHERIT", "NOINHERIT"),
                (row.get::<_, bool>(4), false, "CREATEROLE", "NOCREATEROLE"),
                (row.get::<_, bool>(5), false, "CREATEDB", "NOCREATEDB"),
                (row.get::<_, bool>(6), false, "REPLICATION", "NOREPLICATION"),
                (row.get::<_, bool>(7), false, "BYPASSRLS", "NOBYPASSRLS"),
            ];
            let connection_limit: i32 = row.get(8);
            let valid_until: Option<String> = row.get(9);
            let member_of: Vec<String> = row.get(10);
            // Only what differs from CREATE ROLE's defaults is worth saying.
            let mut attributes: Vec<Arc<str>> = flags
                .iter()
                .filter(|(set, default, _, _)| set != default)
                .map(|(set, _, on, off)| Arc::from(if *set { *on } else { *off }))
                .collect();
            if connection_limit >= 0 {
                attributes.push(format!("CONNECTION LIMIT {connection_limit}").into());
            }
            if let Some(valid_until) = valid_until {
                attributes.push(format!("VALID UNTIL '{valid_until}'").into());
            }
            let quoted = quote(&name);
            let mut definition = format!(
                "CREATE ROLE {quoted} WITH {}",
                std::iter::once(if can_login { "LOGIN" } else { "NOLOGIN" })
                    .chain(attributes.iter().map(|attribute| &**attribute))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            definition.push(';');
            for parent in &member_of {
                definition.push_str(&format!(
                    "
GRANT {} TO {quoted};",
                    quote(parent)
                ));
            }
            Role::new(name, can_login)
                .with_attributes(attributes)
                .with_member_of(member_of.into_iter().map(Arc::from))
                .with_definition(definition)
        })
        .collect())
}

/// `name` as a PostgreSQL identifier.
fn quote(name: &str) -> String {
    crate::PostgresDialect.quote_identifier(name)
}

pub(crate) async fn search_path(client: Arc<Client>) -> Result<Vec<Arc<str>>> {
    let row = client
        .query_one("SELECT current_schemas(false)::text[]", &[])
        .await
        .context("Couldn’t read the search path")?;
    Ok(row
        .get::<_, Vec<String>>(0)
        .into_iter()
        .map(Arc::from)
        .collect())
}

fn referential_action(code: &str) -> ReferentialAction {
    match code {
        "r" => ReferentialAction::Restrict,
        "c" => ReferentialAction::Cascade,
        "n" => ReferentialAction::SetNull,
        "d" => ReferentialAction::SetDefault,
        _ => ReferentialAction::NoAction,
    }
}

/// When a trigger fires, from the bits of `pg_trigger.tgtype`.
fn trigger_timing(tgtype: i32) -> String {
    let timing = if tgtype & 2 != 0 {
        "BEFORE"
    } else if tgtype & 64 != 0 {
        "INSTEAD OF"
    } else {
        "AFTER"
    };
    let events: Vec<&str> = [
        (4, "INSERT"),
        (8, "DELETE"),
        (16, "UPDATE"),
        (32, "TRUNCATE"),
    ]
    .into_iter()
    .filter(|(bit, _)| tgtype & bit != 0)
    .map(|(_, event)| event)
    .collect();
    let level = if tgtype & 1 != 0 { "ROW" } else { "STATEMENT" };
    format!("{timing} {} FOR EACH {level}", events.join(" OR "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_timing_reads_the_type_bits() {
        // BEFORE (2) INSERT (4) OR UPDATE (16) FOR EACH ROW (1)
        assert_eq!(
            trigger_timing(1 | 2 | 4 | 16),
            "BEFORE INSERT OR UPDATE FOR EACH ROW"
        );
        assert_eq!(trigger_timing(8), "AFTER DELETE FOR EACH STATEMENT");
    }
}
