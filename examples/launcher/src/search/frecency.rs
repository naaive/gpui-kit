//! What the user picks, remembered across launches.
//!
//! Two memories feed the root search. **Frecency** is a use count that fades
//! with a half-life of a week, so an application used daily outranks one used
//! heavily a month ago. The **query memory** maps what was typed to what was
//! chosen, so typing the same letters again puts the same pick first, which is
//! what makes a launcher feel predictable under muscle memory.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// How long a use takes to count half as much.
const HALF_LIFE_SECS: f64 = 7.0 * 24.0 * 60.0 * 60.0;
/// Below this an item is forgotten when the store is written: about seven
/// half-lives after a single use.
const FORGOTTEN: f64 = 0.01;
/// Queries longer than this are remembered by their first this-many
/// characters; nobody types more to reach an item they use.
const REMEMBERED_QUERY_CHARS: usize = 24;
/// The query memory keeps the most recent picks up to this many queries.
const REMEMBERED_QUERIES: usize = 2000;
const VERSION: u32 = 1;

/// Seconds since the Unix epoch; every method takes the time explicitly so
/// decay can be tested without waiting a week.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct UsageFile {
    version: u32,
    #[serde(default)]
    items: BTreeMap<String, Usage>,
    #[serde(default)]
    queries: BTreeMap<String, Pick>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Usage {
    /// The decayed use count as of `used`.
    score: f64,
    used: u64,
}

impl Usage {
    fn at(&self, now: u64) -> f64 {
        let elapsed = now.saturating_sub(self.used) as f64;
        self.score * 0.5_f64.powf(elapsed / HALF_LIFE_SECS)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Pick {
    item: String,
    used: u64,
}

/// Item usage, loaded from and written to one JSON file.
///
/// Reading happens once, when the root search first needs it; writing is
/// split into [`UsageStore::snapshot`], cheap and on the caller's thread, and
/// [`write_snapshot`], which does the disk work and belongs on a background
/// executor.
#[derive(Debug, Default)]
pub struct UsageStore {
    path: Option<PathBuf>,
    file: UsageFile,
}

impl UsageStore {
    /// A store that is never written, for tests and for when there is no data
    /// directory.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Reads the store at `path`. A missing file is an empty store; a damaged
    /// one is logged and replaced on the next write rather than blocking the
    /// launcher.
    pub fn load(path: PathBuf) -> Self {
        let file = match std::fs::read_to_string(&path) {
            Ok(source) => serde_json::from_str(&source).unwrap_or_else(|error| {
                tracing::warn!("ignoring damaged usage file {}: {error}", path.display());
                UsageFile::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => UsageFile::default(),
            Err(error) => {
                tracing::warn!("cannot read usage file {}: {error}", path.display());
                UsageFile::default()
            }
        };
        Self {
            path: Some(path),
            file,
        }
    }

    /// `<data dir>/gpui-kit-launcher/usage.json`, where the platform keeps
    /// application data.
    pub fn default_path() -> Option<PathBuf> {
        dirs::data_dir().map(|dir| dir.join("gpui-kit-launcher").join("usage.json"))
    }

    /// How much `item` has been used, decayed to `now`; 0 when never, or so
    /// long ago that it is forgotten.
    pub fn frecency(&self, item: &str, now: u64) -> f64 {
        self.file
            .items
            .get(item)
            .map(|usage| usage.at(now))
            .filter(|frecency| *frecency >= FORGOTTEN)
            .unwrap_or_default()
    }

    /// Records that `item` was chosen after typing `query`.
    ///
    /// Every prefix of the query remembers the pick too, so after choosing
    /// Slack for `sl`, typing just `s` offers Slack first until something else
    /// is chosen for `s`.
    pub fn record(&mut self, item: &str, query: &str, now: u64) {
        let score = self.frecency(item, now) + 1.0;
        self.file
            .items
            .insert(item.to_owned(), Usage { score, used: now });

        let query = normalize(query);
        for (end, _) in query
            .char_indices()
            .skip(1)
            .chain([(query.len(), ' ')])
            .take(REMEMBERED_QUERY_CHARS)
        {
            let prefix = query[..end].trim_end();
            if !prefix.is_empty() {
                self.file.queries.insert(
                    prefix.to_owned(),
                    Pick {
                        item: item.to_owned(),
                        used: now,
                    },
                );
            }
        }
        if self.file.queries.len() > REMEMBERED_QUERIES {
            let mut picks: Vec<(String, u64)> = self
                .file
                .queries
                .iter()
                .map(|(query, pick)| (query.clone(), pick.used))
                .collect();
            picks.sort_by_key(|(_, used)| std::cmp::Reverse(*used));
            for (query, _) in picks.into_iter().skip(REMEMBERED_QUERIES) {
                self.file.queries.remove(&query);
            }
        }
    }

    /// The item last chosen for `query`, if any.
    pub fn remembered(&self, query: &str) -> Option<&str> {
        let query = normalize(query);
        let key: String = query.chars().take(REMEMBERED_QUERY_CHARS).collect();
        self.file
            .queries
            .get(key.trim_end())
            .map(|pick| pick.item.as_str())
    }

    /// Every used item with its frecency at `now`, most frecent first.
    pub fn most_frecent(&self, now: u64) -> Vec<(&str, f64)> {
        let mut items: Vec<(&str, f64)> = self
            .file
            .items
            .iter()
            .map(|(item, usage)| (item.as_str(), usage.at(now)))
            .filter(|(_, frecency)| *frecency >= FORGOTTEN)
            .collect();
        items.sort_by(|(a_item, a), (b_item, b)| b.total_cmp(a).then(a_item.cmp(b_item)));
        items
    }

    /// The store as JSON, with forgotten items dropped, ready for
    /// [`write_snapshot`]. `None` for an in-memory store.
    pub fn snapshot(&self, now: u64) -> Option<(PathBuf, String)> {
        let path = self.path.clone()?;
        let file = UsageFile {
            version: VERSION,
            items: self
                .file
                .items
                .iter()
                .filter(|(_, usage)| usage.at(now) >= FORGOTTEN)
                .map(|(item, usage)| (item.clone(), *usage))
                .collect(),
            queries: self.file.queries.clone(),
        };
        match serde_json::to_string(&file) {
            Ok(json) => Some((path, json)),
            Err(error) => {
                tracing::error!("cannot serialize usage: {error}");
                None
            }
        }
    }
}

/// Writes a snapshot through a temporary file and a rename, so a crash or a
/// concurrent launch never leaves half a file behind.
pub fn write_snapshot(path: &Path, json: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, json)?;
    std::fs::rename(&temporary, path)
}

/// Case and spacing do not make a different query.
fn normalize(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60;
    const T0: u64 = 1_700_000_000;

    #[test]
    fn test_use_decays_by_half_each_week() {
        let mut store = UsageStore::in_memory();
        store.record("app:code", "", T0);
        assert!((store.frecency("app:code", T0) - 1.0).abs() < 1e-9);
        assert!((store.frecency("app:code", T0 + 7 * DAY) - 0.5).abs() < 1e-9);
        assert!((store.frecency("app:code", T0 + 14 * DAY) - 0.25).abs() < 1e-9);
        assert_eq!(store.frecency("app:never", T0), 0.0);
    }

    #[test]
    fn test_recent_use_outranks_old_heavy_use() {
        let mut store = UsageStore::in_memory();
        for _ in 0..4 {
            store.record("app:old", "", T0);
        }
        store.record("app:new", "", T0 + 28 * DAY);
        store.record("app:new", "", T0 + 28 * DAY);
        let now = T0 + 28 * DAY;
        // Four uses four weeks ago count as a quarter use; two today as two.
        assert!(store.frecency("app:new", now) > store.frecency("app:old", now));
        assert_eq!(
            store
                .most_frecent(now)
                .iter()
                .map(|(item, _)| *item)
                .collect::<Vec<_>>(),
            ["app:new", "app:old"]
        );
    }

    #[test]
    fn test_query_memory_remembers_the_pick_and_its_prefixes() {
        let mut store = UsageStore::in_memory();
        store.record("app:slack", "  SL ", T0);
        assert_eq!(store.remembered("sl"), Some("app:slack"));
        assert_eq!(store.remembered("s"), Some("app:slack"));
        assert_eq!(store.remembered("sla"), None);

        // A later pick for a shorter query wins that query only.
        store.record("app:safari", "s", T0 + 1);
        assert_eq!(store.remembered("s"), Some("app:safari"));
        assert_eq!(store.remembered("sl"), Some("app:slack"));

        // Queries are compared by words, not spacing.
        store.record("app:code", "vs  code", T0 + 2);
        assert_eq!(store.remembered("VS Code"), Some("app:code"));
        assert_eq!(store.remembered("vs "), Some("app:code"));
    }

    #[test]
    fn test_persists_through_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("usage.json");
        let mut store = UsageStore::load(path.clone());
        assert_eq!(
            store.frecency("app:code", T0),
            0.0,
            "a missing file is empty"
        );
        store.record("app:code", "co", T0);
        store.record("app:ancient", "", T0 - 400 * DAY);

        let (target, json) = store.snapshot(T0).unwrap();
        write_snapshot(&target, &json).unwrap();

        let reloaded = UsageStore::load(path.clone());
        assert!((reloaded.frecency("app:code", T0) - 1.0).abs() < 1e-9);
        assert_eq!(reloaded.remembered("co"), Some("app:code"));
        assert_eq!(
            reloaded.most_frecent(T0).len(),
            1,
            "forgotten items are not written"
        );

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(UsageStore::load(path).frecency("app:code", T0), 0.0);
        assert!(UsageStore::in_memory().snapshot(T0).is_none());
    }
}
