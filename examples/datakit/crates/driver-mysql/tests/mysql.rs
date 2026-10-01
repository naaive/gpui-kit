//! The driver against a real server.
//!
//! These run only when `DATAKIT_TEST_MYSQL_URL` names a server, as
//! `mysql://user:password@host:port/database`; each test works in a
//! database of its own and drops it afterwards. Without the variable they
//! pass without doing anything, so `cargo test` stays green on a machine
//! with no MySQL.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use datakit_catalog::{ConstraintRule, ReferentialAction, RelationType, RoutineType};
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Dialect as _, Driver, StatementOutcome,
    TypeCategory, Value,
};
use datakit_driver_mysql::{MySqlDialect, MySqlDriver};
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

struct Server {
    profile: ConnectionProfile,
    password: String,
}

fn server() -> Option<Server> {
    let url = std::env::var("DATAKIT_TEST_MYSQL_URL").ok()?;
    let rest = url.strip_prefix("mysql://")?;
    let (credentials, address) = rest.split_once('@')?;
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (host_port, database) = address.split_once('/').unwrap_or((address, ""));
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, "3306"));
    Some(Server {
        profile: ConnectionProfile::new(MySqlDriver::ID, port.parse().ok()?)
            .with_host(host)
            .with_user(user)
            .with_database(database)
            .with_ssl_mode(datakit_driver::SslMode::Prefer),
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
        eprintln!("DATAKIT_TEST_MYSQL_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    let database = format!("datakit_test_{name}");
    let connection = runtime.block_on(async {
        MySqlDriver
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

/// A table of the numbers 1 to 1000, to cross join into a long result.
async fn numbers(connection: &Arc<dyn Connection>, database: &str) {
    run(connection, &format!("CREATE TABLE {database}.n (i int)")).await;
    run(
        connection,
        &format!(
            "INSERT INTO {database}.n WITH RECURSIVE s (i) AS \
             (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < 1000) SELECT i FROM s"
        ),
    )
    .await;
}

#[test]
fn values_keep_the_servers_text_and_numbers_parse() {
    with_database("values", |connection, database| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {database}.items (id int PRIMARY KEY, price decimal(10,2), \
                 active tinyint(1), note varchar(20), data json, ratio double, \
                 code varbinary(4), placed datetime(3), size enum('s','m','l'), \
                 big bigint unsigned)"
            ),
        )
        .await;
        run(
            &connection,
            &format!(
                "INSERT INTO {database}.items VALUES \
                 (1, 9.90, 1, 'first', '{{\"k\": 1}}', 0.5, x'00ff10', \
                  '2024-02-29 13:45:01.250', 'm', 18446744073709551615), \
                 (2, NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL)"
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
                "id", "price", "active", "note", "data", "ratio", "code", "placed", "size", "big"
            ]
        );
        assert_eq!(
            rows[0],
            vec![
                Value::Int(1),
                Value::Text("9.90".into()),
                Value::Int(1),
                Value::Text("first".into()),
                Value::Text("{\"k\": 1}".into()),
                Value::Float(0.5),
                Value::Text("0x00FF10".into()),
                Value::Text("2024-02-29 13:45:01.250".into()),
                Value::Text("m".into()),
                // Too large for an i64: the server's text is kept.
                Value::Text("18446744073709551615".into()),
            ]
        );
        assert_eq!(rows[1][..3], [Value::Int(2), Value::Null, Value::Int(0)]);
        assert!(rows[1][3..].iter().all(Value::is_null));
    });
}

#[test]
fn result_columns_carry_their_types() {
    with_database("types", |connection, _| async move {
        let StatementOutcome::Rows(stream) = run(
            &connection,
            "SELECT CAST(1 AS SIGNED) AS n, 2.5 AS d, NOW() AS t, x'00ff' AS b, 'x' AS s, \
             CAST('{}' AS JSON) AS j, 1.5e0 AS f",
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
            [
                "bigint",
                "decimal",
                "datetime",
                "varbinary",
                "varchar",
                "json",
                "double"
            ]
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
                TypeCategory::Binary,
                TypeCategory::Text,
                TypeCategory::Json,
                TypeCategory::Float,
            ]
        );
    });
}

#[test]
fn commands_report_what_they_did() {
    with_database("commands", |connection, database| async move {
        let StatementOutcome::Command(created) =
            run(&connection, &format!("CREATE TABLE {database}.t (a int)")).await
        else {
            panic!("expected a command");
        };
        assert_eq!(created.tag(), "CREATE TABLE");
        assert_eq!(created.rows(), None);

        let StatementOutcome::Command(inserted) = run(
            &connection,
            &format!("INSERT INTO {database}.t VALUES (1), (2), (3)"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!(inserted.tag(), "INSERT");
        assert_eq!(inserted.rows(), Some(3));

        // Rows that match count, even when the update leaves them as they
        // were.
        let StatementOutcome::Command(updated) = run(
            &connection,
            &format!("UPDATE {database}.t SET a = a WHERE a < 3"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!(updated.tag(), "UPDATE");
        assert_eq!(updated.rows(), Some(2));

        // Several statements report the first; only a comment reports
        // nothing.
        let (_, first) = rows(&connection, "SELECT 1; SELECT 2").await;
        assert_eq!(first, vec![vec![Value::Int(1)]]);
        let StatementOutcome::Command(empty) = run(&connection, "-- nothing").await else {
            panic!("expected a command");
        };
        assert_eq!(empty.tag(), "");
    });
}

#[test]
fn errors_carry_the_sqlstate_and_the_mysql_number() {
    with_database("errors", |connection, _| async move {
        let error = match connection.execute("SELECT 1, nope".into()).await {
            Ok(_) => panic!("the statement must fail"),
            Err(error) => error,
        };
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.code(), Some("42S22"));
        assert_eq!(error.detail(), Some("Error 1054"));
        assert!(error.message().contains("nope"), "{}", error.message());
        assert_eq!(error.position(), None);

        // The session is usable afterwards.
        let (_, rows) = rows(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
    });
}

#[test]
fn dropping_a_long_result_frees_the_session() {
    with_database("abandon", |connection, database| async move {
        numbers(&connection, &database).await;
        let StatementOutcome::Rows(mut stream) = run(
            &connection,
            &format!("SELECT a.i, b.i, c.i, repeat('x', 100) FROM {database}.n a, {database}.n b, {database}.n c"),
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
            "the abandoned statement was killed, not drained ({:?})",
            started.elapsed()
        );
        assert!(!connection.is_closed());
    });
}

#[test]
fn a_running_statement_can_be_cancelled() {
    with_database("cancel", |connection, database| async move {
        numbers(&connection, &database).await;
        let running = tokio::spawn({
            let connection = connection.clone();
            let sql = format!(
                "SELECT sum(a.i + b.i + c.i) FROM {database}.n a, {database}.n b, {database}.n c"
            );
            async move { connection.execute(sql.into()).await }
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
        assert_eq!(error.code(), Some("70100"));
        assert!(started.elapsed() < Duration::from_secs(10));

        let (_, rows) = rows(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
    });
}

#[test]
fn introspection_describes_schemas_relations_and_columns() {
    with_database("introspect", |connection, database| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {database}.orders (id bigint PRIMARY KEY AUTO_INCREMENT, \
                 total decimal(12,2) NOT NULL DEFAULT 0, \
                 note varchar(80) DEFAULT 'it''s' COMMENT 'Said by the customer', \
                 placed_at datetime DEFAULT CURRENT_TIMESTAMP) COMMENT 'Every order'"
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
                .any(|s| &*s.name() == "mysql" && s.is_system())
        );
        assert!(
            !schemas.last().unwrap().name().starts_with("datakit"),
            "system databases come last"
        );

        let loaded = connection
            .introspect_schema(database.as_str().into())
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
                ("total".into(), "decimal(12,2)".into(), false, false),
                ("note".into(), "varchar(80)".into(), true, false),
                ("placed_at".into(), "datetime".into(), true, false),
            ]
        );
        assert!(orders.column("id").unwrap().is_auto_increment());
        assert_eq!(orders.column("total").unwrap().default(), Some("0.00"));
        let note = orders.column("note").unwrap();
        assert_eq!(note.default(), Some("'it''s'"));
        assert_eq!(note.comment(), Some("Said by the customer"));
        assert_eq!(
            orders.column("placed_at").unwrap().default(),
            Some("CURRENT_TIMESTAMP")
        );

        let view = relations
            .iter()
            .find(|r| &*r.name() == "big_orders")
            .unwrap();
        assert_eq!(view.relation_type(), RelationType::View);
        assert_eq!(view.columns().len(), 4);
        assert!(view.definition().unwrap().contains("100"));
        assert_eq!(view.comment(), None);

        let search_path = connection.search_path().await.unwrap();
        let database = server().unwrap().profile.database().to_string();
        if database.is_empty() {
            assert!(search_path.is_empty());
        } else {
            assert_eq!(search_path, [Arc::from(database.as_str())]);
        }
    });
}

#[test]
fn introspection_reads_keys_indexes_triggers_and_routines() {
    with_database("objects", |connection, database| async move {
        for statement in [
            format!(
                "CREATE TABLE {database}.customers (id int AUTO_INCREMENT PRIMARY KEY, \
                 email varchar(100) NOT NULL, UNIQUE KEY customers_email (email))"
            ),
            format!(
                "CREATE TABLE {database}.orders (id bigint unsigned NOT NULL AUTO_INCREMENT, \
                 customer_id int NOT NULL, total decimal(10,2) NOT NULL DEFAULT '0.00', \
                 note varchar(50) DEFAULT NULL COMMENT 'Free text', \
                 doubled decimal(12,2) GENERATED ALWAYS AS (total * 2) VIRTUAL, \
                 PRIMARY KEY (id), \
                 CONSTRAINT orders_customer_fk FOREIGN KEY (customer_id) \
                 REFERENCES {database}.customers (id) ON DELETE CASCADE, \
                 CONSTRAINT orders_total_positive CHECK (total >= 0))"
            ),
            format!("CREATE INDEX orders_note ON {database}.orders (note(10), total DESC)"),
            format!(
                "CREATE TRIGGER {database}.orders_touch BEFORE INSERT ON {database}.orders \
                 FOR EACH ROW SET NEW.note = coalesce(NEW.note, 'new')"
            ),
            format!(
                "CREATE FUNCTION {database}.with_tax(amount decimal(10,2)) RETURNS decimal(10,2) \
                 DETERMINISTIC RETURN amount * 1.2"
            ),
            format!(
                "CREATE PROCEDURE {database}.order_count(IN customer int, OUT total int) \
                 READS SQL DATA SELECT count(*) INTO total FROM {database}.orders \
                 WHERE customer_id = customer"
            ),
        ] {
            run(&connection, &statement).await;
        }

        let loaded = connection
            .introspect_schema(database.as_str().into())
            .await
            .unwrap();
        let orders = loaded.relation("orders").unwrap();
        assert!(orders.column("id").unwrap().is_auto_increment());
        let doubled = orders.column("doubled").unwrap();
        assert!(doubled.is_generated());
        assert_eq!(doubled.default(), Some("(`total` * 2)"));
        let (_, key) = orders.foreign_keys().next().expect("a foreign key");
        assert_eq!(key.referenced_relation(), "customers");
        assert_eq!(key.referenced_schema(), database);
        assert_eq!(key.on_delete(), ReferentialAction::Cascade);
        assert!(orders.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Check { expression } if expression.contains("`total` >= 0")
        )));
        let primary = orders
            .constraints()
            .iter()
            .find(|c| matches!(c.rule(), ConstraintRule::PrimaryKey { .. }))
            .unwrap();
        assert_eq!(primary.rule().columns(), [Arc::from("id")]);
        let note_index = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "orders_note")
            .unwrap();
        assert_eq!(
            note_index.columns(),
            [Arc::from("`note`(10)"), Arc::from("`total` DESC")]
        );
        let trigger = &orders.triggers()[0];
        assert_eq!(trigger.timing(), "BEFORE INSERT FOR EACH ROW");
        assert!(trigger.definition().unwrap().contains("coalesce"));

        let function = loaded.routines_named("with_tax").next().unwrap();
        assert_eq!(function.routine_type(), RoutineType::Function);
        assert_eq!(function.arguments(), "amount decimal(10,2)");
        assert_eq!(function.result(), Some("decimal(10,2)"));
        let procedure = loaded.routines_named("order_count").next().unwrap();
        assert_eq!(procedure.arguments(), "IN customer int, OUT total int");
        assert!(procedure.definition().unwrap().contains("READS SQL DATA"));

        // The definitions the driver assembles are ones the server accepts:
        // drop each object and create it again from them.
        let dialect = MySqlDialect;
        for routine in loaded.routines() {
            run(&connection, &dialect.drop_routine(&database, routine)).await;
            run(
                &connection,
                &dialect.create_routine(&database, routine).unwrap(),
            )
            .await;
        }
        run(
            &connection,
            &dialect.drop_trigger(&database, "orders", trigger),
        )
        .await;
        run(
            &connection,
            &dialect
                .create_trigger(&database, "orders", trigger)
                .unwrap(),
        )
        .await;

        // The DDL the dialect writes recreates the tables in another
        // database, where they read back the same.
        let copy = format!("{database}_copy");
        run(&connection, &format!("DROP DATABASE IF EXISTS {copy}")).await;
        run(&connection, &format!("CREATE DATABASE {copy}")).await;
        for name in ["customers", "orders"] {
            // A trigger's definition names the database it was read from.
            let relation = loaded.relation(name).unwrap().clone().with_triggers([]);
            for statement in dialect.create_relation(&copy, &relation) {
                run(&connection, &statement).await;
            }
        }
        let copied = connection
            .introspect_schema(copy.as_str().into())
            .await
            .unwrap();
        for name in ["customers", "orders"] {
            let original = loaded.relation(name).unwrap();
            let recreated = copied.relation(name).unwrap();
            assert_eq!(recreated.columns(), original.columns(), "{name}");
            assert_eq!(recreated.indexes(), original.indexes(), "{name}");
            assert_eq!(recreated.constraints(), original.constraints(), "{name}");
            assert_eq!(recreated.comment(), original.comment(), "{name}");
        }

        // A changed column is restated in full.
        let copied_orders = copied.relation("orders").unwrap();
        let old = copied_orders.column("note").unwrap();
        let new = old
            .clone()
            .with_data_type("varchar(100)")
            .nullable(false)
            .with_default("'none'")
            .with_comment("Said by the customer");
        for statement in dialect.alter_column(&copy, "orders", old, &new) {
            run(&connection, &statement).await;
        }
        let altered = connection
            .introspect_schema(copy.as_str().into())
            .await
            .unwrap();
        assert_eq!(
            altered.relation("orders").unwrap().column("note"),
            Some(&new)
        );
        run(&connection, &format!("DROP DATABASE {copy}")).await;
    });
}

#[test]
fn sessions_connect_with_and_without_tls() {
    let Some(server) = server() else {
        eprintln!("DATAKIT_TEST_MYSQL_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    runtime.block_on(async {
        for (ssl_mode, encrypted) in [
            (datakit_driver::SslMode::Disable, false),
            (datakit_driver::SslMode::Require, true),
        ] {
            let profile = server.profile.clone().with_ssl_mode(ssl_mode);
            let connection = MySqlDriver
                .connect(&profile, Some(server.password.clone()))
                .await
                .unwrap_or_else(|error| panic!("{ssl_mode:?}: {error:#}"));
            let (_, rows) = rows(&connection, "SHOW SESSION STATUS LIKE 'Ssl_cipher'").await;
            let cipher = rows[0][1].display().unwrap_or_default().to_string();
            assert_eq!(!cipher.is_empty(), encrypted, "{ssl_mode:?}: {cipher}");
            assert!(connection.server_version().starts_with("M"));
        }
    });
}

#[test]
fn plans_parse_from_the_server() {
    with_database("plans", |connection, database| async move {
        numbers(&connection, &database).await;
        let dialect = MySqlDialect;
        let sql = format!(
            "SELECT a.i, count(*) FROM {database}.n a JOIN {database}.n b ON b.i = a.i \
             WHERE a.i < 10 GROUP BY a.i ORDER BY 2 DESC"
        );
        for analyze in [false, true] {
            let explain = dialect.explain(&sql, analyze).unwrap();
            let (_, rows) = rows(&connection, &explain).await;
            let plan = dialect
                .parse_plan(&rows)
                .unwrap_or_else(|error| panic!("analyze {analyze}: {error:#}"));
            let nodes = plan.flatten();
            assert!(nodes.len() > 2, "analyze {analyze}: {plan:?}");
            assert!(
                nodes
                    .iter()
                    .any(|(_, node)| node.target().is_some_and(|target| target.contains('b'))),
                "analyze {analyze}: {plan:?}"
            );
            if analyze {
                assert!(plan.actual_time().is_some(), "{plan:?}");
            } else {
                assert!(plan.total_cost().is_some(), "{plan:?}");
            }
        }
    });
}
