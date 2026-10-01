//! The driver against a real server.
//!
//! These run only when `DATAKIT_TEST_MSSQL_URL` names a server, as
//! `sqlserver://user:password@host:port/database`; each test works in a
//! schema of its own and drops it afterwards. Without the variable they pass
//! without doing anything, so `cargo test` stays green on a machine with no
//! SQL Server.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use datakit_catalog::{ConstraintRule, ReferentialAction, RelationType, RoutineType};
use datakit_driver::{
    Connection, ConnectionProfile, DatabaseError, Dialect as _, Driver, StatementOutcome,
    TypeCategory, Value,
};
use datakit_driver_mssql::{SqlServerDialect, SqlServerDriver};
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

struct Server {
    profile: ConnectionProfile,
    password: String,
}

fn server() -> Option<Server> {
    let url = std::env::var("DATAKIT_TEST_MSSQL_URL").ok()?;
    let rest = url
        .strip_prefix("sqlserver://")
        .or_else(|| url.strip_prefix("mssql://"))?;
    let (credentials, address) = rest.rsplit_once('@')?;
    let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
    let (host_port, database) = address.split_once('/').unwrap_or((address, "master"));
    let (host, port) = host_port.split_once(':').unwrap_or((host_port, "1433"));
    Some(Server {
        profile: ConnectionProfile::new(SqlServerDriver::ID, port.parse().ok()?)
            .with_host(host)
            .with_user(user)
            .with_database(database)
            .with_ssl_mode(datakit_driver::SslMode::Prefer),
        password: password.to_string(),
    })
}

/// The batch that drops `schema` with everything in it; SQL Server has no
/// `DROP SCHEMA … CASCADE`.
fn drop_schema(schema: &str) -> String {
    format!(
        "DECLARE @sql nvarchar(max) = N''; \
         SELECT @sql += N'ALTER TABLE ' + QUOTENAME(s.name) + N'.' + QUOTENAME(t.name) \
             + N' DROP CONSTRAINT ' + QUOTENAME(fk.name) + N'; ' \
         FROM sys.foreign_keys fk \
         JOIN sys.tables t ON t.object_id = fk.parent_object_id \
         JOIN sys.schemas s ON s.schema_id = t.schema_id WHERE s.name = N'{schema}'; \
         SELECT @sql += N'DROP ' + CASE o.type WHEN 'V' THEN N'VIEW' WHEN 'U' THEN N'TABLE' \
             WHEN 'P' THEN N'PROCEDURE' WHEN 'SO' THEN N'SEQUENCE' ELSE N'FUNCTION' END \
             + N' ' + QUOTENAME(s.name) + N'.' + QUOTENAME(o.name) + N'; ' \
         FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
         WHERE s.name = N'{schema}' AND o.type IN ('V', 'U', 'P', 'FN', 'IF', 'TF', 'SO') \
         ORDER BY CASE o.type WHEN 'V' THEN 0 WHEN 'P' THEN 1 WHEN 'U' THEN 3 ELSE 2 END; \
         EXEC (@sql); \
         IF SCHEMA_ID(N'{schema}') IS NOT NULL EXEC (N'DROP SCHEMA [{schema}]')"
    )
}

/// Run `test` with a connection and a fresh schema named after the test.
fn with_schema<F, Fut>(name: &str, test: F)
where
    F: FnOnce(Arc<dyn Connection>, String) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(server) = server() else {
        eprintln!("DATAKIT_TEST_MSSQL_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    let schema = format!("datakit_test_{name}");
    let connection = runtime.block_on(async {
        SqlServerDriver
            .connect(&server.profile, Some(server.password.clone()))
            .await
            .expect("connect")
    });
    runtime.block_on(async {
        run(&connection, &drop_schema(&schema)).await;
        run(&connection, &format!("CREATE SCHEMA {schema}")).await;
    });
    runtime.block_on(test(connection.clone(), schema.clone()));
    runtime.block_on(run(&connection, &drop_schema(&schema)));
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

async fn error_of(connection: &Arc<dyn Connection>, sql: &str) -> DatabaseError {
    let error = match connection.execute(sql.into()).await {
        Ok(StatementOutcome::Rows(stream)) => stream
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .find_map(Result::err)
            .expect("the statement must fail"),
        Ok(StatementOutcome::Command(_)) => panic!("the statement must fail"),
        Err(error) => error,
    };
    error
        .downcast_ref::<DatabaseError>()
        .expect("a database error")
        .clone()
}

#[test]
fn values_keep_exact_text_and_numbers_parse() {
    with_schema("values", |connection, schema| async move {
        run(
            &connection,
            &format!(
                "CREATE TABLE {schema}.items (id int PRIMARY KEY, price decimal(10,2), \
                 active bit, note nvarchar(40), data varbinary(8), uid uniqueidentifier, \
                 placed datetime2(3), day date, ratio float, cost money, big bigint)"
            ),
        )
        .await;
        run(
            &connection,
            &format!(
                "INSERT INTO {schema}.items VALUES \
                 (1, 9.90, 1, N'Grüße', 0xDEAD01, '6F9619FF-8B86-D011-B42D-00C04FC964FF', \
                  '2024-02-29 13:05:09.123', '2024-02-29', 0.5, 12.5, 9007199254740993), \
                 (2, NULL, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL)"
            ),
        )
        .await;

        let StatementOutcome::Rows(stream) = run(
            &connection,
            &format!("SELECT * FROM {schema}.items ORDER BY id"),
        )
        .await
        else {
            panic!("expected rows");
        };
        let types: Vec<(String, TypeCategory)> = stream
            .columns()
            .iter()
            .map(|column| (column.type_name().to_string(), column.category()))
            .collect();
        assert_eq!(
            types,
            [
                ("int".into(), TypeCategory::Integer),
                ("decimal".into(), TypeCategory::Decimal),
                ("bit".into(), TypeCategory::Boolean),
                ("nvarchar".into(), TypeCategory::Text),
                ("varbinary".into(), TypeCategory::Binary),
                ("uniqueidentifier".into(), TypeCategory::Uuid),
                ("datetime2".into(), TypeCategory::Temporal),
                ("date".into(), TypeCategory::Temporal),
                ("float".into(), TypeCategory::Float),
                ("money".into(), TypeCategory::Decimal),
                ("bigint".into(), TypeCategory::Integer),
            ]
        );
        let rows: Vec<Vec<Value>> = stream
            .map(|row| row.expect("row").into_vec())
            .collect()
            .await;
        assert_eq!(
            rows[0],
            vec![
                Value::Int(1),
                Value::Text("9.90".into()),
                Value::Bool(true),
                Value::Text("Grüße".into()),
                Value::Text("0xDEAD01".into()),
                Value::Text("6F9619FF-8B86-D011-B42D-00C04FC964FF".into()),
                Value::Text("2024-02-29 13:05:09.123".into()),
                Value::Text("2024-02-29".into()),
                Value::Float(0.5),
                Value::Text("12.5000".into()),
                Value::Int(9_007_199_254_740_993),
            ]
        );
        assert_eq!(rows[1][0], Value::Int(2));
        assert!(
            rows[1][1..]
                .iter()
                .all(|value| *value == Value::Bool(false) || value.is_null())
        );
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
            &format!("INSERT INTO {schema}.t VALUES (1), (2), (3) -- three"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!(inserted.tag(), "INSERT");
        assert_eq!(inserted.rows(), Some(3));

        let StatementOutcome::Command(updated) = run(
            &connection,
            &format!("UPDATE {schema}.t SET a = a + 1 WHERE a > 1"),
        )
        .await
        else {
            panic!("expected a command");
        };
        assert_eq!((updated.tag(), updated.rows()), ("UPDATE", Some(2)));

        // `OUTPUT` makes a data change return rows.
        let (names, output) = rows(
            &connection,
            &format!("DELETE FROM {schema}.t OUTPUT deleted.a WHERE a = 1"),
        )
        .await;
        assert_eq!(names, ["a"]);
        assert_eq!(output, vec![vec![Value::Int(1)]]);

        // Session state lasts: a temporary table outlives its batch.
        run(&connection, "CREATE TABLE #scratch (n int)").await;
        run(&connection, "INSERT INTO #scratch VALUES (7)").await;
        let (_, scratch) = rows(&connection, "SELECT n FROM #scratch").await;
        assert_eq!(scratch, vec![vec![Value::Int(7)]]);
    });
}

#[test]
fn errors_point_at_the_offending_line() {
    with_schema("errors", |connection, _| async move {
        let sql = "SELECT 1,\n  nope\nFROM sys.objects";
        let error = error_of(&connection, sql).await;
        assert_eq!(error.code(), Some("207"));
        assert!(error.message().contains("nope"), "{}", error.message());
        assert_eq!(&sql[error.position().unwrap()..][..4], "nope");
        assert!(error.detail().unwrap().starts_with("Msg 207, Level 16"));

        // A failed data change reports the server's error, not a count.
        let error = error_of(&connection, "INSERT INTO nowhere VALUES (1)").await;
        assert_eq!(error.code(), Some("208"));

        // The session goes on.
        let (_, rows) = rows(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
    });
}

#[test]
fn dropping_a_long_result_frees_the_session() {
    with_schema("abandon", |connection, _| async move {
        let StatementOutcome::Rows(mut stream) = run(
            &connection,
            "SELECT a.object_id, REPLICATE('x', 100) FROM sys.all_objects a \
             CROSS JOIN sys.all_objects b CROSS JOIN sys.all_objects c",
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
        assert!(!connection.is_closed());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the abandoned statement was stopped, not drained ({:?})",
            started.elapsed()
        );
    });
}

#[test]
fn a_running_statement_can_be_cancelled() {
    with_schema("cancel", |connection, _| async move {
        let running = tokio::spawn({
            let connection = connection.clone();
            async move { connection.execute("WAITFOR DELAY '00:00:30'".into()).await }
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        let started = Instant::now();
        connection.cancel().await.expect("cancel");
        let error = match running.await.unwrap() {
            Ok(_) => panic!("the statement was cancelled"),
            Err(error) => error,
        };
        let error = error
            .downcast_ref::<DatabaseError>()
            .expect("a database error");
        assert_eq!(error.message(), "The statement was cancelled");
        assert!(started.elapsed() < Duration::from_secs(10));

        let (_, rows) = rows(&connection, "SELECT 42").await;
        assert_eq!(rows, vec![vec![Value::Int(42)]]);
        assert!(!connection.is_closed());
    });
}

#[test]
fn introspection_describes_schemas_relations_and_columns() {
    with_schema("introspect", |connection, schema| async move {
        for statement in [
            format!(
                "CREATE TABLE {schema}.orders (id bigint IDENTITY PRIMARY KEY, \
                 total decimal(12,2) NOT NULL DEFAULT 0, note nvarchar(80), \
                 body varchar(max), doubled AS (total * 2))"
            ),
            format!(
                "EXEC sys.sp_addextendedproperty @name = N'MS_Description', \
                 @value = N'Every order', @level0type = N'SCHEMA', @level0name = N'{schema}', \
                 @level1type = N'TABLE', @level1name = N'orders'"
            ),
            format!(
                "CREATE VIEW {schema}.big_orders AS SELECT id, total FROM {schema}.orders \
                 WHERE total > 100"
            ),
        ] {
            run(&connection, &statement).await;
        }

        let schemas = connection.introspect_schemas().await.unwrap();
        let found = schemas.iter().find(|s| *s.name() == *schema).unwrap();
        assert!(!found.is_system());
        for system in ["sys", "INFORMATION_SCHEMA", "db_owner"] {
            assert!(
                schemas
                    .iter()
                    .any(|s| &*s.name() == system && s.is_system()),
                "{system}"
            );
        }
        assert!(
            schemas
                .iter()
                .any(|s| &*s.name() == "dbo" && !s.is_system())
        );

        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let relations = loaded.relations().unwrap();
        let names: Vec<_> = relations.iter().map(|r| r.name().to_string()).collect();
        assert_eq!(names, ["big_orders", "orders"]);
        let orders = loaded.relation("orders").unwrap();
        assert_eq!(orders.relation_type(), RelationType::Table);
        assert_eq!(orders.comment(), Some("Every order"));
        let columns: Vec<_> = orders
            .columns()
            .iter()
            .filter(|c| !c.is_generated())
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
                ("note".into(), "nvarchar(80)".into(), true, false),
                ("body".into(), "varchar(max)".into(), true, false),
            ]
        );
        assert!(orders.column("id").unwrap().is_auto_increment());
        assert_eq!(orders.column("total").unwrap().default(), Some("0"));
        let doubled = orders.column("doubled").unwrap();
        assert!(doubled.is_generated());
        assert_eq!(doubled.default(), Some("[total]*(2)"));

        let view = loaded.relation("big_orders").unwrap();
        assert_eq!(view.relation_type(), RelationType::View);
        assert_eq!(view.columns().len(), 2);
        assert!(view.definition().unwrap().starts_with("CREATE VIEW"));

        let search_path = connection.search_path().await.unwrap();
        assert_eq!(search_path.len(), 1);
    });
}

#[test]
fn introspection_reads_keys_indexes_triggers_routines_and_sequences() {
    with_schema("objects", |connection, schema| async move {
        for statement in [
            format!(
                "CREATE TABLE {schema}.customers (id int IDENTITY CONSTRAINT PK_customers \
                 PRIMARY KEY, email nvarchar(200) CONSTRAINT UQ_customers_email UNIQUE)"
            ),
            format!(
                "CREATE TABLE {schema}.orders (id int NOT NULL CONSTRAINT PK_orders \
                 PRIMARY KEY, customer_id int NOT NULL CONSTRAINT FK_orders_customers \
                 REFERENCES {schema}.customers (id) ON DELETE CASCADE, total decimal(12,2) \
                 CONSTRAINT CK_orders_total CHECK (total >= 0))"
            ),
            format!(
                "CREATE INDEX IX_orders_big ON {schema}.orders (total DESC) \
                 INCLUDE (customer_id) WHERE total > 100"
            ),
            format!(
                "CREATE TRIGGER {schema}.orders_touch ON {schema}.orders AFTER INSERT, UPDATE \
                 AS BEGIN SET NOCOUNT ON END"
            ),
            format!(
                "CREATE PROCEDURE {schema}.touch @id int, @note nvarchar(20) OUTPUT \
                 AS SELECT @note = N'ok'"
            ),
            format!(
                "CREATE FUNCTION {schema}.twice (@n int) RETURNS int AS BEGIN RETURN @n * 2 END"
            ),
            format!(
                "CREATE SEQUENCE {schema}.invoice_numbers AS bigint START WITH 1000 INCREMENT BY 10"
            ),
        ] {
            run(&connection, &statement).await;
        }

        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let orders = loaded.relation("orders").unwrap();
        let (_, key) = orders.foreign_keys().next().expect("a foreign key");
        assert_eq!(key.referenced_relation(), "customers");
        assert_eq!(key.referenced_schema(), schema);
        assert_eq!(key.on_delete(), ReferentialAction::Cascade);
        assert!(orders.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Check { expression } if expression.contains("[total]>=(0)")
        )));
        let primary = orders
            .constraints()
            .iter()
            .find(|constraint| &*constraint.name() == "PK_orders")
            .unwrap();
        assert_eq!(primary.definition(), Some("PRIMARY KEY CLUSTERED (id)"));

        let filtered = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "IX_orders_big")
            .unwrap();
        assert_eq!(filtered.columns(), [Arc::from("total DESC")]);
        assert!(filtered.predicate().unwrap().starts_with("[total]>(100"));
        assert!(
            filtered
                .definition()
                .unwrap()
                .contains("INCLUDE (customer_id) WHERE [total]>(100")
        );
        assert!(orders.indexes().iter().any(|index| index.is_primary()));

        let customers = loaded.relation("customers").unwrap();
        assert!(customers.constraints().iter().any(|constraint| matches!(
            constraint.rule(),
            ConstraintRule::Unique { columns } if columns.len() == 1
        )));

        let trigger = &orders.triggers()[0];
        assert_eq!(trigger.timing(), "AFTER INSERT, UPDATE");
        assert!(trigger.definition().unwrap().starts_with("CREATE TRIGGER"));

        let touch = loaded.routines_named("touch").next().unwrap();
        assert_eq!(touch.routine_type(), RoutineType::Procedure);
        assert_eq!(touch.arguments(), "@id int, @note nvarchar(20) OUTPUT");
        assert!(touch.definition().unwrap().contains("SELECT @note"));
        let twice = loaded.routines_named("twice").next().unwrap();
        assert_eq!(twice.result(), Some("int"));

        let sequence = loaded.sequence("invoice_numbers").unwrap();
        assert_eq!((sequence.start(), sequence.increment()), (1000, 10));
    });
}

#[test]
fn ddl_round_trips_through_the_server() {
    with_schema("ddl", |connection, schema| async move {
        let dialect = SqlServerDialect;
        run(
            &connection,
            &format!(
                "CREATE TABLE {schema}.orders (id int IDENTITY PRIMARY KEY, \
                 total decimal(12,2) NOT NULL DEFAULT 0, note nvarchar(80))"
            ),
        )
        .await;
        run(
            &connection,
            &format!("CREATE INDEX IX_orders_total ON {schema}.orders (total) WHERE total > 0"),
        )
        .await;
        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let orders = loaded.relation("orders").unwrap().clone();

        // The table as the dialect writes it, under another name.
        let copy = datakit_catalog::Relation::new("orders_copy", RelationType::Table)
            .with_columns(orders.columns().to_vec())
            .with_comment("A copy");
        for statement in dialect.create_relation(&schema, &copy) {
            run(&connection, &statement).await;
        }
        let index = orders
            .indexes()
            .iter()
            .find(|index| &*index.name() == "IX_orders_total")
            .unwrap();
        run(&connection, &dialect.drop_index(&schema, "orders", index)).await;
        run(&connection, &dialect.create_index(&schema, "orders", index)).await;

        // A column's type, nullability and default change.
        let old = orders.column("total").unwrap().clone();
        let new = old
            .clone()
            .with_data_type("decimal(14,2)")
            .nullable(true)
            .with_default("1");
        for statement in dialect.alter_column(&schema, "orders_copy", &old, &new) {
            run(&connection, &statement).await;
        }
        run(
            &connection,
            &dialect.rename_column(&schema, "orders_copy", "note", "remark"),
        )
        .await;

        let loaded = connection
            .introspect_schema(schema.as_str().into())
            .await
            .unwrap();
        let copy = loaded.relation("orders_copy").unwrap();
        assert_eq!(copy.comment(), Some("A copy"));
        let total = copy.column("total").unwrap();
        assert_eq!(
            (total.data_type(), total.is_nullable(), total.default()),
            ("decimal(14,2)", true, Some("1"))
        );
        assert!(copy.column("remark").is_some());
        assert!(copy.column("id").unwrap().is_auto_increment());
        assert!(
            loaded
                .relation("orders")
                .unwrap()
                .indexes()
                .iter()
                .any(|index| &*index.name() == "IX_orders_total")
        );

        // A dropped column with a default loses its default first.
        let statements = dialect.migrate(
            &schema,
            &datakit_catalog::diff_schemas(
                &datakit_catalog::Schema::new(schema.as_str()).with_relations([copy.clone()]),
                &datakit_catalog::Schema::new(schema.as_str()).with_relations([copy
                    .clone()
                    .with_columns(
                        copy.columns()
                            .iter()
                            .filter(|column| &*column.name() != "total")
                            .cloned()
                            .collect::<Vec<_>>(),
                    )]),
            ),
        );
        for statement in statements {
            run(&connection, &statement).await;
        }
        let (_, rows) = rows(
            &connection,
            &format!(
                "SELECT COUNT(*) FROM sys.columns WHERE object_id = OBJECT_ID('{schema}.orders_copy')"
            ),
        )
        .await;
        assert_eq!(rows, vec![vec![Value::Int(2)]]);
    });
}

#[test]
fn plans_come_back_as_a_tree_and_the_setting_is_undone() {
    with_schema("plans", |connection, schema| async move {
        run(
            &connection,
            &format!("CREATE TABLE {schema}.t (a int PRIMARY KEY, b int)"),
        )
        .await;
        let dialect = SqlServerDialect;
        for analyze in [false, true] {
            let sql = dialect
                .explain(&format!("SELECT * FROM {schema}.t WHERE b > 1"), analyze)
                .unwrap();
            let (_, documents) = rows(&connection, &sql).await;
            let plan = dialect.parse_plan(&documents).unwrap();
            assert!(plan.operation().contains("Scan"), "{}", plan.operation());
            if analyze {
                assert!(plan.actual_rows().is_some());
            }
            // The next statement runs, rather than being explained.
            let (_, answer) = rows(&connection, "SELECT 42").await;
            assert_eq!(answer, vec![vec![Value::Int(42)]]);
        }
    });
}
