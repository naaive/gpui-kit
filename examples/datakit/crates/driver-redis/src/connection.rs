use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, Ordering},
};

use anyhow::{Context as _, Result, bail};
use datakit_catalog::{Role, Schema};
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, StatementOutcome, TypeCategory};
use datakit_driver::{Row, Value};
use futures::FutureExt as _;
use redis::{
    AsyncConnectionConfig, Client, ConnectionAddr, ConnectionInfo, IntoConnectionInfo as _,
    RedisConnectionInfo, Value as Reply, aio::MultiplexedConnection,
};
use tokio::sync::Mutex;

use crate::{
    command::split,
    introspect,
    reply::{bytes_text, outcome, scalar, table},
};

/// One session with a Redis server.
///
/// Commands run one at a time: some of them change the session's database
/// with `SELECT` and back, which nothing else may interleave with.
pub(crate) struct RedisConnection {
    connection: Arc<Mutex<MultiplexedConnection>>,
    /// The database the session has selected.
    database: Arc<AtomicI64>,
    version: Arc<str>,
    closed: Arc<AtomicBool>,
}

impl RedisConnection {
    pub(crate) async fn open(
        profile: &ConnectionProfile,
        password: Option<String>,
    ) -> Result<Self> {
        let database: i64 = match profile.database().trim() {
            "" => 0,
            text => text
                .trim_start_matches("db")
                .parse()
                .with_context(|| format!("“{text}” is not a database number"))?,
        };
        let user = profile.user().trim();
        let mut redis = RedisConnectionInfo::default().set_db(database);
        if !user.is_empty() {
            redis = redis.set_username(user);
        }
        if let Some(password) = password.filter(|password| !password.is_empty()) {
            redis = redis.set_password(password);
        }
        let info: ConnectionInfo = ConnectionAddr::Tcp(profile.host().to_string(), profile.port())
            .into_connection_info()?
            .set_redis_settings(redis);
        let client = Client::open(info)?;
        let mut connection = client
            .get_multiplexed_async_connection_with_config(&AsyncConnectionConfig::new())
            .await
            .with_context(|| format!("Couldn’t connect to {}", profile.address()))?;
        let info: String = redis::cmd("INFO")
            .arg("server")
            .query_async(&mut connection)
            .await?;
        let version = info
            .lines()
            .find_map(|line| line.strip_prefix("redis_version:"))
            .map(|version| format!("Redis {}", version.trim()))
            .unwrap_or_else(|| "Redis".to_string());
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            database: Arc::new(AtomicI64::new(database)),
            version: version.into(),
            closed: Arc::new(AtomicBool::new(false)),
        })
    }
}

impl Connection for RedisConnection {
    fn server_version(&self) -> Arc<str> {
        self.version.clone()
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        let connection = self.connection.clone();
        let database = self.database.clone();
        let closed = self.closed.clone();
        async move {
            let words: Vec<String> = split(sql.trim())?
                .iter()
                .map(|word| bytes_text(word))
                .collect();
            let raw = split(sql.trim())?;
            let Some(command) = words.first().map(|word| word.to_uppercase()) else {
                bail!("Type a command");
            };
            let mut connection = connection.lock().await;
            let result = match command.as_str() {
                "DATAKIT.VALUE" => value(&mut connection, &database, &words).await,
                "DATAKIT.LEN" => length(&mut connection, &database, &words).await,
                _ => {
                    let mut cmd = redis::cmd(&words[0]);
                    for argument in &raw[1..] {
                        cmd.arg(argument.as_slice());
                    }
                    let reply = cmd.query_async::<Reply>(&mut *connection).await;
                    if let Ok(Reply::Okay) = &reply
                        && command == "SELECT"
                        && let Some(selected) = words.get(1).and_then(|word| word.parse().ok())
                    {
                        database.store(selected, Ordering::Relaxed);
                    }
                    match reply {
                        Ok(reply) => outcome(&words, reply),
                        Err(error) => Err(error.into()),
                    }
                }
            };
            if let Err(error) = &result
                && error
                    .downcast_ref::<redis::RedisError>()
                    .is_some_and(|error| error.is_connection_dropped() || error.is_io_error())
            {
                closed.store(true, Ordering::Relaxed);
            }
            result
        }
        .boxed()
    }

    /// Redis runs one command at a time and has nothing to cancel.
    fn cancel(&self) -> BoxFuture<()> {
        async { Ok(()) }.boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        let connection = self.connection.clone();
        let database = self.database.load(Ordering::Relaxed);
        async move {
            let mut connection = connection.lock().await;
            introspect::databases(&mut connection, database).await
        }
        .boxed()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        let connection = self.connection.clone();
        let database = self.database.clone();
        async move {
            let number = database_number(&schema)?;
            let mut connection = connection.lock().await;
            in_database(&mut connection, &database, number, |connection| {
                introspect::keys(connection, schema.clone()).boxed()
            })
            .await
        }
        .boxed()
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        let database = self.database.load(Ordering::Relaxed);
        async move { Ok(vec![Arc::from(format!("db{database}"))]) }.boxed()
    }

    fn introspect_roles(&self) -> BoxFuture<Vec<Role>> {
        let connection = self.connection.clone();
        async move {
            let mut connection = connection.lock().await;
            introspect::users(&mut connection).await
        }
        .boxed()
    }
}

/// The number of the database `db3` names.
fn database_number(schema: &str) -> Result<i64> {
    schema
        .strip_prefix("db")
        .and_then(|number| number.parse().ok())
        .with_context(|| format!("“{schema}” is not a Redis database"))
}

/// Run `work` with database `number` selected, and select the session's
/// own database again afterwards.
async fn in_database<T>(
    connection: &mut MultiplexedConnection,
    database: &AtomicI64,
    number: i64,
    work: impl for<'a> FnOnce(
        &'a mut MultiplexedConnection,
    ) -> futures::future::BoxFuture<'a, Result<T>>,
) -> Result<T> {
    let own = database.load(Ordering::Relaxed);
    if number != own {
        redis::cmd("SELECT")
            .arg(number)
            .query_async::<()>(connection)
            .await?;
    }
    let result = work(connection).await;
    if number != own {
        redis::cmd("SELECT")
            .arg(own)
            .query_async::<()>(connection)
            .await?;
    }
    result
}

/// `DATAKIT.VALUE db key [limit [offset]]`: the value of `key`, read the
/// way its type is read.
async fn value(
    connection: &mut MultiplexedConnection,
    database: &AtomicI64,
    words: &[String],
) -> Result<StatementOutcome> {
    let [_, schema, key, rest @ ..] = words else {
        bail!("DATAKIT.VALUE takes a database and a key");
    };
    let limit: isize = rest
        .first()
        .and_then(|word| word.parse().ok())
        .unwrap_or(1000);
    let offset: isize = rest.get(1).and_then(|word| word.parse().ok()).unwrap_or(0);
    let key = key.clone();
    in_database(
        connection,
        database,
        database_number(schema)?,
        move |connection| {
            async move {
                let key_type: String = redis::cmd("TYPE").arg(&key).query_async(connection).await?;
                let last = offset + limit - 1;
                let text = |reply: Reply| scalar(reply);
                Ok(match key_type.as_str() {
                    "string" => {
                        let value: Reply =
                            redis::cmd("GET").arg(&key).query_async(connection).await?;
                        let rows = if offset == 0 {
                            vec![vec![text(value)].into()]
                        } else {
                            Vec::new()
                        };
                        table(&[("value", TypeCategory::Text)], rows)
                    }
                    "list" => {
                        let items: Vec<Reply> = redis::cmd("LRANGE")
                            .arg(&key)
                            .arg(offset)
                            .arg(last)
                            .query_async(connection)
                            .await?;
                        let rows = items
                            .into_iter()
                            .enumerate()
                            .map(|(ix, item)| {
                                vec![Value::Int((offset as usize + ix) as i64), text(item)].into()
                            })
                            .collect();
                        table(
                            &[
                                ("index", TypeCategory::Integer),
                                ("value", TypeCategory::Text),
                            ],
                            rows,
                        )
                    }
                    "hash" => {
                        let pairs: Vec<(Reply, Reply)> = redis::cmd("HGETALL")
                            .arg(&key)
                            .query_async(connection)
                            .await?;
                        let rows = page(pairs, offset, limit)
                            .map(|(field, value)| vec![text(field), text(value)].into())
                            .collect();
                        table(
                            &[("field", TypeCategory::Text), ("value", TypeCategory::Text)],
                            rows,
                        )
                    }
                    "set" => {
                        let mut members: Vec<Reply> = redis::cmd("SMEMBERS")
                            .arg(&key)
                            .query_async(connection)
                            .await?;
                        members.sort_by_key(|member| {
                            text(member.clone()).display().map(|text| text.into_owned())
                        });
                        let rows = page(members, offset, limit)
                            .map(|member| vec![text(member)].into())
                            .collect();
                        table(&[("member", TypeCategory::Text)], rows)
                    }
                    "zset" => {
                        let pairs: Vec<(Reply, f64)> = redis::cmd("ZRANGE")
                            .arg(&key)
                            .arg(offset)
                            .arg(last)
                            .arg("WITHSCORES")
                            .query_async(connection)
                            .await?;
                        let rows = pairs
                            .into_iter()
                            .map(|(member, score)| vec![text(member), Value::Float(score)].into())
                            .collect();
                        table(
                            &[
                                ("member", TypeCategory::Text),
                                ("score", TypeCategory::Float),
                            ],
                            rows,
                        )
                    }
                    "stream" => {
                        let entries: Reply = redis::cmd("XRANGE")
                            .arg(&key)
                            .arg("-")
                            .arg("+")
                            .arg("COUNT")
                            .arg(offset + limit)
                            .query_async(connection)
                            .await?;
                        let words = ["XRANGE".to_string()];
                        match outcome(&words, entries)? {
                            StatementOutcome::Rows(rows) if offset > 0 => {
                                use futures::StreamExt as _;
                                let columns = rows.columns().clone();
                                let rest: Vec<Row> = rows
                                    .skip(offset as usize)
                                    .filter_map(|row| async move { row.ok() })
                                    .collect()
                                    .await;
                                datakit_driver::StatementOutcome::Rows(
                                    datakit_driver::RowStream::new(
                                        columns,
                                        futures::stream::iter(rest.into_iter().map(Ok)).boxed(),
                                    ),
                                )
                            }
                            other => other,
                        }
                    }
                    "none" => table(&[("value", TypeCategory::Text)], Vec::new()),
                    other => bail!("DataKit can’t show a value of type “{other}” yet"),
                })
            }
            .boxed()
        },
    )
    .await
}

/// `DATAKIT.LEN db key`: how many rows `DATAKIT.VALUE` has for `key`.
async fn length(
    connection: &mut MultiplexedConnection,
    database: &AtomicI64,
    words: &[String],
) -> Result<StatementOutcome> {
    let [_, schema, key, ..] = words else {
        bail!("DATAKIT.LEN takes a database and a key");
    };
    let key = key.clone();
    in_database(
        connection,
        database,
        database_number(schema)?,
        move |connection| {
            async move {
                let key_type: String = redis::cmd("TYPE").arg(&key).query_async(connection).await?;
                let count: i64 = match key_type.as_str() {
                    "string" => 1,
                    "none" => 0,
                    kind => {
                        let command = match kind {
                            "list" => "LLEN",
                            "hash" => "HLEN",
                            "set" => "SCARD",
                            "zset" => "ZCARD",
                            "stream" => "XLEN",
                            other => bail!("DataKit can’t count a value of type “{other}”"),
                        };
                        redis::cmd(command)
                            .arg(&key)
                            .query_async(connection)
                            .await?
                    }
                };
                Ok(table(
                    &[("count", TypeCategory::Integer)],
                    vec![vec![Value::Int(count)].into()],
                ))
            }
            .boxed()
        },
    )
    .await
}

/// `items[offset..offset + limit]`, as far as there are items.
fn page<T>(items: Vec<T>, offset: isize, limit: isize) -> impl Iterator<Item = T> {
    items
        .into_iter()
        .skip(offset.max(0) as usize)
        .take(limit.max(0) as usize)
}
