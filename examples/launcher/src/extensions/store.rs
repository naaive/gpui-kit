//! The Extension Store: a listing of extensions kept in a GitHub repository
//! (or a folder), installed by downloading their files.
//!
//! A store is a folder holding `index.json` and one folder per extension.
//! The index names every file of every extension with its SHA-256, so an
//! install downloads exactly those files from `raw.githubusercontent.com`,
//! checks each, validates both manifests, and only then moves the extension
//! into place, beside those installed from Git. `launcher store-index <dir>`
//! writes the index.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_shell::plugin::PluginManifest;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    install::{self, InstalledExtension},
    manifest::{CommandMode, LauncherManifest},
    paths::{DataDirectory, read_json, write_json},
};

/// The store this repository publishes, on its default branch.
pub const DEFAULT_SOURCE: &str = "naaive/gpui-kit@main/examples/launcher/store";

const INDEX: &str = "index.json";
const INDEX_VERSION: u32 = 1;
/// Files the launcher or an editor writes beside an extension; never part
/// of what is published.
const GENERATED: &[&str] = &["jsconfig.json", "launcher.schema.json"];

/// Where a store is read from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreSource {
    GitHub {
        owner: String,
        repository: String,
        reference: String,
        /// The store's folder inside the repository, without slashes at
        /// either end; empty for the root.
        folder: String,
    },
    Folder(PathBuf),
}

impl StoreSource {
    /// Reads `owner/repo`, `owner/repo@ref` or `owner/repo@ref/folder`, or an
    /// absolute folder path.
    pub fn parse(source: &str) -> Result<Self> {
        let source = source.trim();
        if Path::new(source).is_absolute() {
            return Ok(Self::Folder(PathBuf::from(source)));
        }
        let (repository, rest) = match source.split_once('@') {
            Some((repository, rest)) => (repository, Some(rest)),
            None => (source, None),
        };
        let Some((owner, name)) = repository.split_once('/') else {
            bail!("`{source}` is not owner/repo@branch/folder or a folder path");
        };
        let valid = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        if !valid(owner) || !valid(name) {
            bail!("`{source}` does not name a GitHub repository");
        }
        let (reference, folder) = match rest {
            None => ("main".to_owned(), String::new()),
            Some(rest) => match rest.split_once('/') {
                Some((reference, folder)) => (reference.to_owned(), folder.to_owned()),
                None => (rest.to_owned(), String::new()),
            },
        };
        if reference.is_empty() || reference.contains("..") {
            bail!("`{source}` names an invalid branch");
        }
        let folder = folder.trim_matches('/').to_owned();
        if folder.split('/').any(|part| part == "..") {
            bail!("`{source}` names a folder outside the repository");
        }
        Ok(Self::GitHub {
            owner: owner.to_owned(),
            repository: name.to_owned(),
            reference,
            folder,
        })
    }

    /// The store the user chose, `LAUNCHER_STORE`, or the default.
    pub fn configured(setting: Option<&str>) -> Result<Self> {
        let source = std::env::var("LAUNCHER_STORE")
            .ok()
            .filter(|source| !source.trim().is_empty())
            .or_else(|| setting.map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_SOURCE.to_owned());
        Self::parse(&source)
    }

    /// Reads a file of the store, by its path inside the store. Blocking.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let path = contained(path)?;
        match self {
            Self::Folder(folder) => {
                let file = folder.join(&path);
                std::fs::read(&file).with_context(|| format!("cannot read {}", file.display()))
            }
            Self::GitHub { .. } => {
                let url = self.raw_url(&path);
                let response = reqwest::blocking::Client::new()
                    .get(&url)
                    .header("User-Agent", "gpui-kit-launcher")
                    .timeout(Duration::from_secs(30))
                    .send()
                    .with_context(|| format!("cannot reach {url}"))?;
                if !response.status().is_success() {
                    bail!("{url} answered {}", response.status());
                }
                Ok(response.bytes()?.to_vec())
            }
        }
    }

    fn raw_url(&self, path: &str) -> String {
        match self {
            Self::GitHub {
                owner,
                repository,
                reference,
                folder,
            } => {
                let prefix = match folder.is_empty() {
                    true => String::new(),
                    false => format!("{folder}/"),
                };
                format!(
                    "https://raw.githubusercontent.com/{owner}/{repository}/{reference}/{prefix}{path}"
                )
            }
            Self::Folder(folder) => folder.join(path).display().to_string(),
        }
    }

    /// Where a person reads an extension's source: its folder on GitHub.
    pub fn page_url(&self, listing: &Listing) -> String {
        match self {
            Self::GitHub {
                owner,
                repository,
                reference,
                folder,
            } => {
                let folder = match folder.is_empty() {
                    true => listing.path.clone(),
                    false => format!("{folder}/{}", listing.path),
                };
                format!("https://github.com/{owner}/{repository}/tree/{reference}/{folder}")
            }
            Self::Folder(folder) => folder.join(&listing.path).display().to_string(),
        }
    }

    /// Downloads and parses `index.json`. Blocking.
    pub fn index(&self) -> Result<StoreIndex> {
        let bytes = self.read(INDEX)?;
        let index: StoreIndex =
            serde_json::from_slice(&bytes).context("the store's index.json is not valid")?;
        if index.version > INDEX_VERSION {
            bail!(
                "the store needs a newer launcher (index version {})",
                index.version
            );
        }
        Ok(index)
    }
}

impl std::fmt::Display for StoreSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitHub {
                owner,
                repository,
                reference,
                folder,
            } => {
                write!(formatter, "{owner}/{repository}@{reference}")?;
                if !folder.is_empty() {
                    write!(formatter, "/{folder}")?;
                }
                Ok(())
            }
            Self::Folder(folder) => write!(formatter, "{}", folder.display()),
        }
    }
}

/// A relative path that stays inside where it is resolved, with `/`.
fn contained(path: &str) -> Result<String> {
    let relative = Path::new(path);
    let ok = !path.is_empty()
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !ok {
        bail!("`{path}` is not a path inside the store");
    }
    Ok(path.replace('\\', "/"))
}

/// `index.json`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StoreIndex {
    pub version: u32,
    pub extensions: Vec<Listing>,
}

/// One extension the store offers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Listing {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub version: String,
    /// A Lucide icon name.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    /// The extension's folder inside the store.
    pub path: String,
    pub commands: Vec<ListedCommand>,
    pub files: Vec<ListedFile>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ListedCommand {
    pub name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// `view`, `no-view` or `menu-bar`.
    pub mode: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ListedFile {
    /// Inside the extension's folder, with `/`.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

impl Listing {
    /// The README, when the extension has one.
    pub fn readme(&self) -> Option<&ListedFile> {
        self.files
            .iter()
            .find(|file| file.path.eq_ignore_ascii_case("README.md"))
    }
}

/// What the launcher recorded about an extension it installed from the
/// store: `<data>/store-installs.json`, by id.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoreRecord {
    pub version: String,
    pub source: String,
}

pub fn records(data: &DataDirectory) -> Result<BTreeMap<String, StoreRecord>> {
    read_json(&records_file(data))
}

fn records_file(data: &DataDirectory) -> PathBuf {
    data.root().join("store-installs.json")
}

/// Forgets an uninstalled extension.
pub fn forget(data: &DataDirectory, id: &str) -> Result<()> {
    let mut records = records(data)?;
    if records.remove(id).is_some() {
        write_json(&records_file(data), &records)?;
    }
    Ok(())
}

/// Downloads `listing`'s files, checks them against the index, and
/// installs the extension, replacing an earlier copy from the store.
/// Blocking.
///
/// An extension with the same id installed from Git, or placed in the
/// extensions folder by hand, is not replaced: it is the user's.
pub fn install(
    data: &DataDirectory,
    source: &StoreSource,
    listing: &Listing,
) -> Result<InstalledExtension> {
    let target = data.extension_dir(&listing.id);
    let mut records = records(data)?;
    if target.exists() && !records.contains_key(&listing.id) {
        bail!(
            "`{}` is already installed from elsewhere; uninstall it first",
            listing.id
        );
    }
    let folder = contained(&listing.path)?;
    let staging = install::Staging::new(data)?;
    for file in &listing.files {
        let relative = contained(&file.path)?;
        let bytes = source.read(&format!("{folder}/{relative}"))?;
        if hex(&Sha256::digest(&bytes)) != file.sha256 {
            bail!("`{relative}` does not match the store's index; try again later");
        }
        let path = staging.path().join(&relative);
        std::fs::create_dir_all(path.parent().expect("inside the staging folder"))?;
        std::fs::write(&path, bytes)?;
    }
    let manifest = install::validate(staging.path())?;
    if manifest.id() != listing.id {
        bail!(
            "the store lists `{}`, but its manifest says `{}`",
            listing.id,
            manifest.id()
        );
    }
    staging.replace(&target)?;
    records.insert(
        listing.id.clone(),
        StoreRecord {
            version: manifest.version().to_owned(),
            source: source.to_string(),
        },
    );
    write_json(&records_file(data), &records)?;
    install::installed(data)?
        .into_iter()
        .find(|extension| extension.id() == listing.id)
        .ok_or_else(|| anyhow!("`{}` was installed but cannot be read", listing.id))
}

/// Reads every extension folder in `store/extensions` and describes it,
/// for `index.json`.
pub fn build_index(store: &Path) -> Result<StoreIndex> {
    let root = store.join("extensions");
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&root)
        .with_context(|| format!("cannot read {}", root.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let mut extensions = Vec::new();
    for folder in folders {
        let shell = PluginManifest::read(&folder)
            .map_err(|error| anyhow!("{}: {error}", folder.display()))?;
        let launcher =
            LauncherManifest::read(&folder).with_context(|| format!("{}", folder.display()))?;
        let mut files = Vec::new();
        collect_files(&folder, &folder, &mut files)?;
        files.sort_by(|a: &ListedFile, b| a.path.cmp(&b.path));
        let path = folder
            .strip_prefix(store)
            .expect("inside the store")
            .to_string_lossy()
            .replace('\\', "/");
        extensions.push(Listing {
            id: shell.id().to_owned(),
            name: shell.name().to_owned(),
            description: launcher.description.clone().unwrap_or_default(),
            author: launcher.author.clone().unwrap_or_default(),
            version: shell.version().to_owned(),
            icon: launcher.icon.clone(),
            categories: launcher.categories.clone(),
            path,
            commands: launcher
                .commands
                .iter()
                .map(|command| ListedCommand {
                    name: command.name.clone(),
                    title: command.title.clone(),
                    subtitle: command.subtitle.clone(),
                    mode: match command.mode {
                        CommandMode::View => "view",
                        CommandMode::NoView => "no-view",
                        CommandMode::MenuBar => "menu-bar",
                    }
                    .to_owned(),
                })
                .collect(),
            files,
        });
    }
    if let Some(duplicate) = extensions
        .iter()
        .enumerate()
        .find(|(ix, listing)| extensions[..*ix].iter().any(|other| other.id == listing.id))
    {
        bail!("two extensions have the id `{}`", duplicate.1.id);
    }
    Ok(StoreIndex {
        version: INDEX_VERSION,
        extensions,
    })
}

/// Writes `store/index.json`; answers how many extensions it lists.
pub fn write_index(store: &Path) -> Result<usize> {
    let index = build_index(store)?;
    let text = serde_json::to_string_pretty(&index)? + "\n";
    std::fs::write(store.join(INDEX), text)?;
    Ok(index.extensions.len())
}

fn collect_files(root: &Path, folder: &Path, files: &mut Vec<ListedFile>) -> Result<()> {
    for entry in std::fs::read_dir(folder)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files)?;
            continue;
        }
        if name.ends_with(".d.ts") || GENERATED.contains(&name.as_str()) {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        files.push(ListedFile {
            path: path
                .strip_prefix(root)
                .expect("inside the extension")
                .to_string_lossy()
                .replace('\\', "/"),
            sha256: hex(&Sha256::digest(&bytes)),
            size: bytes.len() as u64,
        });
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_sources() {
        assert_eq!(
            StoreSource::parse("naaive/gpui-kit@main/examples/launcher/store").unwrap(),
            StoreSource::GitHub {
                owner: "naaive".into(),
                repository: "gpui-kit".into(),
                reference: "main".into(),
                folder: "examples/launcher/store".into(),
            }
        );
        let source = StoreSource::parse("owner/repo").unwrap();
        assert_eq!(source.to_string(), "owner/repo@main");
        assert_eq!(
            source.raw_url("index.json"),
            "https://raw.githubusercontent.com/owner/repo/main/index.json"
        );
        assert!(StoreSource::parse("nonsense").is_err());
        assert!(StoreSource::parse("a/b@main/../x").is_err());
        assert!(contained("../x").is_err());
        assert!(contained("/etc/passwd").is_err());
    }

    /// A store in a folder: its index is built, an extension installed from
    /// it, a tampered file refused, and the bundled store's index is current.
    #[test]
    fn test_installs_from_a_store_folder() {
        let folder = tempfile::tempdir().unwrap();
        let store = folder.path().join("store");
        let extension = store.join("extensions/hello");
        std::fs::create_dir_all(extension.join("commands")).unwrap();
        std::fs::write(
            extension.join("gpui-shell.json"),
            r#"{ "id": "com.example.hello", "name": "Hello", "version": "1.2.0", "entry": "commands/hello.js" }"#,
        )
        .unwrap();
        std::fs::write(
            extension.join("launcher.json"),
            r#"{ "description": "Says hello", "categories": ["Fun"],
                 "commands": [{ "name": "hello", "title": "Say Hello", "module": "commands/hello.js" }] }"#,
        )
        .unwrap();
        std::fs::write(extension.join("commands/hello.js"), "export default 1;").unwrap();
        std::fs::write(extension.join("commands/gpui-kit.d.ts"), "generated").unwrap();
        assert_eq!(write_index(&store).unwrap(), 1);

        let source = StoreSource::Folder(store.clone());
        let index = source.index().unwrap();
        let listing = &index.extensions[0];
        assert_eq!(listing.description, "Says hello");
        assert_eq!(listing.categories, ["Fun"]);
        let paths: Vec<&str> = listing
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(
            paths,
            ["commands/hello.js", "gpui-shell.json", "launcher.json"]
        );

        let data = DataDirectory::new(folder.path().join("data"));
        let installed = install(&data, &source, listing).unwrap();
        assert_eq!(installed.version(), "1.2.0");
        assert_eq!(
            records(&data).unwrap()["com.example.hello"].version,
            "1.2.0"
        );
        assert!(
            data.extension_dir("com.example.hello")
                .join("commands/hello.js")
                .is_file()
        );

        std::fs::write(extension.join("commands/hello.js"), "tampered").unwrap();
        let error = install(&data, &source, listing).unwrap_err();
        assert!(error.to_string().contains("does not match"), "{error}");
        assert!(
            data.extension_dir("com.example.hello")
                .join("commands/hello.js")
                .is_file(),
            "a failed update leaves the installed copy"
        );

        forget(&data, "com.example.hello").unwrap();
        let error = install(&data, &source, listing).unwrap_err();
        assert!(
            error.to_string().contains("installed from elsewhere"),
            "{error}"
        );
    }

    #[test]
    fn test_the_published_index_is_current() {
        let store = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("store");
        let published: StoreIndex =
            serde_json::from_str(&std::fs::read_to_string(store.join(INDEX)).unwrap()).unwrap();
        assert_eq!(
            published,
            build_index(&store).unwrap(),
            "run `launcher store-index examples/launcher/store`"
        );
    }
}
