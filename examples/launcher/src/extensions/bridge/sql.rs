//! `sql_query`: reading another application's SQLite database, such as an
//! editor's list of recent projects, the way Raycast's `executeSQL` does.

use std::path::Path;

use anyhow::{Context as _, Result};
use rusqlite::{OpenFlags, types::ValueRef};
use serde_json::{Map, Value};

/// Runs `sql` on a copy of the database at `path` and answers each row as
/// an object keyed by column name.
///
/// The application that owns the file may hold it open, locked, with recent
/// writes still in its `-wal` file; a copy of the three files is read
/// instead, so nothing the application does is disturbed or blocked. The
/// copy is kept until the files change, since an editor's database can be
/// hundreds of megabytes.
pub fn query(path: &Path, sql: &str, parameters: &[Value]) -> Result<Vec<Value>> {
    let copy = current_copy(path)?;
    let connection = rusqlite::Connection::open_with_flags(
        &copy,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut statement = connection.prepare(sql)?;
    let columns: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let parameters: Vec<rusqlite::types::Value> = parameters
        .iter()
        .map(|value| match value {
            Value::Null => rusqlite::types::Value::Null,
            Value::Bool(flag) => rusqlite::types::Value::Integer(i64::from(*flag)),
            Value::Number(number) => match number.as_i64() {
                Some(integer) => rusqlite::types::Value::Integer(integer),
                None => rusqlite::types::Value::Real(number.as_f64().unwrap_or_default()),
            },
            Value::String(text) => rusqlite::types::Value::Text(text.clone()),
            other => rusqlite::types::Value::Text(other.to_string()),
        })
        .collect();
    let mut rows = statement.query(rusqlite::params_from_iter(parameters))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut object = Map::new();
        for (ix, column) in columns.iter().enumerate() {
            let value = match row.get_ref(ix)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(integer) => Value::from(integer),
                ValueRef::Real(real) => Value::from(real),
                ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).into_owned()),
                // Editors keep JSON in blobs as often as in text.
                ValueRef::Blob(bytes) => Value::String(String::from_utf8_lossy(bytes).into_owned()),
            };
            object.insert(column.clone(), value);
        }
        result.push(Value::Object(object));
    }
    Ok(result)
}

/// The files a database is made of: itself and its journal.
fn companions(path: &Path) -> [std::path::PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        std::path::PathBuf::from(name)
    };
    [path.to_path_buf(), with("-wal"), with("-shm")]
}

/// What changes when a file does: its length and modification time.
fn stamp(path: &Path) -> String {
    companions(path)
        .iter()
        .map(|file| {
            std::fs::metadata(file)
                .map(|metadata| {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_nanos());
                    format!("{}:{modified}", metadata.len())
                })
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// A copy of the database at `path`, made again only when it changed.
fn current_copy(path: &Path) -> Result<std::path::PathBuf> {
    use sha2::{Digest as _, Sha256};
    let key: String = Sha256::digest(path.to_string_lossy().as_bytes())
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let folder = std::env::temp_dir().join("gpui-kit-launcher-sql").join(key);
    let copy = folder.join("database.sqlite");
    let stamp_file = folder.join("stamp");
    let stamp = stamp(path);
    if std::fs::read_to_string(&stamp_file).is_ok_and(|saved| saved == stamp) && copy.is_file() {
        return Ok(copy);
    }
    std::fs::create_dir_all(&folder)?;
    std::fs::remove_file(&stamp_file).ok();
    let [source, wal, shm] = companions(path);
    let [target, target_wal, target_shm] = companions(&copy);
    std::fs::copy(&source, &target).with_context(|| format!("cannot read {}", path.display()))?;
    for (from, to) in [(wal, target_wal), (shm, target_shm)] {
        if std::fs::copy(&from, &to).is_err() {
            std::fs::remove_file(&to).ok();
        }
    }
    std::fs::write(&stamp_file, stamp)?;
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reads_rows_from_a_copy() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("state.vscdb");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ItemTable (key TEXT, value BLOB);
                 INSERT INTO ItemTable VALUES ('history', CAST('{\"entries\":[]}' AS BLOB));
                 INSERT INTO ItemTable VALUES ('count', 3);",
            )
            .unwrap();
        // Still open, as the editor keeps it.
        let rows = query(
            &path,
            "SELECT key, value FROM ItemTable WHERE key = ?",
            &[Value::from("history")],
        )
        .unwrap();
        assert_eq!(
            rows,
            [serde_json::json!({ "key": "history", "value": "{\"entries\":[]}" })]
        );
        let rows = query(
            &path,
            "SELECT value FROM ItemTable WHERE key = 'count'",
            &[],
        )
        .unwrap();
        assert_eq!(rows, [serde_json::json!({ "value": 3 })]);
        assert!(
            query(&path, "DELETE FROM ItemTable", &[]).is_err(),
            "read-only"
        );

        // A change is read: the copy is made again.
        connection
            .execute("UPDATE ItemTable SET value = 4 WHERE key = 'count'", [])
            .unwrap();
        let rows = query(
            &path,
            "SELECT value FROM ItemTable WHERE key = 'count'",
            &[],
        )
        .unwrap();
        assert_eq!(rows, [serde_json::json!({ "value": 4 })]);
        drop(connection);
    }
}
