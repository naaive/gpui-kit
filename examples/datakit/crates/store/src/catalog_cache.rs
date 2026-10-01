use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use datakit_catalog::Catalog;
use datakit_driver::DataSourceId;
use serde::{Deserialize, Serialize};

use crate::data_sources::write_atomically;

/// The last catalog read from each data source, one JSON file apiece.
///
/// It lets the explorer and completion work the moment DataKit starts,
/// before any connection opens. It is a cache: a file that is missing, from
/// another version or unreadable is simply not there, and the next
/// introspection replaces it.
pub struct CatalogCache {
    directory: PathBuf,
}

#[derive(Serialize)]
struct Written<'a> {
    version: u32,
    catalog: &'a Catalog,
}

#[derive(Deserialize)]
struct Read {
    version: u32,
    catalog: Catalog,
}

/// Bumped whenever [`Catalog`]'s shape changes in a way old files cannot
/// describe.
const VERSION: u32 = 1;

impl CatalogCache {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn path(&self, id: &DataSourceId) -> PathBuf {
        // Ids are generated UUIDs; anything else is reduced to what a file
        // name can safely hold.
        let name: String = id
            .as_str()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.directory.join(format!("{name}.json"))
    }

    /// The cached catalog of `id`, or `None` when there is none to use.
    pub fn load(&self, id: &DataSourceId) -> Option<Catalog> {
        let path = self.path(id);
        let json = std::fs::read(&path).ok()?;
        match serde_json::from_slice::<Read>(&json) {
            Ok(read) if read.version == VERSION => Some(read.catalog),
            Ok(_) => None,
            Err(error) => {
                tracing::info!("ignoring the catalog cache {}: {error}", path.display());
                None
            }
        }
    }

    pub fn save(&self, id: &DataSourceId, catalog: &Catalog) -> Result<()> {
        let json = serde_json::to_vec(&Written {
            version: VERSION,
            catalog,
        })
        .context("cannot write the catalog cache")?;
        write_atomically(&self.path(id), &json)
    }

    pub fn remove(&self, id: &DataSourceId) -> Result<()> {
        match std::fs::remove_file(self.path(id)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).context("cannot remove the catalog cache")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use datakit_catalog::{Column, Relation, RelationType, Schema};

    use super::*;

    #[test]
    fn a_catalog_survives_a_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(directory.path());
        let id = DataSourceId::generate();
        assert!(cache.load(&id).is_none());

        let catalog = Catalog::new("shop")
            .with_schemas([Schema::new("public"), Schema::new("audit")])
            .with_schema(
                Schema::new("public").with_relations([Relation::new("users", RelationType::Table)
                    .with_columns([Column::new("id", "integer").primary_key(true)])]),
            )
            .with_search_path(["public".into()]);
        cache.save(&id, &catalog).unwrap();
        assert_eq!(cache.load(&id), Some(catalog));

        cache.remove(&id).unwrap();
        assert!(cache.load(&id).is_none());
        cache.remove(&id).unwrap();
    }

    #[test]
    fn a_damaged_file_is_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let cache = CatalogCache::new(directory.path());
        let id = DataSourceId::generate();
        std::fs::write(cache.path(&id), "{ not json").unwrap();
        assert!(cache.load(&id).is_none());
    }
}
