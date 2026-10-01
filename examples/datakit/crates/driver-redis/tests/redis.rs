//! The driver against a real server.
//!
//! These run only when `DATAKIT_TEST_REDIS_URL` names a server, as
//! `redis://[user]:password@host:port`; each test works in a database of its
//! own, which it empties first. Without the variable they pass without doing
//! anything, so `cargo test` stays green on a machine with no Redis.

use std::sync::Arc;

use datakit_catalog::RelationType;
use datakit_driver::{
    Connection, ConnectionProfile, Dialect as _, Driver, StatementOutcome, Value,
};
use datakit_driver_redis::{RedisDialect, RedisDriver};
use datakit_runtime::IoRuntime;
use futures::StreamExt as _;

struct Server {
    profile: ConnectionProfile,
    password: String,
}

fn server(database: u8) -> Option<Server> {
    let url = std::env::var("DATAKIT_TEST_REDIS_URL").ok()?;
    let rest = url.strip_prefix("redis://")?;
    let (credentials, address) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, password) = credentials.split_once(':').unwrap_or(("", credentials));
    let (host, port) = address.split_once(':').unwrap_or((address, "6379"));
    Some(Server {
        profile: ConnectionProfile::new(RedisDriver::ID, port.trim_end_matches('/').parse().ok()?)
            .with_host(host)
            .with_user(user)
            .with_database(database.to_string()),
        password: password.to_string(),
    })
}

/// Run `test` with a connection to database `database`, emptied first.
fn with_database<F, Fut>(database: u8, test: F)
where
    F: FnOnce(Arc<dyn Connection>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(server) = server(database) else {
        eprintln!("DATAKIT_TEST_REDIS_URL is not set; skipping");
        return;
    };
    let runtime = IoRuntime::new().unwrap();
    let connection = runtime.block_on(async {
        RedisDriver
            .connect(&server.profile, Some(server.password.clone()))
            .await
            .expect("connect")
    });
    runtime.block_on(run(&connection, "FLUSHDB"));
    runtime.block_on(test(connection));
}

async fn run(connection: &Arc<dyn Connection>, line: &str) -> StatementOutcome {
    connection.execute(line.into()).await.expect(line)
}

async fn rows(connection: &Arc<dyn Connection>, line: &str) -> (Vec<String>, Vec<Vec<String>>) {
    match run(connection, line).await {
        StatementOutcome::Rows(stream) => {
            let names = stream
                .columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect();
            let rows = stream
                .map(|row| {
                    row.expect("row")
                        .iter()
                        .map(|value| match value {
                            Value::Null => "nil".to_string(),
                            value => value.display().unwrap_or_default().into_owned(),
                        })
                        .collect()
                })
                .collect()
                .await;
            (names, rows)
        }
        StatementOutcome::Command(summary) => panic!("expected rows, got {}", summary.tag()),
    }
}

#[test]
fn replies_become_rows() {
    with_database(9, |connection| async move {
        assert!(connection.server_version().starts_with("Redis "));
        match run(&connection, "SET greeting \"hello world\"").await {
            StatementOutcome::Command(summary) => assert_eq!(summary.tag(), "OK"),
            StatementOutcome::Rows(_) => panic!("SET returns a status"),
        }
        assert_eq!(
            rows(&connection, "GET greeting").await,
            (vec!["value".into()], vec![vec!["hello world".into()]])
        );
        assert_eq!(rows(&connection, "GET missing").await.1, [["nil"]]);

        run(&connection, "HSET user:1 name Ada lang Rust").await;
        let (columns, mut pairs) = rows(&connection, "HGETALL user:1").await;
        pairs.sort();
        assert_eq!(columns, ["field", "value"]);
        assert_eq!(pairs, [["lang", "Rust"], ["name", "Ada"]]);

        run(&connection, "RPUSH queue a b c").await;
        assert_eq!(
            rows(&connection, "LRANGE queue 1 -1").await,
            (
                vec!["index".into(), "value".into()],
                vec![vec!["1".into(), "b".into()], vec!["2".into(), "c".into()]]
            )
        );

        run(&connection, "ZADD board 10 ada 20 grace").await;
        assert_eq!(
            rows(&connection, "ZRANGE board 0 -1 WITHSCORES").await.1,
            [["ada", "10"], ["grace", "20"]]
        );
        assert!(matches!(
            run(&connection, "INCR counter").await,
            StatementOutcome::Command(summary) if summary.tag() == "INCR 1"
        ));
        assert_eq!(rows(&connection, "STRLEN greeting").await.1, [["11"]]);

        let error = connection
            .execute("NOSUCHCOMMAND".into())
            .await
            .err()
            .unwrap();
        assert!(
            error.to_string().to_lowercase().contains("unknown"),
            "{error}"
        );
        assert!(connection.execute("GET \"open".into()).await.is_err());
    });
}

#[test]
fn keys_are_the_relations_of_their_database() {
    with_database(10, |connection| async move {
        run(&connection, "SET plain value").await;
        run(&connection, "HSET user:1 name Ada").await;
        run(&connection, "SADD tags rust sql").await;
        run(&connection, "XADD events * kind login").await;

        let schemas = connection.introspect_schemas().await.unwrap();
        assert!(schemas.iter().any(|schema| &*schema.name() == "db10"));
        assert_eq!(connection.search_path().await.unwrap(), [Arc::from("db10")]);

        let schema = connection.introspect_schema("db10".into()).await.unwrap();
        let relations = schema.relations().unwrap();
        let names: Vec<_> = relations.iter().map(|relation| relation.name()).collect();
        assert_eq!(
            names,
            [
                Arc::from("events"),
                "plain".into(),
                "tags".into(),
                "user:1".into()
            ]
        );
        assert!(
            relations
                .iter()
                .all(|relation| relation.relation_type() == RelationType::Key)
        );
        let user = &relations[3];
        assert_eq!(user.comment(), Some("hash"));

        // Opening a key reads it as its type is read.
        let sql = RedisDialect.select_page("db10", "user:1", "", "", 100, 0);
        assert_eq!(rows(&connection, &sql).await.1, [["name", "Ada"]]);
        let sql = RedisDialect.select_page("db10", "tags", "", "", 100, 0);
        assert_eq!(rows(&connection, &sql).await.1, [["rust"], ["sql"]]);
        let sql = RedisDialect.select_page("db10", "events", "", "", 100, 0);
        assert_eq!(rows(&connection, &sql).await.1[0][1], "kind=login");
        let sql = RedisDialect.count_rows("db10", "tags", "");
        assert_eq!(rows(&connection, &sql).await.1, [["2"]]);
    });
}

#[test]
fn another_database_is_read_without_leaving_the_sessions_own() {
    with_database(11, |connection| async move {
        run(&connection, "SET here 1").await;
        let other = server(12).unwrap();
        let elsewhere = RedisDriver
            .connect(&other.profile, Some(other.password))
            .await
            .unwrap();
        run(&elsewhere, "FLUSHDB").await;
        run(&elsewhere, "SET there 2").await;

        let schema = connection.introspect_schema("db12".into()).await.unwrap();
        assert_eq!(schema.relations().unwrap()[0].name(), Arc::from("there"));
        let sql = RedisDialect.select_page("db12", "there", "", "", 10, 0);
        assert_eq!(rows(&connection, &sql).await.1, [["2"]]);
        // Still in its own database.
        assert_eq!(rows(&connection, "GET here").await.1, [["1"]]);
    });
}
