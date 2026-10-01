//! The driver against a real server.
//!
//! These run only when `DATAKIT_TEST_PG_URL` names a server, as
//! `postgres://user:password@host:port/database`; each test works in a schema
//! of its own and drops it afterwards. Without the variable they pass
//! without doing anything, so `cargo test` stays green on a machine with no
//! PostgreSQL.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use datakit_catalog::{ConstraintRule, ReferentialAction, RelationType};
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Dialect as _, Driver, StatementOutcome, Value,
};
use datakit_driver_postgres::PostgresDriver;
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

struct Server {
    profile: ConnectionProfile,
    password: String,
}

fn server() -> Option<Server> {
    let url = std::env::var("DATAKIT_TEST_PG_URL").ok()?;
    let rest = url
        .strip_prefix("postgres://")
        .or_else(|| url.strip_prefix("postgresql://"))?;
    let (credentials, address) = rest.split_once('@')?;
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (host_port, database) = address.split_once('/').unwrap_or((address, "postgres"));
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, "5432"));
    Some(Server {
        profile: ConnectionProfile::new(PostgresDriver::ID, port.parse().ok()?)
            .with_host(host)
            .with_user(user)
            .with_database(database)
            .with_ssl_mode(datakit_driver::SslMode::Disable),
        password: password.to_string(),
    })
}

/// Run `test` with a connection and a fresh schema named after the test.
fn with_schema<F, Fut>(name: &str, test: F)
where
    F: FnOnce(Arc<dyn Connection>, String) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(server) = server() else {
        eprintln!("DATAKIT_TEST_PG_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    let schema = format!("datakit_test_{name}");
    let connection = runtime.block_on(async {
        PostgresDriver
            .connect(&server.profile, Some(server.password.clone()))
            .await
            .expect("connect")
    });
    runtime.block_on(async {
        run(
            &connection,
            &format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        )
        .await;
        run(&connection, &format!("CREATE SCHEMA {schema}")).await;
    });
    runtime.block_on(test(connection.clone(), schema.clone()));
    runtime.block_on(run(&connection, &format!("DROP SCHEMA {schema} CASCADE")));
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
    with_schema("values", |connection, schema| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {schema}.items (id int PRIMARY KEY, price numeric(10,2), \
                 active boolean, note text, tags text[], data jsonb, ratio float8)"
            ),
        )
        .await;
        run(
            &connection,
            &format!(
                "INSERT INTO {schema}.items VALUES \
                 (1, 9.90, true, 'first', '{{a,b}}', '{{\"k\": 1}}', 0.5), \
                 (2, NULL, false, NULL, NULL, NULL, NULL)"
            ),
        )
        .await;

        let (names, rows) = rows(
            &connection,
            &format!("SELECT * FROM {schema}.items ORDER BY id"),
        )
        .await;
        assert_eq!(
            names,
            ["id", "price", "active", "note", "tags", "data", "ratio"]
        );
        assert_eq!(
            rows[0],
            vec![
                Value::Int(1),
                Value::Text("9.90".into()),
                Value::Bool(true),
                Value::Text("first".into()),
                Value::Text("{a,b}".into()),
                Value::Text("{\"k\": 1}".into()),
                Value::Float(0.5),
            ]
        );
        assert_eq!(
            rows[1],
            vec![
                Value::Int(2),
                Value::Null,
                Value::Bool(false),
                Value::Null,
                Value::Null,
                Value::Null,
                Value::Null,
            ]
        );
    });
}

#[test]
fn result_columns_carry_their_types() {
    with_schema("types", |connection, _| async move {
        let StatementOutcome::Rows(stream) = run(
            &connection,
            "SELECT 1::int8 AS n, 2.5::numeric AS d, now() AS t",
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
        assert_eq!(types, ["bigint", "numeric", "timestamptz"]);
        assert!(stream.columns()[1].category().is_numeric());
    });
}

#[test]
fn commands_report_what_they_did() {
    with_schema("commands", |connection, schema| async move {
        let StatementOutcome::Command(created) =
            run(&connection, &format!("CREATE TABLE {schema}.t (a int)")).await
        else {
            panic!("expected a command");
        };
        assert_eq!(created.tag(), "CREATE TABLE");
        assert_eq!(created.rows(), None);

        let StatementOutcome::Command(inserted) = run(
            &connection,
            &format!("INSERT INTO {schema}.t SELECT generate_series(1, 3)"),
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
fn errors_point_at_the_offending_text() {
    with_schema("errors", |connection, _| async move {
        let sql = "SELECT 1, nope FROM pg_class";
        let error = match connection.execute(sql.into()).await {
            Ok(_) => panic!("the statement must fail"),
            Err(error) => error,
        };
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.code(), Some("42703"));
        assert_eq!(&sql[error.position().unwrap()..][..4], "nope");
    });
}

#[test]
fn dropping_a_long_result_frees_the_session() {
    with_schema("abandon", |connection, _| async move {
        let StatementOutcome::Rows(mut stream) = run(
            &connection,
            "SELECT g, repeat('x', 100) FROM generate_series(1, 50000000) g",
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
            "the abandoned statement was cancelled, not drained ({:?})",
            started.elapsed()
        );
    });
}

#[test]
fn a_running_statement_can_be_cancelled() {
    with_schema("cancel", |connection, _| async move {
        let running = tokio::spawn({
            let connection = connection.clone();
            async move { connection.execute("SELECT pg_sleep(30)".into()).await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        let started = Instant::now();
        connection.cancel().await.expect("cancel");
        let outcome = match running.await.unwrap() {
            // The sleep returns a row description first, then fails while
            // the row is computed.
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
        assert_eq!(error.code(), Some("57014"));
        assert!(started.elapsed() < Duration::from_secs(10));
    });
}

#[test]
fn introspection_describes_schemas_relations_and_columns() {
    with_schema("introspect", |connection, schema| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {schema}.orders (id bigint PRIMARY KEY, total numeric(12,2) NOT NULL \
                 DEFAULT 0, note varchar(80)); \
                 "
            ),
        )
        .await;
        run(
            &connection,
            &format!("COMMENT ON TABLE {schema}.orders IS 'Every order'"),
        )
        .await;
        run(
            &connection,
            &format!(
                "CREATE VIEW {schema}.big_orders AS SELECT * FROM {schema}.orders WHERE total > 100"
            ),
        )
        .await;

        let schemas = connection.introspect_schemas().await.unwrap();
        let found = schemas.iter().find(|s| *s.name() == *schema).unwrap();
        assert!(!found.is_system());
        assert!(
            schemas
                .iter()
                .any(|s| &*s.name() == "pg_catalog" && s.is_system())
        );

        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let relations = loaded.relations().unwrap();
        let names: Vec<_> = relations.iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, ["big_orders", "orders"]);
        let orders = relations.iter().find(|r| &*r.name() == "orders").unwrap();
        assert_eq!(orders.relation_type(), RelationType::Table);
        assert_eq!(orders.comment(), Some("Every order"));
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
                ("id".into(), "bigint".into(), false, true),
                ("total".into(), "numeric(12,2)".into(), false, false),
                ("note".into(), "character varying(80)".into(), true, false),
            ]
        );
        assert_eq!(orders.column("total").unwrap().default(), Some("0"));
        let view = relations
            .iter()
            .find(|r| &*r.name() == "big_orders")
            .unwrap();
        assert_eq!(view.relation_type(), RelationType::View);
        assert_eq!(view.columns().len(), 3);

        let search_path = connection.search_path().await.unwrap();
        assert!(search_path.iter().any(|s| &**s == "public"));
    });
}

#[test]
fn introspection_reads_keys_indexes_triggers_routines_and_sequences() {
    with_schema("objects", |connection, schema| async move {
        for statement in [
            format!("CREATE TABLE {schema}.customers (id serial PRIMARY KEY, email text UNIQUE)"),
            format!(
                "CREATE TABLE {schema}.orders (id bigint GENERATED BY DEFAULT AS IDENTITY \
                 PRIMARY KEY, customer_id integer NOT NULL REFERENCES {schema}.customers (id) \
                 ON DELETE CASCADE, total numeric CHECK (total >= 0))"
            ),
            format!("CREATE INDEX orders_big ON {schema}.orders (total) WHERE total > 100"),
            format!(
                "CREATE FUNCTION {schema}.touch() RETURNS trigger LANGUAGE plpgsql \
                 AS $$ BEGIN RETURN NEW; END $$"
            ),
            format!(
                "CREATE TRIGGER orders_touch BEFORE INSERT OR UPDATE ON {schema}.orders \
                 FOR EACH ROW EXECUTE FUNCTION {schema}.touch()"
            ),
            format!("CREATE SEQUENCE {schema}.invoice_numbers START 1000 INCREMENT 10"),
        ] {
            run(&connection, &statement).await;
        }

        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let orders = loaded.relation("orders").unwrap();
        assert!(orders.column("id").unwrap().is_auto_increment());
        let (_, key) = orders.foreign_keys().next().expect("a foreign key");
        assert_eq!(key.referenced_relation(), "customers");
        assert_eq!(key.on_delete(), ReferentialAction::Cascade);
        assert!(orders.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Check { expression } if expression.contains("total >= ")
        )));
        let partial = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "orders_big")
            .unwrap();
        assert_eq!(partial.columns(), [std::sync::Arc::from("total")]);
        assert!(partial.predicate().unwrap().contains("100"));
        let trigger = &orders.triggers()[0];
        assert_eq!(trigger.timing(), "BEFORE INSERT OR UPDATE FOR EACH ROW");

        let routine = loaded.routines_named("touch").next().unwrap();
        assert_eq!(routine.result(), Some("trigger"));
        assert!(routine.definition().unwrap().contains("RETURN NEW"));

        let sequence = loaded.sequence("invoice_numbers").unwrap();
        assert_eq!((sequence.start(), sequence.increment()), (1000, 10));
        let owned = loaded.sequence("customers_id_seq").unwrap();
        assert_eq!(owned.owned_by(), Some("customers.id"));

        // The DDL the dialect writes recreates the table in another schema.
        let dialect = datakit_driver_postgres::PostgresDialect;
        let customers = loaded.relation("customers").unwrap();
        for statement in dialect.create_relation(&format!("{schema}_copy"), customers) {
            assert!(statement.starts_with("CREATE"), "{statement}");
        }
        run(&connection, &format!("CREATE SCHEMA {schema}_copy")).await;
        for statement in dialect.create_relation(&format!("{schema}_copy"), customers) {
            run(&connection, &statement).await;
        }
        run(&connection, &format!("DROP SCHEMA {schema}_copy CASCADE")).await;
    });
}

#[test]
fn roles_show_who_can_log_in_and_their_memberships() {
    with_schema("roles", |connection, _| async move {
        for sql in [
            "DROP ROLE IF EXISTS datakit_test_member",
            "DROP ROLE IF EXISTS datakit_test_group",
            "CREATE ROLE datakit_test_group NOLOGIN CREATEDB",
            "CREATE ROLE datakit_test_member LOGIN CONNECTION LIMIT 3 IN ROLE datakit_test_group",
        ] {
            run(&connection, sql).await;
        }
        let roles = connection.introspect_roles().await.expect("roles");
        for sql in [
            "DROP ROLE datakit_test_member",
            "DROP ROLE datakit_test_group",
        ] {
            run(&connection, sql).await;
        }

        assert!(roles.iter().all(|role| !role.name().starts_with("pg_")));
        let group = roles
            .iter()
            .find(|role| &*role.name() == "datakit_test_group")
            .expect("the group");
        assert!(!group.can_login());
        assert_eq!(group.attributes(), [Arc::from("CREATEDB")]);
        let member = roles
            .iter()
            .find(|role| &*role.name() == "datakit_test_member")
            .expect("the member");
        assert!(member.can_login());
        assert_eq!(member.member_of(), [Arc::from("datakit_test_group")]);
        assert_eq!(
            member.definition(),
            Some(
                "CREATE ROLE datakit_test_member WITH LOGIN CONNECTION LIMIT 3;\n\
                 GRANT datakit_test_group TO datakit_test_member;"
            )
        );
    });
}
