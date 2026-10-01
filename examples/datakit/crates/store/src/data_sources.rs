use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use datakit_driver::ConnectionProfile;
use serde::{Deserialize, Serialize};

/// The data sources file.
///
/// A missing file is an empty list. A file that does not parse is an error
/// and is never overwritten, so a hand edit with a typo is not lost to the
/// next save.
pub struct DataSourceFile {
    path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Contents {
    version: u32,
    data_sources: Vec<ConnectionProfile>,
}

const VERSION: u32 = 1;

impl DataSourceFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Vec<ConnectionProfile>> {
        let json = match std::fs::read_to_string(&self.path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", self.path.display()));
            }
        };
        // Notepad and PowerShell write a byte order mark; it is not JSON.
        let json = json.strip_prefix('\u{feff}').unwrap_or(&json);
        let contents: Contents = serde_json::from_str(json)
            .with_context(|| format!("{} is not a valid data sources file", self.path.display()))?;
        Ok(contents.data_sources)
    }

    /// Replace the file's contents with `data_sources`, atomically.
    pub fn save(&self, data_sources: &[ConnectionProfile]) -> Result<()> {
        let contents = Contents {
            version: VERSION,
            data_sources: data_sources.to_vec(),
        };
        write_atomically(
            &self.path,
            serde_json::to_string_pretty(&contents)?.as_bytes(),
        )
    }
}

/// Write `contents` to a sibling file and rename it over `path`, so a crash
/// leaves either the old file or the new one.
pub(crate) fn write_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let directory = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, contents)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_order_mark_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data-sources.json");
        std::fs::write(&path, "\u{feff}{\"version\": 1, \"data_sources\": []}").unwrap();
        assert!(DataSourceFile::new(path).load().unwrap().is_empty());
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        let directory = tempfile::tempdir().unwrap();
        let file = DataSourceFile::new(directory.path().join("data-sources.json"));
        assert!(file.load().unwrap().is_empty());
    }

    #[test]
    fn data_sources_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let file = DataSourceFile::new(directory.path().join("nested/data-sources.json"));
        let profiles = vec![
            ConnectionProfile::new("postgresql", 5432).with_name("Local"),
            ConnectionProfile::new("postgresql", 6543).with_host("db.internal"),
        ];
        file.save(&profiles).unwrap();
        assert_eq!(file.load().unwrap(), profiles);
    }

    #[test]
    fn an_invalid_file_is_an_error_and_is_left_alone() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data-sources.json");
        std::fs::write(&path, "{ not json").unwrap();
        let file = DataSourceFile::new(&path);
        assert!(file.load().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }
}
