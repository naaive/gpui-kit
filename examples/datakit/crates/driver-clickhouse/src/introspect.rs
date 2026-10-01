//! Reading the catalog from ClickHouse's `system` tables.
//!
//! A ClickHouse database is a schema. Its tables carry what the catalog has
//! no field for in the places closest in meaning:
//!
//! - A table's definition is its engine clause (`engine_full`), such as
//!   `ReplacingMergeTree(version) ORDER BY id SETTINGS …`, which
//!   [`ClickHouseDialect`](crate::ClickHouseDialect) repeats when it writes
//!   `CREATE TABLE`. A view's is its query; a materialized view's is
//!   everything after its name, `TO target AS SELECT …` or an engine and the
//!   query, since it cannot be recreated without them.
//! - The primary key is an index marked primary, whose method is the engine.
//!   The columns in it are marked as primary key columns, which is what the
//!   data editor finds rows by — though ClickHouse does not keep keys unique.
//! - Data skipping indexes are indexes whose method is their type and whose
//!   definition is the `INDEX … TYPE … GRANULARITY …` clause.
//! - `CHECK` and `ASSUME` constraints are read from the `CREATE TABLE`
//!   statement the server keeps, there being no table of them.
//!
//! SQL functions belong to the server, not a database; they are listed in
//! the database a session starts in. Every query runs outside the session,
//! so introspection never waits behind a statement the console is running.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_catalog::{
    Column, Constraint, ConstraintRule, Index, Relation, RelationType, Role, Routine, RoutineType,
    Schema,
};
use datakit_driver::Dialect as _;

use crate::{
    ClickHouseDialect,
    http::{Http, Record},
    text, types,
};

/// Databases ClickHouse itself maintains.
const SYSTEM_DATABASES: &[&str] = &["system", "INFORMATION_SCHEMA", "information_schema"];

// `SELECT *` where a column is newer than the oldest supported server, so a
// missing column reads as absent instead of failing the query.
const DATABASES: &str = "SELECT * FROM system.databases ORDER BY name";

const DATABASE: &str = "SELECT * FROM system.databases WHERE name = {schema:String}";

const TABLES: &str = "
    SELECT *
    FROM system.tables
    WHERE database = {schema:String} AND NOT is_temporary
    ORDER BY name";

const COLUMNS: &str = "
    SELECT table, name, type, default_kind, default_expression, comment, is_in_primary_key
    FROM system.columns
    WHERE database = {schema:String}
    ORDER BY table, position";

const INDEXES: &str = "
    SELECT *
    FROM system.data_skipping_indices
    WHERE database = {schema:String}
    ORDER BY table, name";

const ROUTINES: &str = "
    SELECT name, create_query
    FROM system.functions
    WHERE origin = 'SQLUserDefined'
    ORDER BY name";

fn schema_from_record(record: Record) -> Schema {
    let name = record.text("name");
    let mut schema = Schema::new(name).system(SYSTEM_DATABASES.contains(&name));
    if let Some(comment) = record.get("comment").filter(|comment| !comment.is_empty()) {
        schema = schema.with_comment(comment);
    }
    schema
}

const USERS: &str = "SELECT name, 1 AS login FROM system.users \
    UNION ALL SELECT name, 0 AS login FROM system.roles ORDER BY name";
const ROLE_GRANTS: &str = "SELECT ifNull(user_name, role_name) AS grantee, granted_role_name \
    FROM system.role_grants";

/// The server's users and roles; a user without `SHOW USERS` sees none.
pub(crate) async fn roles(http: Http) -> Result<Vec<Role>> {
    let (users, grants) = futures::try_join!(http.query(USERS, &[]), http.query(ROLE_GRANTS, &[]),)
        .context("Couldn’t read the users and roles")?;
    let mut granted: HashMap<&str, Vec<Arc<str>>> = HashMap::new();
    for grant in grants.records() {
        granted
            .entry(grant.text("grantee"))
            .or_default()
            .push(grant.text("granted_role_name").into());
    }
    Ok(users
        .records()
        .map(|record| {
            let name = record.text("name");
            let login = record.text("login") == "1";
            let quoted = ClickHouseDialect.quote_identifier(name);
            let member_of = granted.get(name).cloned().unwrap_or_default();
            let mut definition =
                format!("CREATE {} {quoted};", if login { "USER" } else { "ROLE" });
            for role in &member_of {
                definition.push_str(&format!(
                    "\nGRANT {} TO {quoted};",
                    ClickHouseDialect.quote_identifier(role)
                ));
            }
            Role::new(name, login)
                .with_member_of(member_of)
                .with_definition(definition)
        })
        .collect())
}

pub(crate) async fn schemas(http: Http) -> Result<Vec<Schema>> {
    let databases = http
        .query(DATABASES, &[])
        .await
        .context("Couldn’t read the databases")?;
    let mut schemas: Vec<Schema> = databases.records().map(schema_from_record).collect();
    // User databases first, like PostgreSQL's schemas.
    schemas.sort_by_key(|schema| (schema.is_system(), schema.name()));
    Ok(schemas)
}

/// Everything in the database `name`, with the server's SQL functions when
/// `routines` is set.
pub(crate) async fn schema(http: Http, name: Arc<str>, routines: bool) -> Result<Schema> {
    let parameters = [("param_schema", &*name)];
    let (database, tables, columns, indexes) = futures::try_join!(
        http.query(DATABASE, &parameters),
        http.query(TABLES, &parameters),
        http.query(COLUMNS, &parameters),
        http.query(INDEXES, &parameters),
    )
    .with_context(|| format!("Couldn’t read the objects of {name}"))?;
    // `origin` is newer than the oldest supported server; without it there
    // are no functions to list.
    let routines = if routines {
        http.query(ROUTINES, &[])
            .await
            .inspect_err(|error| tracing::debug!("couldn’t read SQL functions: {error}"))
            .ok()
    } else {
        None
    };

    let mut columns_by_table: HashMap<&str, Vec<Column>> = HashMap::new();
    for record in columns.records() {
        columns_by_table
            .entry(record.text("table"))
            .or_default()
            .push(column(record));
    }

    let mut indexes_by_table: HashMap<&str, Vec<Index>> = HashMap::new();
    for record in indexes.records() {
        indexes_by_table
            .entry(record.text("table"))
            .or_default()
            .push(skipping_index(record));
    }

    let relations = tables.records().map(|record| {
        let name = record.text("name");
        let engine = record.text("engine");
        let create = record.text("create_table_query");
        let relation_type = match engine {
            "View" | "LiveView" | "WindowView" => RelationType::View,
            "MaterializedView" => RelationType::MaterializedView,
            _ => RelationType::Table,
        };
        let mut relation = Relation::new(name, relation_type)
            .with_columns(columns_by_table.remove(name).unwrap_or_default());
        match relation_type {
            RelationType::View => {
                relation = relation.with_definition(view_query(create));
            }
            RelationType::MaterializedView => {
                relation = relation.with_definition(after_name(create));
            }
            _ => {
                let mut indexes = Vec::new();
                let key = record.text("primary_key");
                if !key.is_empty() {
                    let columns: Vec<String> = text::split_top_level(key, ',')
                        .into_iter()
                        .map(text::unquote)
                        .collect();
                    indexes.push(
                        Index::new("primary_key", columns)
                            .primary(true)
                            .with_method(engine),
                    );
                }
                indexes.extend(indexes_by_table.remove(name).unwrap_or_default());
                relation = relation
                    .with_indexes(indexes)
                    .with_constraints(constraints(create));
                if let Some(engine) = record
                    .get("engine_full")
                    .filter(|engine| !engine.is_empty())
                {
                    relation = relation.with_definition(engine);
                }
            }
        }
        if let Some(comment) = record.get("comment").filter(|comment| !comment.is_empty()) {
            relation = relation.with_comment(comment);
        }
        if let Some(rows) = record
            .get("total_rows")
            .and_then(|rows| rows.parse::<u64>().ok())
        {
            relation = relation.with_estimated_rows(rows.min(i64::MAX as u64) as i64);
        }
        relation
    });
    let relations: Vec<Relation> = relations.collect();

    let routines: Vec<Routine> = routines
        .iter()
        .flat_map(|routines| routines.records())
        .map(|record| routine(record.text("name"), record.text("create_query")))
        .collect();

    let schema = match database.records().next() {
        Some(record) => schema_from_record(record),
        None => Schema::new(name.clone()).system(SYSTEM_DATABASES.contains(&&*name)),
    };
    Ok(schema.with_relations(relations).with_routines(routines))
}

fn column(record: Record) -> Column {
    let data_type = record.text("type");
    let expression = record.text("default_expression");
    let mut column = Column::new(record.text("name"), data_type)
        .nullable(types::is_nullable(data_type))
        .primary_key(record.text("is_in_primary_key") == "1");
    if !expression.is_empty() {
        // `MATERIALIZED` and `ALIAS` columns are computed; `DEFAULT` and
        // `EPHEMERAL` ones only start from the expression.
        let computed = matches!(record.text("default_kind"), "MATERIALIZED" | "ALIAS");
        column = column.generated(computed).with_default(expression);
    }
    if let Some(comment) = record.get("comment").filter(|comment| !comment.is_empty()) {
        column = column.with_comment(comment);
    }
    column
}

fn skipping_index(record: Record) -> Index {
    let name = record.text("name");
    let expression = record.text("expr");
    let index_type = record
        .get("type_full")
        .filter(|ty| !ty.is_empty())
        .unwrap_or(record.text("type"));
    let granularity = record.get("granularity").unwrap_or("1");
    Index::new(name, [expression])
        .with_method(index_type)
        .with_definition(format!(
            "INDEX {} {expression} TYPE {index_type} GRANULARITY {granularity}",
            ClickHouseDialect.quote_identifier(name)
        ))
}

/// The query of a view, from its `CREATE VIEW` statement.
fn view_query(create: &str) -> &str {
    match text::find_keyword(create, "AS") {
        Some(ix) => create[ix + "AS".len()..].trim(),
        None => create,
    }
}

/// What follows the name in a `CREATE` statement.
fn after_name(create: &str) -> &str {
    let mut rest = create.trim_start();
    // Everything up to the name is keywords: `CREATE MATERIALIZED VIEW IF
    // NOT EXISTS`.
    for keyword in [
        "CREATE",
        "OR",
        "REPLACE",
        "MATERIALIZED",
        "VIEW",
        "TABLE",
        "IF",
        "NOT",
        "EXISTS",
    ] {
        if rest.len() >= keyword.len()
            && rest[..keyword.len()].eq_ignore_ascii_case(keyword)
            && rest[keyword.len()..].starts_with(char::is_whitespace)
        {
            rest = rest[keyword.len()..].trim_start();
        }
    }
    text::skip_name(rest).trim()
}

/// The `CHECK` and `ASSUME` constraints declared in a `CREATE TABLE`
/// statement.
fn constraints(create: &str) -> Vec<Constraint> {
    let Some((elements, _)) = text::bracketed(after_name(create)) else {
        return Vec::new();
    };
    text::split_top_level(elements, ',')
        .into_iter()
        .filter_map(|element| {
            let rest = element.strip_prefix("CONSTRAINT ")?.trim_start();
            let after = text::skip_name(rest);
            let name = text::unquote(&rest[..rest.len() - after.len()]);
            let body = after.trim();
            let expression = ["CHECK ", "ASSUME "]
                .iter()
                .find_map(|keyword| body.strip_prefix(keyword))?
                .trim();
            Some(
                Constraint::new(
                    name,
                    ConstraintRule::Check {
                        expression: expression.into(),
                    },
                )
                .with_definition(body),
            )
        })
        .collect()
}

/// A SQL function, from its `CREATE FUNCTION name AS (a, b) -> …`.
fn routine(name: &str, create: &str) -> Routine {
    let lambda = text::find_keyword(create, "AS")
        .map(|ix| create[ix + "AS".len()..].trim())
        .unwrap_or_default();
    let arguments = match text::bracketed(lambda) {
        Some((arguments, _)) => arguments.trim(),
        None => lambda.split("->").next().unwrap_or_default().trim(),
    };
    let mut routine = Routine::new(name, RoutineType::Function, arguments).with_language("SQL");
    if !create.is_empty() {
        routine = routine.with_definition(create);
    }
    routine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_definition_is_its_query() {
        assert_eq!(
            view_query(
                "CREATE VIEW shop.recent (`id` UInt64, `as` String) AS SELECT id, note AS as FROM shop.events"
            ),
            "SELECT id, note AS as FROM shop.events"
        );
    }

    #[test]
    fn a_materialized_view_keeps_what_follows_its_name() {
        assert_eq!(
            after_name(
                "CREATE MATERIALIZED VIEW shop.`daily counts` TO shop.daily (`n` UInt64) AS SELECT count() AS n FROM shop.events"
            ),
            "TO shop.daily (`n` UInt64) AS SELECT count() AS n FROM shop.events"
        );
    }

    #[test]
    fn constraints_are_read_from_the_create_statement() {
        let create = "CREATE TABLE shop.orders (`id` UInt64, `total` Decimal(12, 2), \
                      CONSTRAINT positive CHECK total >= 0, CONSTRAINT `has id` ASSUME id > 0) \
                      ENGINE = MergeTree ORDER BY id SETTINGS index_granularity = 8192";
        let constraints = constraints(create);
        assert_eq!(constraints.len(), 2);
        assert_eq!(&*constraints[0].name(), "positive");
        assert_eq!(
            constraints[0].rule(),
            &ConstraintRule::Check {
                expression: "total >= 0".into()
            }
        );
        assert_eq!(constraints[0].definition(), Some("CHECK total >= 0"));
        assert_eq!(&*constraints[1].name(), "has id");
        assert_eq!(constraints[1].definition(), Some("ASSUME id > 0"));
        assert!(super::constraints("CREATE TABLE t (a UInt8) ENGINE = Log").is_empty());
    }

    #[test]
    fn a_function_lists_its_parameters() {
        let routine = routine(
            "linear",
            "CREATE FUNCTION linear AS (x, k, b) -> ((k * x) + b)",
        );
        assert_eq!(routine.arguments(), "x, k, b");
        assert_eq!(routine.language(), Some("SQL"));
        assert_eq!(
            super::routine("twice", "CREATE FUNCTION twice AS x -> (x * 2)").arguments(),
            "x"
        );
    }
}
