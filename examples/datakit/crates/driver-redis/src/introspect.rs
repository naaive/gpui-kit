//! What a Redis server holds: its databases, their keys, its users.

use std::sync::Arc;

use anyhow::{Context as _, Result};
use datakit_catalog::{Relation, RelationType, Role, Schema};
use redis::aio::MultiplexedConnection;

use crate::{KEY_LIMIT, command::quote, reply::column};

/// The databases that hold keys, and the session's own, as `db0` upward;
/// each says how many keys it has.
pub(crate) async fn databases(
    connection: &mut MultiplexedConnection,
    own: i64,
) -> Result<Vec<Schema>> {
    let keyspace: String = redis::cmd("INFO")
        .arg("keyspace")
        .query_async(connection)
        .await
        .context("Couldn’t read the databases")?;
    // `db0:keys=3,expires=0,avg_ttl=0`
    let mut found: Vec<(i64, i64)> = keyspace
        .lines()
        .filter_map(|line| {
            let (name, stats) = line.trim().split_once(':')?;
            let number = name.strip_prefix("db")?.parse().ok()?;
            let keys = stats
                .split(',')
                .find_map(|stat| stat.strip_prefix("keys="))?
                .parse()
                .ok()?;
            Some((number, keys))
        })
        .collect();
    if !found.iter().any(|(number, _)| *number == own) {
        found.push((own, 0));
    }
    found.sort();
    Ok(found
        .into_iter()
        .map(|(number, keys)| {
            Schema::new(format!("db{number}")).with_comment(format!("{keys} keys"))
        })
        .collect())
}

/// Up to [`KEY_LIMIT`] keys of the selected database, named `schema`, each
/// a relation whose columns are those its value is shown with.
pub(crate) async fn keys(
    connection: &mut MultiplexedConnection,
    schema: Arc<str>,
) -> Result<Schema> {
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        let (next, batch): (u64, Vec<Vec<u8>>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("COUNT")
            .arg(500)
            .query_async(connection)
            .await
            .with_context(|| format!("Couldn’t read the keys of {schema}"))?;
        names.extend(batch);
        cursor = next;
        if cursor == 0 || names.len() >= KEY_LIMIT {
            break;
        }
    }
    names.truncate(KEY_LIMIT);
    names.sort();
    names.dedup();
    let mut pipe = redis::pipe();
    for name in &names {
        pipe.cmd("TYPE").arg(name.as_slice());
    }
    let types: Vec<String> = if names.is_empty() {
        Vec::new()
    } else {
        pipe.query_async(connection).await?
    };
    let relations = names.iter().zip(types).map(|(name, key_type)| {
        let columns = match key_type.as_str() {
            "list" => vec![column("index", "integer"), column("value", "string")],
            "hash" => vec![column("field", "string"), column("value", "string")],
            "set" => vec![column("member", "string")],
            "zset" => vec![column("member", "string"), column("score", "double")],
            "stream" => vec![column("id", "string"), column("fields", "string")],
            _ => vec![column("value", "string")],
        };
        Relation::new(crate::reply::bytes_text(name), RelationType::Key)
            .with_comment(key_type)
            .with_columns(columns)
    });
    Ok(Schema::new(schema).with_relations(relations.collect::<Vec<_>>()))
}

/// The server's users, from its access control list (Redis 6 and later).
pub(crate) async fn users(connection: &mut MultiplexedConnection) -> Result<Vec<Role>> {
    let lines: Vec<String> = redis::cmd("ACL")
        .arg("LIST")
        .query_async(connection)
        .await
        .context("Couldn’t read the users")?;
    // `user default on nopass ~* &* +@all`
    Ok(lines
        .iter()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let _ = words.next().filter(|word| *word == "user")?;
            let name = words.next()?;
            let rules: Vec<&str> = words.collect();
            let enabled = rules.contains(&"on");
            Some(
                Role::new(name, enabled)
                    .with_attributes(rules.iter().map(|rule| Arc::from(*rule)))
                    .with_definition(format!("ACL SETUSER {} {}", quote(name), rules.join(" "))),
            )
        })
        .collect())
}
