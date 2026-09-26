//! The per-extension cache behind `cache_get` and friends.
//!
//! One JSON object per extension, in a file the launcher placed. It exists so
//! a command can show last time's results instantly while it fetches fresh
//! ones, which is why it is capped: it is a cache, not a database, and an
//! extension that outgrows it should be told so rather than silently filling
//! the disk.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub const FILE: &str = "cache.json";
/// Large enough for a few thousand search results, small enough that reading
/// the whole file on first use is instant.
pub const DEFAULT_LIMIT: usize = 10 * 1024 * 1024;
/// Keys are names, not data; a longer one is almost certainly a mistake.
const MAX_KEY_LENGTH: usize = 256;

pub struct Cache {
    path: PathBuf,
    limit: usize,
    /// Read on first use rather than at launch, because most commands never
    /// touch their cache.
    entries: Option<Map<String, Value>>,
}

impl Cache {
    pub fn new(directory: &Path, limit: usize) -> Self {
        Self {
            path: directory.join(FILE),
            limit,
            entries: None,
        }
    }

    pub fn get(&mut self, key: &str) -> Result<Option<Value>, String> {
        check_key(key)?;
        Ok(self.entries()?.get(key).cloned())
    }

    /// Stores `value` under `key`, refusing a write that would take the file
    /// over the limit. The check happens before anything is written, so a
    /// refused write leaves the cache as it was.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        check_key(key)?;
        let mut entries = self.entries()?.clone();
        entries.insert(key.to_owned(), value);
        let serialized = serialize(&entries)?;
        if serialized.len() > self.limit {
            return Err(format!(
                "cache_set(\"{key}\") would grow the cache to {} bytes, above its limit of {} \
                 bytes; remove entries with cache_remove or store less",
                serialized.len(),
                self.limit
            ));
        }
        self.write(&serialized)?;
        self.entries = Some(entries);
        Ok(())
    }

    /// Removes `key`, answering whether it was there.
    pub fn remove(&mut self, key: &str) -> Result<bool, String> {
        check_key(key)?;
        let mut entries = self.entries()?.clone();
        if entries.remove(key).is_none() {
            return Ok(false);
        }
        self.write(&serialize(&entries)?)?;
        self.entries = Some(entries);
        Ok(true)
    }

    pub fn clear(&mut self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("cannot clear {}: {error}", self.path.display()));
            }
        }
        self.entries = Some(Map::new());
        Ok(())
    }

    /// A missing file is an empty cache; a malformed one is an error, so a
    /// corrupted cache is reported once rather than overwritten unseen.
    fn entries(&mut self) -> Result<&Map<String, Value>, String> {
        if self.entries.is_none() {
            let entries = match std::fs::read_to_string(&self.path) {
                Ok(source) => {
                    serde_json::from_str::<Map<String, Value>>(&source).map_err(|error| {
                        format!(
                            "the cache in {} is not a JSON object ({error}); call cache_clear() \
                             to start over",
                            self.path.display()
                        )
                    })?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
                Err(error) => {
                    return Err(format!("cannot read {}: {error}", self.path.display()));
                }
            };
            self.entries = Some(entries);
        }
        Ok(self.entries.as_ref().expect("just loaded"))
    }

    /// Writes through a temporary file renamed over the target, so a crash
    /// mid-write leaves the previous cache intact.
    fn write(&self, serialized: &str) -> Result<(), String> {
        let fail = |error: std::io::Error| format!("cannot write {}: {error}", self.path.display());
        if let Some(directory) = self.path.parent() {
            std::fs::create_dir_all(directory).map_err(fail)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, serialized).map_err(fail)?;
        std::fs::rename(&temporary, &self.path).map_err(fail)
    }
}

fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("a cache key must not be empty".into());
    }
    if key.len() > MAX_KEY_LENGTH {
        return Err(format!(
            "a cache key must be at most {MAX_KEY_LENGTH} bytes, not {}",
            key.len()
        ));
    }
    Ok(())
}

fn serialize(entries: &Map<String, Value>) -> Result<String, String> {
    serde_json::to_string(entries).map_err(|error| format!("cannot encode the cache: {error}"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("launcher-cache-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&directory).ok();
        directory
    }

    #[test]
    fn test_cache_round_trips_through_its_file() {
        let root = directory("round-trip");
        let mut cache = Cache::new(&root, DEFAULT_LIMIT);
        assert_eq!(cache.get("repos").unwrap(), None);
        cache.set("repos", json!([{ "name": "gpui-kit" }])).unwrap();

        let mut reopened = Cache::new(&root, DEFAULT_LIMIT);
        assert_eq!(
            reopened.get("repos").unwrap(),
            Some(json!([{ "name": "gpui-kit" }]))
        );
        assert!(reopened.remove("repos").unwrap());
        assert!(!reopened.remove("repos").unwrap());
        reopened.set("a", json!(1)).unwrap();
        reopened.clear().unwrap();
        assert_eq!(Cache::new(&root, DEFAULT_LIMIT).get("a").unwrap(), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_cache_refuses_a_write_over_its_limit() {
        let root = directory("limit");
        let mut cache = Cache::new(&root, 64);
        cache.set("small", json!("fits")).unwrap();
        let error = cache.set("large", json!("x".repeat(100))).unwrap_err();
        assert!(error.contains("above its limit of 64 bytes"), "{error}");
        assert_eq!(
            cache.get("large").unwrap(),
            None,
            "a refused write leaves the cache as it was"
        );
        assert_eq!(cache.get("small").unwrap(), Some(json!("fits")));
        assert!(cache.get("").unwrap_err().contains("must not be empty"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn test_cache_reports_a_malformed_file() {
        let root = directory("malformed");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(FILE), "[1, 2]").unwrap();
        let error = Cache::new(&root, DEFAULT_LIMIT).get("a").unwrap_err();
        assert!(error.contains("not a JSON object"), "{error}");
        std::fs::remove_dir_all(&root).ok();
    }
}
