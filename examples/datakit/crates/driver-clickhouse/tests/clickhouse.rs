//! The driver against a real server.
//!
//! These run only when `DATAKIT_TEST_CLICKHOUSE_URL` names a server, as
//! `http://user:password@host:port/database` (`https://` for TLS without
//! verification); each test works in a database of its own and drops it
//! afterwards. Without the variable they pass without doing anything, so
//! `cargo test` stays green on a machine with no ClickHouse.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use datakit_catalog::{ConstraintRule, RelationType};
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Dialect as _, Driver, RowChange, SslMode,
    StatementOutcome, TypeCategory, Value,
};
use datakit_driver_clickhouse::{ClickHouseDialect, ClickHouseDriver};
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

struct Server {
    profile: ConnectionProfile,
    password: String,
}

fn server() -> Option<Server> {
    let url = std::env::var("DATAKIT_TEST_CLICKHOUSE_URL").ok()?;
    let (ssl_mode, rest) = match url.strip_prefix("https://") {
        Some(rest) => (SslMode::Require, rest),
        None => (SslMode::Disable, url.strip_prefix("http://")?),
    };
    let (credentials, address) = rest.split_once('@').unwrap_or(("default", rest));
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (host_port, database) = address.split_once('/').unwrap_or((address, ""));
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, "8123"));
    Some(Server {
        profile: ConnectionProfile::new(ClickHouseDriver::ID, port.parse().ok()?)
            .with_host(host)
            .with_user(user)
            .with_database(database)
            .with_ssl_mode(ssl_mode),
        password: password.to_string(),
    })
}

/// Run `test` with a connection and a fresh database named after the test.
fn with_database<F, Fut>(name: &str, test: F)
where
    F: FnOnce(Arc<dyn Connection>, String) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(server) = server() else {
        eprintln!("DATAKIT_TEST_CLICKHOUSE_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    let database = format!("datakit_test_{name}");
    let connection = runtime.block_on(async {
        ClickHouseDriver
            .connect(&server.profile, Some(server.password.clone()))
            .await
            .expect("connect")
    });
    runtime.block_on(async {
        run(&connection, &format!("DROP DATABASE IF EXISTS {database}")).await;
        run(&connection, &format!("CREATE DATABASE {database}")).await;
    });
    runtime.block_on(test(connection.clone(), database.clone()));
    runtime.block_on(run(&connection, &format!("DROP DATABASE {database}")));
}

async fn run(connection: &Arc<dyn Connection>, sql: &str) -> StatementOutcome {
    connection.execute(sql.into()).await.expect(sql)
}

async fn rows(connection: &Arc<dyn Connection>, sql: &str) -> (Vec<String>, Vec<Vec<Value>>) {
    match run(connection, sql).await {
        StatementOutcome::Rows(stream) => {
            let names = stream
                .columns()
                .iter()
                .map(|c| c.name().to_string())
                .collect();
            let rows = stream
                .map(|row| row.expect("row").into_vec())
                .collect()
                .await;
            (names, rows)
        }
        StatementOutcome::Command(summary) => panic!("expected rows, got {}", summary.tag()),
    }
}

#[test]
fn values_keep_the_servers_text_and_numbers_parse() {
    with_database("values", |connection, database| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {database}.items (id UInt64, price Decimal(10, 2), active Bool, \
                 note Nullable(String), tags Array(String), ratio Nullable(Float64), \
                 at DateTime('UTC'), big UInt64) ENGINE = MergeTree ORDER BY id"
            ),
        )
        .await;
        run(
            &connection,
            &format!(
                "INSERT INTO {database}.items VALUES \
                 (1, 9.95, true, 'tab\\there', ['a', 'b'], 0.5, '2024-01-02 03:04:05', 18446744073709551615), \
                 (2, 0, false, NULL, [], NULL, '2024-01-02 03:04:05', 7)"
            ),
        )
        .await;

        let (names, rows) = rows(
            &connection,
            &format!("SELECT * FROM {database}.items ORDER BY id"),
        )
        .await;
        assert_eq!(
            names,
            [
                "id", "price", "active", "note", "tags", "ratio", "at", "big"
            ]
        );
        assert_eq!(
            rows[0],
            vec![
                Value::Int(1),
                Value::Text("9.95".into()),
                Value::Bool(true),
                Value::Text("tab\there".into()),
                Value::Text("['a','b']".into()),
                Value::Float(0.5),
                Value::Text("2024-01-02 03:04:05".into()),
                // Past i64, an integer keeps its digits.
                Value::Text("18446744073709551615".into()),
            ]
        );
        assert_eq!(rows[1][3], Value::Null);
        assert_eq!(rows[1][5], Value::Null);
        assert_eq!(rows[1][7], Value::Int(7));
    });
}

#[test]
fn result_columns_carry_their_types() {
    with_database("types", |connection, _| async move {
        let StatementOutcome::Rows(stream) = run(
            &connection,
            "SELECT toUInt64(1) AS n, toDecimal64(2.5, 1) AS d, toDate('2024-01-02') AS t, \
             CAST(NULL AS Nullable(String)) AS s",
        )
        .await
        else {
            panic!("expected rows");
        };
        let types: Vec<String> = stream
            .columns()
            .iter()
            .map(|column| column.type_name().to_string())
            .collect();
        assert_eq!(
            types,
            ["UInt64", "Decimal(18, 1)", "Date", "Nullable(String)"]
        );
        let categories: Vec<TypeCategory> = stream
            .columns()
            .iter()
            .map(|column| column.category())
            .collect();
        assert_eq!(
            categories,
            [
                TypeCategory::Integer,
                TypeCategory::Decimal,
                TypeCategory::Temporal,
                TypeCategory::Text
            ]
        );
    });
}

#[test]
fn commands_report_what_they_did() {
    with_database("commands", |connection, database| async move {
        let StatementOutcome::Command(created) = run(
            &connection,
            &format!("CREATE TABLE {database}.t (a UInt64) ENGINE = MergeTree ORDER BY a"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!(created.tag(), "CREATE TABLE");
        assert_eq!(created.rows(), None);

        let StatementOutcome::Command(inserted) = run(
            &connection,
            &format!("INSERT INTO {database}.t SELECT number FROM numbers(3)"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!(inserted.tag(), "INSERT");
        assert_eq!(inserted.rows(), Some(3));
    });
}

#[test]
fn session_settings_last_between_statements() {
    with_database("session", |connection, database| async move {
        run(&connection, &format!("USE {database}")).await;
        let (_, rows) = rows(&connection, "SELECT currentDatabase()").await;
        assert_eq!(rows, vec![vec![Value::Text(database.as_str().into())]]);
        let search_path = connection.search_path().await.unwrap();
        assert_eq!(search_path, [Arc::from(database.as_str())]);
    });
}

#[test]
fn errors_point_at_the_offending_text() {
    with_database("errors", |connection, _| async move {
        for (sql, code, at) in [
            ("SELEC 1", "62", "SELEC"),
            ("SELECT 1, nope FROM system.one", "47", "nope"),
        ] {
            let error = match connection.execute(sql.into()).await {
                Ok(_) => panic!("{sql} must fail"),
                Err(error) => error,
            };
            let error = error
                .downcast_ref::<DatabaseError>()
                .expect("a database error");
            assert_eq!(error.code(), Some(code), "{sql}");
            assert_eq!(&sql[error.position().unwrap()..][..at.len()], at, "{sql}");
        }
    });
}

#[test]
fn an_error_after_the_first_rows_ends_the_stream() {
    with_database("late_error", |connection, _| async move {
        // Megabytes of rows come before the error, more than the server
        // buffers, so it has started the response by then.
        let outcome = connection
            .execute(
                "SELECT number, repeat('x', 100), throwIf(number = 100000, 'late')                  FROM numbers(200000) SETTINGS max_block_size = 1000"
                    .into(),
            )
            .await;
        let StatementOutcome::Rows(stream) = outcome.expect("rows arrive before the error") else {
            panic!("expected rows");
        };
        let items: Vec<_> = stream.collect().await;
        assert!(items[0].is_ok());
        let error = items
            .into_iter()
            .find_map(Result::err)
            .expect("the stream ends with the error");
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.code(), Some("395"));
    });
}

#[test]
fn dropping_a_long_result_frees_the_session() {
    with_database("abandon", |connection, _| async move {
        let StatementOutcome::Rows(mut stream) = run(
            &connection,
            "SELECT number, repeat('x', 100) FROM system.numbers",
        )
        .await
        else {
            panic!("expected rows");
        };
        assert!(stream.next().await.is_some());
        drop(stream);

        let started = Instant::now();
        let (_, rows) = rows(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the abandoned statement was stopped, not drained ({:?})",
            started.elapsed()
        );
    });
}

#[test]
fn a_running_statement_can_be_cancelled() {
    with_database("cancel", |connection, _| async move {
        let running = tokio::spawn({
            let connection = connection.clone();
            async move {
                connection
                    .execute("SELECT sum(number) FROM numbers(1000000000000)".into())
                    .await
            }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        let started = Instant::now();
        connection.cancel().await.expect("cancel");
        let outcome = match running.await.unwrap() {
            Ok(StatementOutcome::Rows(stream)) => stream
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .find_map(Result::err),
            Ok(StatementOutcome::Command(_)) => None,
            Err(error) => Some(error),
        };
        let error = outcome.expect("the statement was cancelled");
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.code(), Some("394"));
        assert!(started.elapsed() < Duration::from_secs(10));
    });
}

#[test]
fn introspection_describes_databases_relations_and_columns() {
    with_database("introspect", |connection, database| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {database}.orders (\
                 id UInt64, \
                 total Decimal(12, 2) DEFAULT 0 COMMENT 'Sum', \
                 note Nullable(String), \
                 day Date MATERIALIZED today(), \
                 INDEX note_bloom note TYPE bloom_filter GRANULARITY 2, \
                 CONSTRAINT positive CHECK total >= 0) \
                 ENGINE = MergeTree ORDER BY id COMMENT 'Every order'"
            ),
        )
        .await;
        run(
            &connection,
            &format!(
                "CREATE VIEW {database}.big_orders AS SELECT * FROM {database}.orders WHERE total > 100"
            ),
        )
        .await;

        let schemas = connection.introspect_schemas().await.unwrap();
        let found = schemas.iter().find(|s| *s.name() == *database).unwrap();
        assert!(!found.is_system());
        assert!(
            schemas
                .iter()
                .any(|s| &*s.name() == "system" && s.is_system())
        );

        let loaded = connection
            .introspect_schema(database.as_str().into())
            .await
            .unwrap();
        let relations = loaded.relations().unwrap();
        let names: Vec<_> = relations.iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, ["big_orders", "orders"]);
        let orders = loaded.relation("orders").unwrap();
        assert_eq!(orders.relation_type(), RelationType::Table);
        assert_eq!(orders.comment(), Some("Every order"));
        assert!(orders.definition().unwrap().starts_with("MergeTree"));
        let columns: Vec<_> = orders
            .columns()
            .iter()
            .map(|c| {
                (
                    c.name().to_string(),
                    c.data_type().to_string(),
                    c.is_nullable(),
                    c.is_primary_key(),
                )
            })
            .collect();
        assert_eq!(
            columns,
            [
                ("id".into(), "UInt64".into(), false, true),
                ("total".into(), "Decimal(12, 2)".into(), false, false),
                ("note".into(), "Nullable(String)".into(), true, false),
                ("day".into(), "Date".into(), false, false),
            ]
        );
        assert_eq!(orders.column("total").unwrap().default(), Some("0"));
        assert_eq!(orders.column("total").unwrap().comment(), Some("Sum"));
        assert!(orders.column("day").unwrap().is_generated());
        let bloom = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "note_bloom")
            .unwrap();
        assert_eq!(bloom.method(), Some("bloom_filter"));
        assert!(orders.indexes().iter().any(|index| index.is_primary()));
        assert!(orders.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Check { expression } if expression.contains("total >= 0")
        )));

        let view = loaded.relation("big_orders").unwrap();
        assert_eq!(view.relation_type(), RelationType::View);
        assert!(view.definition().unwrap().starts_with("SELECT"));
        assert_eq!(view.columns().len(), 3);
    });
}

#[test]
fn the_dialect_recreates_tables_and_edits_rows() {
    with_database("ddl", |connection, database| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {database}.events (\
                 id UInt64, \
                 kind LowCardinality(String), \
                 note Nullable(String) COMMENT 'Free text', \
                 INDEX by_kind kind TYPE set(100) GRANULARITY 4) \
                 ENGINE = ReplacingMergeTree ORDER BY (id, kind) COMMENT 'All events'"
            ),
        )
        .await;
        run(
            &connection,
            &format!("CREATE VIEW {database}.notes AS SELECT note FROM {database}.events"),
        )
        .await;
        let loaded = connection
            .introspect_schema(database.as_str().into())
            .await
            .unwrap();

        // The DDL the dialect writes recreates both in another database.
        let copy = format!("{database}_copy");
        run(&connection, &format!("DROP DATABASE IF EXISTS {copy}")).await;
        run(&connection, &format!("CREATE DATABASE {copy}")).await;
        let dialect = ClickHouseDialect;
        for name in ["events", "notes"] {
            let relation = loaded.relation(name).unwrap();
            for statement in dialect.create_relation(&copy, relation) {
                run(&connection, &statement).await;
            }
        }
        let copied = connection
            .introspect_schema(copy.as_str().into())
            .await
            .unwrap();
        let events = loaded.relation("events").unwrap();
        let events_copy = copied.relation("events").unwrap();
        assert_eq!(events_copy.columns(), events.columns());
        assert_eq!(events_copy.indexes(), events.indexes());
        assert_eq!(events_copy.comment(), Some("All events"));
        assert_eq!(events_copy.definition(), events.definition());

        // Row changes, as the data editor makes them.
        let table = &copy;
        for change in [
            RowChange::Insert {
                values: vec![
                    ("id".into(), Value::Int(1)),
                    ("kind".into(), Value::Text("click".into())),
                ],
            },
            RowChange::Insert { values: vec![] },
            RowChange::Update {
                key: vec![
                    ("id".into(), Value::Int(1)),
                    ("kind".into(), Value::Text("click".into())),
                ],
                values: vec![("note".into(), Value::Text("it's \\ fine".into()))],
            },
            RowChange::Delete {
                key: vec![("id".into(), Value::Int(0))],
            },
        ] {
            run(&connection, &dialect.row_change(table, "events", &change)).await;
        }
        let (_, rows) = rows(
            &connection,
            &format!("SELECT id, kind, note FROM {copy}.events ORDER BY id"),
        )
        .await;
        assert_eq!(
            rows,
            vec![vec![
                Value::Int(1),
                Value::Text("click".into()),
                Value::Text("it's \\ fine".into()),
            ]]
        );

        // Comments and column changes.
        let note = events_copy.column("note").unwrap();
        for statement in dialect.alter_column(
            &copy,
            "events",
            note,
            &note.clone().with_comment("Changed").with_default("'none'"),
        ) {
            run(&connection, &statement).await;
        }
        let changed = connection
            .introspect_schema(copy.as_str().into())
            .await
            .unwrap();
        let note = changed.relation("events").unwrap().column("note").unwrap();
        assert_eq!(note.comment(), Some("Changed"));
        assert_eq!(note.default(), Some("'none'"));

        run(&connection, &format!("DROP DATABASE {copy}")).await;
    });
}

#[test]
fn explaining_returns_a_plan_tree() {
    with_database("explain", |connection, database| async move {
        run(
            &connection,
            &format!("CREATE TABLE {database}.t (a UInt64) ENGINE = MergeTree ORDER BY a"),
        )
        .await;
        let dialect = ClickHouseDialect;
        let sql = dialect
            .explain(&format!("SELECT a FROM {database}.t WHERE a > 10"), false)
            .unwrap();
        let (_, rows) = rows(&connection, &sql).await;
        let plan = dialect.parse_plan(&rows).unwrap();
        assert!(
            plan.flatten()
                .iter()
                .any(|(_, node)| node.operation() == "ReadFromMergeTree"),
            "{plan:?}"
        );
    });
}
