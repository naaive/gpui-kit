//! Where the launcher keeps what it knows about extensions.
//!
//! Everything the user decided — permissions, preferences, installed
//! extensions — lives under one data directory owned by the launcher, never
//! inside an extension's own directory: an extension can be replaced by an
//! update or edited by its author, and neither may change what the user
//! approved.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Serialize, de::DeserializeOwned};

/// The directory name under the platform's data directory.
const APPLICATION: &str = "gpui-kit-launcher";

/// The launcher's data directory and the layout inside it.
///
/// ```text
/// <data>/
/// ├── extensions/<id>/          installed extensions (Git checkouts)
/// ├── extension-data/<id>/      an extension's `${dataDir}`
/// │   ├── store.json            its `localStorage`
/// │   └── cache/
/// ├── installed.json            where each installed extension came from
/// ├── permissions.json          what the user allowed each extension
/// ├── preferences.json          non-secret preference values
/// └── secrets.json              password preferences, only without a keychain
/// ```
///
/// An extension's data sits beside its code rather than inside it, so an
/// update, which replaces the checkout, keeps the user's data.
#[derive(Clone, Debug)]
pub struct DataDirectory {
    root: PathBuf,
}

impl DataDirectory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The platform's per-user data directory, such as
    /// `~/Library/Application Support/gpui-kit-launcher` on macOS.
    pub fn platform_default() -> Self {
        let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
        Self::new(base.join(APPLICATION))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where installed extensions live; a catalog root.
    pub fn extensions_dir(&self) -> PathBuf {
        self.root.join("extensions")
    }

    pub fn extension_dir(&self, id: &str) -> PathBuf {
        self.extensions_dir().join(id)
    }

    /// An extension's `${dataDir}`: its storage, cache and any files it writes.
    pub fn extension_data_dir(&self, id: &str) -> PathBuf {
        self.root.join("extension-data").join(id)
    }

    /// The file behind an extension's `localStorage`, named as GPUI Shell's
    /// plugin manager names it.
    pub fn storage_path(&self, id: &str) -> PathBuf {
        self.extension_data_dir(id).join("store.json")
    }

    pub fn cache_dir(&self, id: &str) -> PathBuf {
        self.extension_data_dir(id).join("cache")
    }

    pub fn installs_file(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    pub fn permissions_file(&self) -> PathBuf {
        self.root.join("permissions.json")
    }

    pub fn preferences_file(&self) -> PathBuf {
        self.root.join("preferences.json")
    }

    pub fn secrets_file(&self) -> PathBuf {
        self.root.join("secrets.json")
    }
}

/// Reads a JSON file, treating a missing file as the empty value.
///
/// A malformed file is an error rather than an empty value: silently starting
/// over would discard what the user decided.
pub(super) fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read_to_string(path) {
        Ok(source) => {
            serde_json::from_str(&source).with_context(|| format!("invalid {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Writes a JSON file through a temporary file renamed over the target, so a
/// crash mid-write leaves the previous decisions intact.
pub(super) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_file(path, serde_json::to_string_pretty(value)?.as_bytes(), false)
}

/// Writes `contents` atomically; `private` restricts the file to its owner.
pub(super) fn write_file(path: &Path, contents: &[u8], private: bool) -> Result<()> {
    let directory = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("cannot create {}", directory.display()))?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, contents)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    if private {
        restrict_to_owner(&temporary)?;
    }
    std::fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot restrict {}", path.display()))
}

/// Windows has no mode bits; a file in the user's profile is already readable
/// by that user only.
#[cfg(not(unix))]
fn restrict_to_owner(_: &Path) -> Result<()> {
    Ok(())
}
