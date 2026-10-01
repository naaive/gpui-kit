use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use anyhow::Result;
use datakit_catalog::Schema;
use datakit_driver::{
    BoxFuture, ColumnInfo, CommandSummary, Connection, Row, RowStream, StatementOutcome,
    TypeCategory, Value,
};
use futures::{FutureExt as _, Stream, StreamExt as _};
use tokio_postgres::{CancelToken, Client, SimpleQueryMessage, SimpleQueryStream, types::Type};

use crate::{error::convert, introspect, tls::Tls, types};

/// One PostgreSQL session.
pub(crate) struct PostgresConnection {
    client: Arc<Client>,
    tls: Tls,
    server_version: Arc<str>,
}

impl PostgresConnection {
    pub(crate) async fn connect(config: tokio_postgres::Config, tls: Tls) -> Result<Self> {
        let client = match &tls {
            Tls::None => {
                let (client, connection) = config.connect(tokio_postgres::NoTls).await?;
                tokio::spawn(async move {
                    if let Err(error) = connection.await {
                        tracing::debug!("PostgreSQL connection ended: {error}");
                    }
                });
                client
            }
            Tls::Rustls(connector) => {
                let (client, connection) = config.connect(connector.clone()).await?;
                tokio::spawn(async move {
                    if let Err(error) = connection.await {
                        tracing::debug!("PostgreSQL connection ended: {error}");
                    }
                });
                client
            }
        };
        let version: String = client
            .query_one("SELECT current_setting('server_version')", &[])
            .await?
            .get(0);
        Ok(Self {
            client: Arc::new(client),
            tls,
            server_version: format!("PostgreSQL {version}").into(),
        })
    }
}

impl Connection for PostgresConnection {
    fn server_version(&self) -> Arc<str> {
        self.server_version.clone()
    }

    fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        let client = self.client.clone();
        let tls = self.tls.clone();
        async move {
            // Preparing tells the column types without running anything. A
            // statement that cannot be prepared (it may be invalid, or a
            // utility command) still runs; its columns are then untyped.
            let types: Option<Vec<Type>> = client.prepare(&sql).await.ok().map(|statement| {
                statement
                    .columns()
                    .iter()
                    .map(|column| column.type_().clone())
                    .collect()
            });

            let stream = client
                .simple_query_raw(&sql)
                .await
                .map_err(|error| convert(error, &sql))?;
            let mut stream = Box::pin(stream);
            loop {
                match stream.next().await {
                    Some(Ok(SimpleQueryMessage::RowDescription(columns))) => {
                        let columns: Vec<ColumnInfo> = columns
                            .iter()
                            .enumerate()
                            .map(
                                |(ix, column)| match types.as_ref().and_then(|t| t.get(ix)) {
                                    Some(ty) => ColumnInfo::new(
                                        column.name(),
                                        types::display_name(ty),
                                        types::category(ty),
                                    ),
                                    None => ColumnInfo::new(column.name(), "", TypeCategory::Text),
                                },
                            )
                            .collect();
                        let categories = columns.iter().map(ColumnInfo::category).collect();
                        let rows = Rows {
                            stream,
                            categories,
                            sql: sql.clone(),
                            cancel: Some((client.cancel_token(), tls)),
                        };
                        return Ok(StatementOutcome::Rows(RowStream::new(
                            columns,
                            rows.boxed(),
                        )));
                    }
                    Some(Ok(SimpleQueryMessage::CommandComplete(rows))) => {
                        let tag = command_tag(&sql);
                        let counted = counts_rows(&tag);
                        return Ok(StatementOutcome::Command(CommandSummary::new(
                            tag,
                            counted.then_some(rows),
                        )));
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(error)) => return Err(convert(error, &sql)),
                    // An empty statement: only a comment, say.
                    None => return Ok(StatementOutcome::Command(CommandSummary::new("", None))),
                }
            }
        }
        .boxed()
    }

    fn cancel(&self) -> BoxFuture<()> {
        let token = self.client.cancel_token();
        let tls = self.tls.clone();
        async move { tls.cancel(&token).await }.boxed()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        introspect::schemas(self.client.clone()).boxed()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        introspect::schema(self.client.clone(), schema).boxed()
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        introspect::search_path(self.client.clone()).boxed()
    }
}

/// The rows after a row description, until the command completes.
///
/// Dropped before the end, it asks the server to cancel the statement:
/// otherwise the session would go on receiving, and discarding, every
/// remaining row before it could run anything else.
struct Rows {
    stream: Pin<Box<SimpleQueryStream>>,
    categories: Vec<TypeCategory>,
    sql: Arc<str>,
    cancel: Option<(CancelToken, Tls)>,
}

impl Stream for Rows {
    type Item = Result<Row>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.cancel.is_none() {
                return Poll::Ready(None);
            }
            let message = match self.stream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(message) => message,
            };
            match message {
                Some(Ok(SimpleQueryMessage::Row(row))) => {
                    let values: Row = self
                        .categories
                        .iter()
                        .enumerate()
                        .map(|(ix, category)| match row.get(ix) {
                            Some(text) => Value::from_text(text, *category),
                            None => Value::Null,
                        })
                        .collect();
                    return Poll::Ready(Some(Ok(values)));
                }
                Some(Ok(SimpleQueryMessage::CommandComplete(_))) | None => {
                    self.cancel = None;
                    return Poll::Ready(None);
                }
                Some(Ok(_)) => continue,
                Some(Err(error)) => {
                    self.cancel = None;
                    let error = convert(error, &self.sql);
                    return Poll::Ready(Some(Err(error)));
                }
            }
        }
    }
}

impl Drop for Rows {
    fn drop(&mut self) {
        let Some((token, tls)) = self.cancel.take() else {
            return;
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    if let Err(error) = tls.cancel(&token).await {
                        tracing::debug!("couldn’t cancel an abandoned statement: {error}");
                    }
                });
            }
            Err(_) => tracing::debug!("an abandoned statement runs on: no runtime to cancel it"),
        }
    }
}

/// The command a statement runs, as PostgreSQL names it in its command tag:
/// `INSERT`, `CREATE TABLE`, `SELECT`.
fn command_tag(sql: &str) -> String {
    let words: Vec<String> = leading_words(sql).take(5).collect();
    let Some(first) = words.first() else {
        return String::new();
    };
    match first.as_str() {
        "CREATE" | "DROP" | "ALTER" => {
            // `CREATE OR REPLACE VIEW` and `CREATE UNIQUE INDEX` name the
            // object after the modifier.
            let object = words.iter().skip(1).find(|word| {
                !matches!(
                    word.as_str(),
                    "OR" | "REPLACE" | "UNIQUE" | "TEMP" | "TEMPORARY"
                )
            });
            match object {
                Some(object) => format!("{first} {object}"),
                None => first.clone(),
            }
        }
        _ => first.clone(),
    }
}

/// The leading words of `sql`, upper-cased, skipping comments.
fn leading_words(sql: &str) -> impl Iterator<Item = String> + '_ {
    let mut rest = sql;
    std::iter::from_fn(move || {
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix("--") {
                rest = after.split_once('\n').map_or("", |(_, tail)| tail);
            } else if let Some(after) = rest.strip_prefix("/*") {
                rest = after.split_once("*/").map_or("", |(_, tail)| tail);
            } else {
                break;
            }
        }
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let word = rest[..end].to_uppercase();
        rest = &rest[end..];
        Some(word)
    })
}

/// Whether the command's row count means something.
fn counts_rows(tag: &str) -> bool {
    matches!(
        tag,
        "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "COPY" | "MOVE" | "FETCH" | "SELECT"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tags_name_the_command_and_its_object() {
        assert_eq!(command_tag("insert into t values (1)"), "INSERT");
        assert_eq!(
            command_tag("-- note\n/* x */ create or replace view v as select 1"),
            "CREATE VIEW"
        );
        assert_eq!(
            command_tag("CREATE UNIQUE INDEX i ON t (a)"),
            "CREATE INDEX"
        );
        assert_eq!(command_tag("drop table t"), "DROP TABLE");
        assert_eq!(command_tag("  "), "");
    }

    #[test]
    fn only_data_commands_count_rows() {
        assert!(counts_rows("DELETE"));
        assert!(!counts_rows("CREATE TABLE"));
    }
}
