//! The driver against real SQLite databases: files in a temporary directory
//! and in-memory databases, so these run everywhere with nothing to set up.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use datakit_catalog::{ConstraintRule, ReferentialAction, RelationType, Schema, diff_schemas};
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Dialect as _, Driver, RowChange,
    StatementOutcome, TypeCategory, Value,
};
use datakit_driver_sqlite::{SqliteDialect, SqliteDriver};
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

/// A statement that runs for minutes before it returns its one row.
const COUNT_TO_A_BILLION: &str = "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c \
                                  WHERE x < 1000000000) SELECT count(*) FROM c";

fn profile(file: &str) -> ConnectionProfile {
    ConnectionProfile::new(SqliteDriver::ID, 0).with_option(ConnectionProfile::FILE, file)
}

/// Run `test` on the I/O runtime with a connection to `file`.
fn with_database<F, Fut>(file: &str, test: F)
where
    F: FnOnce(Arc<dyn Connection>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let runtime = IoRuntime::new().unwrap();
    let connection = runtime.block_on(async {
        SqliteDriver
            .connect(&profile(file), None)
            .await
            .expect("connect")
    });
    runtime.block_on(test(connection));
}

async fn run(connection: &Arc<dyn Connection>, sql: &str) -> StatementOutcome {
    connection.execute(sql.into()).await.expect(sql)
}

async fn select(connection: &Arc<dyn Connection>, sql: &str) -> (Vec<String>, Vec<Vec<Value>>) {
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

async fn error(connection: &Arc<dyn Connection>, sql: &str) -> DatabaseError {
    let error = match connection.execute(sql.into()).await {
        Ok(StatementOutcome::Rows(stream)) => stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .find_map(Result::err)
            .expect("the statement must fail"),
        Ok(StatementOutcome::Command(_)) => panic!("the statement must fail: {sql}"),
        Err(error) => error,
    };
    error
        .downcast_ref::<DatabaseError>()
        .expect("a database error")
        .clone()
}

#[test]
fn the_driver_describes_itself() {
    let driver = SqliteDriver::new();
    assert_eq!(driver.id(), "sqlite");
    assert_eq!(driver.name(), "SQLite");
    assert!(driver.is_file_based());
    assert_eq!(driver.default_port(), 0);
    assert!(!driver.dialect().has_schemas());
}

#[test]
fn a_profile_without_a_file_or_with_a_file_that_is_not_a_database_fails() {
    let runtime = IoRuntime::new().unwrap();
    let missing =
        runtime.block_on(SqliteDriver.connect(&ConnectionProfile::new("sqlite", 0), None));
    assert!(missing.is_err());

    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("notes.txt");
    std::fs::write(
        &file,
        "This is not a database, but it is long enough to look like one.",
    )
    .unwrap();
    let error = match runtime.block_on(SqliteDriver.connect(&profile(file.to_str().unwrap()), None))
    {
        Ok(_) => panic!("a text file is not a database"),
        Err(error) => error,
    };
    let error = error
        .downcast_ref::<DatabaseError>()
        .expect("a database error");
    assert_eq!(error.code(), Some("SQLITE_NOTADB"));
}

#[test]
fn values_keep_their_storage_class_and_columns_their_declared_type() {
    with_database(":memory:", |connection| async move {
        run(
            &connection,
            "CREATE TABLE items (id INTEGER PRIMARY KEY, price NUMERIC(10,2), active BOOLEAN, \
             note TEXT, data BLOB, ratio REAL, created DATETIME)",
        )
        .await;
        run(
            &connection,
            "INSERT INTO items VALUES \
             (1, 9.9, 1, 'first', X'0A0BFF', 0.5, '2024-01-02 03:04:05'), \
             (2, NULL, 0, NULL, NULL, NULL, NULL)",
        )
        .await;

        let StatementOutcome::Rows(stream) =
            run(&connection, "SELECT * FROM items ORDER BY id").await
        else {
            panic!("expected rows");
        };
        let columns: Vec<(String, TypeCategory)> = stream
            .columns()
            .iter()
            .map(|column| (column.type_name().to_string(), column.category()))
            .collect();
        assert_eq!(
            columns,
            [
                ("INTEGER".into(), TypeCategory::Integer),
                ("NUMERIC(10,2)".into(), TypeCategory::Decimal),
                ("BOOLEAN".into(), TypeCategory::Boolean),
                ("TEXT".into(), TypeCategory::Text),
                ("BLOB".into(), TypeCategory::Binary),
                ("REAL".into(), TypeCategory::Float),
                ("DATETIME".into(), TypeCategory::Temporal),
            ]
        );
        let rows: Vec<Vec<Value>> = stream.map(|row| row.unwrap().into_vec()).collect().await;
        assert_eq!(
            rows[0],
            vec![
                Value::Int(1),
                Value::Float(9.9),
                Value::Bool(true),
                Value::Text("first".into()),
                Value::Text("X'0A0BFF'".into()),
                Value::Float(0.5),
                Value::Text("2024-01-02 03:04:05".into()),
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

        // An expression has no declared type; its first value decides.
        let StatementOutcome::Rows(stream) = run(
            &connection,
            "SELECT 1 + 1 AS n, 'x' AS t, 2.5 AS r, X'00' AS b, NULL AS z",
        )
        .await
        else {
            panic!("expected rows");
        };
        let columns: Vec<(String, TypeCategory)> = stream
            .columns()
            .iter()
            .map(|column| (column.type_name().to_string(), column.category()))
            .collect();
        assert_eq!(
            columns,
            [
                ("integer".into(), TypeCategory::Integer),
                ("text".into(), TypeCategory::Text),
                ("real".into(), TypeCategory::Float),
                ("blob".into(), TypeCategory::Binary),
                ("".into(), TypeCategory::Text),
            ]
        );

        // A blob shown as a literal is written back as a blob.
        let dialect = SqliteDialect;
        run(
            &connection,
            &dialect.row_change(
                "main",
                "items",
                &RowChange::Update {
                    key: vec![("id".into(), Value::Int(2))],
                    values: vec![("data".into(), Value::Text("X'0A0BFF'".into()))],
                },
            ),
        )
        .await;
        let (_, rows) = select(
            &connection,
            "SELECT typeof(data), data = X'0A0BFF' FROM items WHERE id = 2",
        )
        .await;
        assert_eq!(rows, vec![vec![Value::Text("blob".into()), Value::Int(1)]]);
    });
}

#[test]
fn commands_report_what_they_did() {
    with_database(":memory:", |connection| async move {
        let StatementOutcome::Command(created) =
            run(&connection, "CREATE TABLE t (a INTEGER)").await
        else {
            panic!("expected a command");
        };
        assert_eq!(created.tag(), "CREATE TABLE");
        assert_eq!(created.rows(), None);

        let StatementOutcome::Command(inserted) = run(
            &connection,
            "INSERT INTO t WITH RECURSIVE s(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM s \
             WHERE n < 3) SELECT n FROM s",
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!((inserted.tag(), inserted.rows()), ("INSERT", Some(3)));

        let StatementOutcome::Command(updated) =
            run(&connection, "UPDATE t SET a = a * 10 WHERE a > 1").await
        else {
            panic!("expected a command");
        };
        assert_eq!((updated.tag(), updated.rows()), ("UPDATE", Some(2)));

        let StatementOutcome::Command(index) =
            run(&connection, "CREATE UNIQUE INDEX t_a ON t (a)").await
        else {
            panic!("expected a command");
        };
        assert_eq!((index.tag(), index.rows()), ("CREATE INDEX", None));

        // Several statements run in order and report the last.
        let StatementOutcome::Command(deleted) = run(
            &connection,
            "DELETE FROM t WHERE a = 1; DELETE FROM t WHERE a > 1;",
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!((deleted.tag(), deleted.rows()), ("DELETE", Some(2)));

        let StatementOutcome::Command(nothing) = run(&connection, "-- only a comment").await else {
            panic!("expected a command");
        };
        assert_eq!(nothing.tag(), "");

        // A transaction belongs to the session.
        run(&connection, "BEGIN").await;
        run(&connection, "INSERT INTO t VALUES (7)").await;
        run(&connection, "ROLLBACK").await;
        let (_, rows) = select(&connection, "SELECT count(*) FROM t").await;
        assert_eq!(rows, vec![vec![Value::Int(0)]]);
    });
}

#[test]
fn errors_carry_sqlite_codes_and_point_at_the_offending_text() {
    with_database(":memory:", |connection| async move {
        let sql = "SELECT 1, nope FROM sqlite_schema";
        let syntax = error(&connection, sql).await;
        assert_eq!(syntax.code(), Some("SQLITE_ERROR"));
        assert!(syntax.message().contains("nope"), "{}", syntax.message());
        assert_eq!(&sql[syntax.position().expect("a position")..][..4], "nope");

        run(
            &connection,
            "CREATE TABLE parents (id INTEGER PRIMARY KEY, email TEXT UNIQUE)",
        )
        .await;
        run(
            &connection,
            "CREATE TABLE children (parent_id INTEGER REFERENCES parents (id))",
        )
        .await;
        run(
            &connection,
            "INSERT INTO parents VALUES (1, 'a@example.com')",
        )
        .await;
        let unique = error(
            &connection,
            "INSERT INTO parents VALUES (2, 'a@example.com')",
        )
        .await;
        assert_eq!(unique.code(), Some("SQLITE_CONSTRAINT_UNIQUE"));
        assert!(unique.message().contains("parents.email"));

        // Every session enforces foreign keys.
        let foreign = error(&connection, "INSERT INTO children VALUES (99)").await;
        assert_eq!(foreign.code(), Some("SQLITE_CONSTRAINT_FOREIGNKEY"));
    });
}

#[test]
fn a_running_statement_can_be_cancelled() {
    with_database(":memory:", |connection| async move {
        let running = tokio::spawn({
            let connection = connection.clone();
            async move { connection.execute(COUNT_TO_A_BILLION.into()).await }
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let started = Instant::now();
        connection.cancel().await.expect("cancel");
        let error = match running.await.unwrap() {
            Ok(_) => panic!("the statement was cancelled"),
            Err(error) => error,
        };
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.code(), Some("SQLITE_INTERRUPT"));
        assert!(started.elapsed() < Duration::from_secs(5));

        // The session goes on, and a cancel while idle changes nothing.
        connection.cancel().await.expect("cancel");
        let (_, rows) = select(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
    });
}

#[test]
fn dropping_a_long_result_frees_the_session() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("long.db");
    with_database(file.to_str().unwrap(), |connection| async move {
        let StatementOutcome::Rows(mut stream) = run(
            &connection,
            "WITH RECURSIVE g(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM g WHERE n < 50000000) \
             SELECT n, printf('%100s', n) FROM g",
        )
        .await
        else {
            panic!("expected rows");
        };
        assert!(stream.next().await.is_some());
        drop(stream);

        let started = Instant::now();
        let (_, rows) = select(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the abandoned statement was stopped, not drained ({:?})",
            started.elapsed()
        );
    });
}

#[test]
fn dropping_a_statement_before_its_first_row_frees_the_session() {
    with_database(":memory:", |connection| async move {
        let abandoned = tokio::time::timeout(
            Duration::from_millis(300),
            connection.execute(COUNT_TO_A_BILLION.into()),
        )
        .await;
        assert!(abandoned.is_err(), "the statement is still computing");

        let started = Instant::now();
        let (_, rows) = select(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
        assert!(started.elapsed() < Duration::from_secs(5));
    });
}

/// A schema with one of everything introspection reads.
const SHOP: &[&str] = &[
    "CREATE TABLE customers (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        email TEXT NOT NULL UNIQUE,
        name TEXT DEFAULT 'anonymous',
        created TEXT DEFAULT (datetime('now'))
    )",
    "CREATE TABLE orders (
        id INTEGER PRIMARY KEY,
        customer_id INTEGER NOT NULL CONSTRAINT orders_customer REFERENCES customers (id) ON DELETE CASCADE,
        total NUMERIC(12,2) NOT NULL DEFAULT 0 CHECK (total >= 0),
        doubled AS (total * 2),
        status TEXT
    )",
    "CREATE INDEX orders_big ON orders (total DESC) WHERE total > 100",
    "CREATE INDEX orders_status ON orders (lower(status), customer_id)",
    "CREATE TABLE LineItems (
        OrderId INTEGER NOT NULL REFERENCES orders,
        Line INTEGER NOT NULL,
        Qty INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY (OrderId, Line)
    )",
    "CREATE VIEW big_orders AS SELECT id, total FROM orders WHERE total > 100",
    "CREATE TRIGGER orders_touch AFTER UPDATE OF total ON orders
     BEGIN
         UPDATE customers SET name = name WHERE id = NEW.customer_id;
     END",
];

#[test]
fn introspection_describes_tables_views_indexes_keys_and_triggers() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("shop.db");
    let attached = directory.path().join("archive.db");
    let attached = attached.to_str().unwrap().replace('\'', "''");
    with_database(file.to_str().unwrap(), |connection| async move {
        for statement in SHOP {
            run(&connection, statement).await;
        }

        let names = |schemas: Vec<Schema>| -> Vec<String> {
            schemas
                .iter()
                .map(|schema| schema.name().to_string())
                .collect()
        };
        assert_eq!(
            names(connection.introspect_schemas().await.unwrap()),
            ["main"]
        );
        run(
            &connection,
            &format!("ATTACH DATABASE '{attached}' AS archive"),
        )
        .await;
        run(&connection, "CREATE TEMP TABLE scratch (x)").await;
        assert_eq!(
            names(connection.introspect_schemas().await.unwrap()),
            ["main", "temp", "archive"]
        );
        assert_eq!(
            connection.search_path().await.unwrap(),
            vec![Arc::<str>::from("main")]
        );

        let schema = connection.introspect_schema("main".into()).await.unwrap();
        let names: Vec<String> = schema
            .relations()
            .unwrap()
            .iter()
            .map(|relation| relation.name().to_string())
            .collect();
        assert_eq!(names, ["LineItems", "big_orders", "customers", "orders"]);

        let customers = schema.relation("customers").unwrap();
        let id = customers.column("id").unwrap();
        assert!(id.is_primary_key() && id.is_auto_increment() && !id.is_nullable());
        assert_eq!(
            customers.column("name").unwrap().default(),
            Some("'anonymous'")
        );
        assert_eq!(
            customers.column("created").unwrap().default(),
            Some("datetime('now')")
        );
        let email = customers
            .constraints()
            .iter()
            .find(|constraint| matches!(constraint.rule(), ConstraintRule::Unique { .. }))
            .unwrap();
        assert_eq!(&*email.name(), "customers_email_key");
        assert_eq!(email.rule().columns(), [Arc::from("email")]);

        let orders = schema.relation("orders").unwrap();
        assert!(!orders.column("id").unwrap().is_auto_increment());
        let doubled = orders.column("doubled").unwrap();
        assert!(doubled.is_generated());
        assert_eq!(doubled.default(), Some("total * 2"));
        let (constraint, key) = orders.foreign_keys().next().expect("a foreign key");
        assert_eq!(&*constraint.name(), "orders_customer");
        assert_eq!(
            (key.referenced_relation(), key.referenced_columns()),
            ("customers", &[Arc::from("id")][..])
        );
        assert_eq!(key.on_delete(), ReferentialAction::Cascade);
        assert!(orders.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Check { expression } if &**expression == "total >= 0"
        )));
        let big = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "orders_big")
            .unwrap();
        assert_eq!(big.columns(), [Arc::from("total")]);
        assert_eq!(big.predicate(), Some("total > 100"));
        let status = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "orders_status")
            .unwrap();
        assert_eq!(
            status.columns(),
            [Arc::from("lower(status)"), Arc::from("customer_id")]
        );
        assert_eq!(orders.triggers().len(), 1);
        assert_eq!(orders.triggers()[0].timing(), "AFTER UPDATE FOR EACH ROW");

        // A key that names no columns references the primary key.
        let items = schema.relation("LineItems").unwrap();
        let (_, key) = items.foreign_keys().next().unwrap();
        assert_eq!(key.referenced_columns(), [Arc::from("id")]);
        let primary: Vec<String> = items
            .primary_key()
            .iter()
            .map(|column| column.name().to_string())
            .collect();
        assert_eq!(primary, ["OrderId", "Line"]);
        assert!(items.indexes().iter().any(|index| index.is_primary()));

        let view = schema.relation("big_orders").unwrap();
        assert_eq!(view.relation_type(), RelationType::View);
        assert_eq!(
            view.definition(),
            Some("SELECT id, total FROM orders WHERE total > 100")
        );
        assert_eq!(view.columns().len(), 2);

        let temp = connection.introspect_schema("temp".into()).await.unwrap();
        assert!(temp.relation("scratch").is_some());

        // Statistics, once gathered, estimate the row counts.
        run(
            &connection,
            "INSERT INTO customers (email) VALUES ('a@x'), ('b@x')",
        )
        .await;
        run(&connection, "ANALYZE").await;
        let schema = connection.introspect_schema("main".into()).await.unwrap();
        assert_eq!(
            schema.relation("customers").unwrap().estimated_rows(),
            Some(2)
        );
    });
}

#[test]
fn created_relations_read_back_as_they_were_introspected() {
    let runtime = IoRuntime::new().unwrap();
    let source = runtime.block_on(async {
        SqliteDriver
            .connect(&profile(":memory:"), None)
            .await
            .unwrap()
    });
    let copy = runtime.block_on(async {
        SqliteDriver
            .connect(&profile(":memory:"), None)
            .await
            .unwrap()
    });
    runtime.block_on(async move {
        for statement in SHOP {
            run(&source, statement).await;
        }
        let original = source.introspect_schema("main".into()).await.unwrap();

        let dialect = SqliteDialect;
        let statements = dialect.migrate(
            "main",
            &diff_schemas(&Schema::new("main").with_relations([]), &original),
        );
        for statement in &statements {
            run(&copy, statement).await;
        }
        let copied = copy.introspect_schema("main".into()).await.unwrap();
        assert_eq!(copied.relations(), original.relations());
    });
}

#[test]
fn a_rebuilt_table_keeps_its_rows_indexes_triggers_and_views() {
    with_database(":memory:", |connection| async move {
        for statement in SHOP {
            run(&connection, statement).await;
        }
        run(
            &connection,
            "INSERT INTO customers (id, email) VALUES (1, 'a@x')",
        )
        .await;
        run(
            &connection,
            "INSERT INTO orders (customer_id, total, status) VALUES (1, 150, NULL), (1, 5, 'open')",
        )
        .await;
        let before = connection.introspect_schema("main".into()).await.unwrap();

        // Make `status` NOT NULL with a default, which needs a rebuild.
        let orders = before.relation("orders").unwrap();
        let mut columns = orders.columns().to_vec();
        let status = columns
            .iter_mut()
            .find(|column| &*column.name() == "status")
            .unwrap();
        *status = status.clone().nullable(false).with_default("'new'");
        let edited = orders.clone().with_columns(columns);
        let after = Schema::new("main").with_relations(before.relations().unwrap().iter().map(
            |relation| {
                if &*relation.name() == "orders" {
                    edited.clone()
                } else {
                    relation.clone()
                }
            },
        ));
        let dialect = SqliteDialect;
        // The NULL status would violate the new constraint; fill it first.
        run(
            &connection,
            "UPDATE orders SET status = 'new' WHERE status IS NULL",
        )
        .await;
        for statement in dialect.migrate("main", &diff_schemas(&before, &after)) {
            run(&connection, &statement).await;
        }

        let rebuilt = connection.introspect_schema("main".into()).await.unwrap();
        assert_eq!(rebuilt.relation("orders"), Some(&edited));
        let (_, rows) = select(&connection, "SELECT id, total FROM big_orders").await;
        assert_eq!(rows, vec![vec![Value::Int(1), Value::Int(150)]]);
        let (_, rows) = select(&connection, "PRAGMA foreign_keys").await;
        assert_eq!(rows, vec![vec![Value::Int(1)]]);
    });
}

#[test]
fn a_plan_is_explained_as_a_tree() {
    with_database(":memory:", |connection| async move {
        for statement in SHOP {
            run(&connection, statement).await;
        }
        let dialect = SqliteDialect;
        let explain = dialect
            .explain(
                "SELECT * FROM orders JOIN customers ON customers.id = orders.customer_id \
                 ORDER BY orders.status",
                false,
            )
            .unwrap();
        let (_, rows) = select(&connection, &explain).await;
        let plan = dialect.parse_plan(&rows).unwrap();
        let operations: Vec<(String, Option<String>)> = plan
            .flatten()
            .iter()
            .map(|(_, node)| {
                (
                    node.operation().to_string(),
                    node.target().map(String::from),
                )
            })
            .collect();
        assert!(
            operations.contains(&("SCAN".into(), Some("orders".into()))),
            "{operations:?}"
        );
        assert!(
            operations.contains(&("SEARCH".into(), Some("customers".into()))),
            "{operations:?}"
        );
    });
}
