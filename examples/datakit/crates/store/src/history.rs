use std::{path::Path, sync::Arc};

use anyhow::{Context as _, Result};
use datakit_driver::DataSourceId;
use rusqlite::{Connection, OptionalExtension as _, params};

/// How a statement in the history ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryOutcome {
    /// It returned rows; how many were fetched.
    Rows(u64),
    /// It changed rows, or ran a command that does not count them.
    Command(Option<u64>),
    Failed(Arc<str>),
    Cancelled,
}

/// One statement someone ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    id: Option<i64>,
    data_source: DataSourceId,
    sql: Arc<str>,
    started_at_ms: i64,
    duration_ms: u64,
    outcome: HistoryOutcome,
}

impl HistoryEntry {
    /// A statement run against `data_source`, started at `started_at_ms`
    /// milliseconds since the Unix epoch.
    pub fn new(
        data_source: DataSourceId,
        sql: impl Into<Arc<str>>,
        started_at_ms: i64,
        duration_ms: u64,
        outcome: HistoryOutcome,
    ) -> Self {
        Self {
            id: None,
            data_source,
            sql: sql.into(),
            started_at_ms,
            duration_ms,
            outcome,
        }
    }

    /// The entry's id in the history, once recorded.
    pub fn id(&self) -> Option<i64> {
        self.id
    }

    pub fn data_source(&self) -> &DataSourceId {
        &self.data_source
    }

    pub fn sql(&self) -> &Arc<str> {
        &self.sql
    }

    pub fn started_at_ms(&self) -> i64 {
        self.started_at_ms
    }

    pub fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub fn outcome(&self) -> &HistoryOutcome {
        &self.outcome
    }
}

/// Every statement run, newest first, in a SQLite database.
pub struct QueryHistory {
    connection: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS history (
        id INTEGER PRIMARY KEY,
        data_source TEXT NOT NULL,
        sql TEXT NOT NULL,
        started_at_ms INTEGER NOT NULL,
        duration_ms INTEGER NOT NULL,
        status TEXT NOT NULL,
        rows INTEGER,
        error TEXT
    );
    CREATE INDEX IF NOT EXISTS history_by_time ON history (started_at_ms DESC);
";

const COLUMNS: &str = "id, data_source, sql, started_at_ms, duration_ms, status, rows, error";

fn read_entry(row: &rusqlite::Row) -> rusqlite::Result<HistoryEntry> {
    let status: String = row.get(5)?;
    let rows: Option<i64> = row.get(6)?;
    let error: Option<String> = row.get(7)?;
    let outcome = match status.as_str() {
        "rows" => HistoryOutcome::Rows(rows.unwrap_or_default() as u64),
        "command" => HistoryOutcome::Command(rows.map(|rows| rows as u64)),
        "cancelled" => HistoryOutcome::Cancelled,
        _ => HistoryOutcome::Failed(error.unwrap_or_default().into()),
    };
    Ok(HistoryEntry {
        id: Some(row.get(0)?),
        data_source: DataSourceId::from(row.get::<_, String>(1)?.as_str()),
        sql: row.get::<_, String>(2)?.into(),
        started_at_ms: row.get(3)?,
        duration_ms: row.get::<_, i64>(4)? as u64,
        outcome,
    })
}

/// How many entries the history keeps; older ones are dropped on record.
const LIMIT: i64 = 10_000;

impl QueryHistory {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("cannot create {}", directory.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("cannot open the query history at {}", path.display()))?;
        Self::with_connection(connection)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(connection: Connection) -> Result<Self> {
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    /// Add `entry`, returning it with its id.
    pub fn record(&self, entry: HistoryEntry) -> Result<HistoryEntry> {
        let (status, rows, error) = match &entry.outcome {
            HistoryOutcome::Rows(rows) => ("rows", Some(*rows as i64), None),
            HistoryOutcome::Command(rows) => ("command", rows.map(|rows| rows as i64), None),
            HistoryOutcome::Failed(error) => ("failed", None, Some(error.to_string())),
            HistoryOutcome::Cancelled => ("cancelled", None, None),
        };
        self.connection.execute(
            "INSERT INTO history (data_source, sql, started_at_ms, duration_ms, status, rows, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.data_source.as_str(),
                &*entry.sql,
                entry.started_at_ms,
                entry.duration_ms as i64,
                status,
                rows,
                error,
            ],
        )?;
        let id = self.connection.last_insert_rowid();
        self.connection
            .execute("DELETE FROM history WHERE id <= ?1", params![id - LIMIT])?;
        Ok(HistoryEntry {
            id: Some(id),
            ..entry
        })
    }

    /// The newest `limit` entries whose SQL contains `filter` (without
    /// case), for `data_source` when given.
    pub fn recent(
        &self,
        data_source: Option<&DataSourceId>,
        filter: &str,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>> {
        let pattern = format!(
            "%{}%",
            filter
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut statement = self.connection.prepare(&format!(
            "SELECT {COLUMNS} FROM history
             WHERE (?1 IS NULL OR data_source = ?1) AND sql LIKE ?2 ESCAPE '\\'
             ORDER BY id DESC
             LIMIT ?3"
        ))?;
        let rows = statement.query_map(
            params![data_source.map(DataSourceId::as_str), pattern, limit as i64],
            read_entry,
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The entry with `id`, if it is still kept.
    pub fn entry(&self, id: i64) -> Result<Option<HistoryEntry>> {
        Ok(self
            .connection
            .query_row(
                &format!("SELECT {COLUMNS} FROM history WHERE id = ?1"),
                params![id],
                read_entry,
            )
            .optional()?)
    }

    /// Forget every entry, for `data_source` when given.
    pub fn clear(&self, data_source: Option<&DataSourceId>) -> Result<()> {
        self.connection.execute(
            "DELETE FROM history WHERE ?1 IS NULL OR data_source = ?1",
            params![data_source.map(DataSourceId::as_str)],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(data_source: &str, sql: &str, outcome: HistoryOutcome) -> HistoryEntry {
        HistoryEntry::new(
            DataSourceId::from(data_source),
            sql,
            1_700_000_000_000,
            12,
            outcome,
        )
    }

    #[test]
    fn entries_come_back_newest_first_with_their_outcome() {
        let history = QueryHistory::open_in_memory().unwrap();
        history
            .record(entry("a", "select 1", HistoryOutcome::Rows(1)))
            .unwrap();
        history
            .record(entry(
                "a",
                "delete from t",
                HistoryOutcome::Command(Some(3)),
            ))
            .unwrap();
        history
            .record(entry(
                "b",
                "select nope",
                HistoryOutcome::Failed("no".into()),
            ))
            .unwrap();

        let all = history.recent(None, "", 10).unwrap();
        assert_eq!(
            all.iter().map(|e| &**e.sql()).collect::<Vec<_>>(),
            vec!["select nope", "delete from t", "select 1"]
        );
        assert_eq!(all[1].outcome(), &HistoryOutcome::Command(Some(3)));
        assert_eq!(all[0].outcome(), &HistoryOutcome::Failed("no".into()));
    }

    #[test]
    fn entries_filter_by_data_source_and_text() {
        let history = QueryHistory::open_in_memory().unwrap();
        history
            .record(entry("a", "select * from Orders", HistoryOutcome::Rows(0)))
            .unwrap();
        history
            .record(entry("b", "select * from orders", HistoryOutcome::Rows(0)))
            .unwrap();
        history
            .record(entry("a", "select 100%_off", HistoryOutcome::Rows(0)))
            .unwrap();

        let a = DataSourceId::from("a");
        assert_eq!(history.recent(Some(&a), "orders", 10).unwrap().len(), 1);
        assert_eq!(history.recent(None, "ORDERS", 10).unwrap().len(), 2);
        assert_eq!(
            history.recent(None, "%_", 10).unwrap().len(),
            1,
            "wildcards in the filter match themselves"
        );
    }

    #[test]
    fn clearing_one_data_source_keeps_the_others() {
        let history = QueryHistory::open_in_memory().unwrap();
        history
            .record(entry("a", "select 1", HistoryOutcome::Cancelled))
            .unwrap();
        history
            .record(entry("b", "select 2", HistoryOutcome::Rows(1)))
            .unwrap();
        history.clear(Some(&DataSourceId::from("a"))).unwrap();
        let left = history.recent(None, "", 10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].data_source().as_str(), "b");
    }

    #[test]
    fn the_history_survives_reopening() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite3");
        let recorded = QueryHistory::open(&path)
            .unwrap()
            .record(entry("a", "select 1", HistoryOutcome::Rows(1)))
            .unwrap();
        let reopened = QueryHistory::open(&path).unwrap();
        assert_eq!(
            reopened.entry(recorded.id().unwrap()).unwrap(),
            Some(recorded)
        );
    }
}
