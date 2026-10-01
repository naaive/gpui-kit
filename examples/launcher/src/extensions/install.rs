//! Installing, updating and removing extensions from Git.
//!
//! An installed extension is a Git checkout in `<data>/extensions/<id>`, a
//! catalog root like any other. Every change is prepared beside the installed
//! copy, validated with both manifests, and only then moved into place, so a
//! failed clone, a broken manifest or a lost connection leaves the working
//! installation untouched.
//!
//! These functions block on `git` and belong on a background thread, except
//! [`uninstall`], which only removes local files.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_shell::plugin::PluginManifest;
use serde::{Deserialize, Serialize};

use super::{
    Catalog,
    manifest::LauncherManifest,
    paths::{DataDirectory, read_json, write_json},
    permissions::PermissionStore,
    preferences::{PreferenceStore, all_scopes},
};

/// How long one `git` command may take before it is stopped.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Where an extension is installed from: a repository and, optionally, the
/// branch, tag or commit to follow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitSource {
    url: String,
    #[serde(default)]
    reference: Option<String>,
}

impl GitSource {
    /// Reads `owner/repo`, a Git URL or a local path, each with an optional
    /// `#ref`. `owner/repo` means a GitHub repository, as in `gpui-shell.json`
    /// dependencies.
    pub fn parse(source: &str) -> Result<Self> {
        let source = source.trim();
        let (location, reference) = match source.rsplit_once('#') {
            Some((location, reference)) if !reference.is_empty() => {
                (location, Some(reference.to_owned()))
            }
            Some((location, _)) => (location, None),
            None => (source, None),
        };
        let url = if is_shorthand(location) {
            format!("https://github.com/{location}.git")
        } else if location.contains("://")
            || location.starts_with("git@")
            || Path::new(location).is_absolute()
        {
            location.to_owned()
        } else {
            bail!("`{source}` is not a repository; use owner/repo, a Git URL, or an absolute path");
        };
        if reference
            .as_deref()
            .is_some_and(|reference| reference.starts_with('-'))
        {
            bail!("`{source}` names an invalid ref");
        }
        Ok(Self { url, reference })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }
}

impl std::fmt::Display for GitSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reference {
            Some(reference) => write!(formatter, "{}#{reference}", self.url),
            None => formatter.write_str(&self.url),
        }
    }
}

fn is_shorthand(location: &str) -> bool {
    let mut parts = location.split('/');
    let valid = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
    };
    valid(parts.next()) && valid(parts.next()) && parts.next().is_none()
}

/// An extension in the launcher's extension directory.
#[derive(Clone, Debug)]
pub struct InstalledExtension {
    id: String,
    name: String,
    version: String,
    directory: PathBuf,
    source: Option<GitSource>,
}

impl InstalledExtension {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Where it was installed from; `None` for a copy placed there by hand.
    pub fn source(&self) -> Option<&GitSource> {
        self.source.as_ref()
    }
}

/// `<data>/installed.json`: the source of each extension installed from Git.
fn records(data: &DataDirectory) -> Result<BTreeMap<String, GitSource>> {
    read_json(&data.installs_file())
}

/// Every extension in the extension directory, sorted by name.
pub fn installed(data: &DataDirectory) -> Result<Vec<InstalledExtension>> {
    let mut records = records(data)?;
    let Ok(entries) = std::fs::read_dir(data.extensions_dir()) else {
        return Ok(Vec::new());
    };
    let mut extensions: Vec<InstalledExtension> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .filter_map(|directory| {
            let manifest = PluginManifest::read(&directory).ok()?;
            Some(InstalledExtension {
                id: manifest.id().to_owned(),
                name: manifest.name().to_owned(),
                version: manifest.version().to_owned(),
                source: records.remove(manifest.id()),
                directory,
            })
        })
        .collect();
    extensions.sort_by_key(|extension| extension.name.to_lowercase());
    Ok(extensions)
}

/// Clones `source` and installs the extension it contains.
///
/// Installing the same repository again replaces the installed copy; an
/// installed extension with the same id from anywhere else is refused, so a
/// repository cannot take over another extension's permissions and data by
/// claiming its id.
pub fn install(data: &DataDirectory, source: &str) -> Result<InstalledExtension> {
    let source = GitSource::parse(source)?;
    let staging = Staging::new(data)?;
    git(
        [
            OsStr::new("clone"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
            OsStr::new(source.url()),
            staging.path().as_os_str(),
        ],
        None,
    )
    .with_context(|| format!("cannot clone {}", source.url()))?;
    if let Some(reference) = source.reference() {
        git(["checkout", "--quiet", reference], Some(staging.path()))
            .with_context(|| format!("{} has no ref `{reference}`", source.url()))?;
    }
    let manifest = validate(staging.path())?;
    let id = manifest.id().to_owned();

    let mut records = records(data)?;
    let target = data.extension_dir(&id);
    if target.exists()
        && records
            .get(&id)
            .is_none_or(|installed| installed.url != source.url)
    {
        let from = records
            .get(&id)
            .map(|installed| format!(" from {}", installed.url))
            .unwrap_or_default();
        bail!(
            "another extension with the id `{id}` is already installed{from}; uninstall it first"
        );
    }
    staging.replace(&target)?;
    records.insert(id.clone(), source.clone());
    write_json(&data.installs_file(), &records)?;
    Ok(InstalledExtension {
        id,
        name: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        directory: target,
        source: Some(source),
    })
}

/// Brings an installed extension up to date with the ref it follows.
///
/// The installed checkout is cloned locally (sharing its objects), fetched and
/// reset there, so only what changed is downloaded and the installed copy is
/// replaced in one step. A grown capability request is not granted by the
/// update: the permission check before the next run asks about it.
pub fn update(data: &DataDirectory, id: &str) -> Result<InstalledExtension> {
    let records = records(data)?;
    let source = records
        .get(id)
        .cloned()
        .ok_or_else(|| anyhow!("`{id}` was not installed from Git, so it cannot be updated"))?;
    let target = data.extension_dir(id);
    let staging = Staging::new(data)?;
    git(
        [
            OsStr::new("clone"),
            OsStr::new("--quiet"),
            OsStr::new("--"),
            target.as_os_str(),
            staging.path().as_os_str(),
        ],
        None,
    )
    .with_context(|| format!("cannot copy {}", target.display()))?;
    let at = Some(staging.path());
    git(["remote", "set-url", "origin", source.url()], at)?;
    git(["fetch", "--quiet", "--tags", "--force", "origin"], at)
        .with_context(|| format!("cannot fetch {}", source.url()))?;
    match source.reference() {
        Some(reference) => {
            let branch = format!("refs/remotes/origin/{reference}");
            if git(["rev-parse", "--verify", "--quiet", &branch], at).is_ok() {
                git(["checkout", "--quiet", "-B", reference, &branch], at)?;
            } else {
                git(["checkout", "--quiet", "--detach", reference], at)
                    .with_context(|| format!("{} has no ref `{reference}`", source.url()))?;
            }
        }
        None => {
            git(["remote", "set-head", "origin", "--auto"], at)?;
            git(["reset", "--quiet", "--hard", "origin/HEAD"], at)?;
        }
    }
    let manifest = validate(staging.path())?;
    if manifest.id() != id {
        bail!(
            "the update changes the extension's id from `{id}` to `{}`; install it separately",
            manifest.id()
        );
    }
    staging.replace(&target)?;
    Ok(InstalledExtension {
        id: id.to_owned(),
        name: manifest.name().to_owned(),
        version: manifest.version().to_owned(),
        directory: target,
        source: Some(source),
    })
}

/// Removes an installed extension and everything the launcher kept for it:
/// its permissions, its preferences and keychain secrets, and its data.
pub fn uninstall(
    data: &DataDirectory,
    id: &str,
    permissions: &PermissionStore,
    preferences: &PreferenceStore,
) -> Result<()> {
    let directory = data.extension_dir(id);
    if !directory.is_dir() {
        bail!(
            "`{id}` is not installed in {}",
            data.extensions_dir().display()
        );
    }
    // Read before removing: the declarations say which secrets exist.
    let catalog = Catalog::discover(&[data.extensions_dir()]);
    preferences.forget(id, &all_scopes(&catalog, id))?;
    permissions.forget(id)?;
    std::fs::remove_dir_all(&directory)
        .with_context(|| format!("cannot remove {}", directory.display()))?;
    let data_dir = data.extension_data_dir(id);
    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)
            .with_context(|| format!("cannot remove {}", data_dir.display()))?;
    }
    let mut records = records(data)?;
    if records.remove(id).is_some() {
        write_json(&data.installs_file(), &records)?;
    }
    super::store::forget(data, id)
}

/// Both manifests must be valid before an extension replaces anything.
pub(super) fn validate(directory: &Path) -> Result<PluginManifest> {
    let manifest = PluginManifest::read(directory).map_err(|error| anyhow!("{error}"))?;
    LauncherManifest::read(directory)?;
    Ok(manifest)
}

/// A directory to prepare a checkout in, removed unless it was moved into
/// place.
///
/// It lives in `<data>/staging` rather than beside the installed extensions,
/// so a catalog scan never sees a half-cloned extension, and on the same file
/// system, so moving it into place is a rename.
pub(super) struct Staging {
    path: PathBuf,
}

impl Staging {
    pub(super) fn new(data: &DataDirectory) -> Result<Self> {
        let parent = data.root().join("staging");
        std::fs::create_dir_all(&parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        std::fs::create_dir_all(data.extensions_dir())
            .with_context(|| format!("cannot create {}", data.extensions_dir().display()))?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        Ok(Self {
            path: parent.join(format!("{}-{nanos}", std::process::id())),
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Moves the checkout to `target`, replacing what is there.
    ///
    /// The old copy is moved aside first and restored if the move fails, so
    /// there is never a moment without a working installation to go back to.
    pub(super) fn replace(self, target: &Path) -> Result<()> {
        let aside = self.path.with_extension("previous");
        let had_previous = target.exists();
        if had_previous {
            std::fs::rename(target, &aside)
                .with_context(|| format!("cannot move {} aside", target.display()))?;
        }
        if let Err(error) = std::fs::rename(&self.path, target) {
            if had_previous {
                std::fs::rename(&aside, target).ok();
            }
            return Err(error).with_context(|| format!("cannot move into {}", target.display()));
        }
        if had_previous {
            std::fs::remove_dir_all(&aside).ok();
        }
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.path.exists() {
            std::fs::remove_dir_all(&self.path).ok();
        }
    }
}

/// Runs `git` non-interactively: no terminal prompt for credentials, no
/// input, and a time limit, because nobody is there to answer.
fn git<I, S>(arguments: I, directory: Option<&Path>) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let arguments: Vec<_> = arguments
        .into_iter()
        .map(|argument| argument.as_ref().to_owned())
        .collect();
    let mut command = Command::new("git");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let mut child = command
        .args(&arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("cannot run git; is it installed?")?;

    // Drained on threads so a chatty command cannot fill a pipe and stall
    // while this thread waits for it to exit.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).ok();
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        stderr.read_to_string(&mut text).ok();
        text
    });

    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            bail!(
                "git {} did not finish within {} seconds",
                display(&arguments),
                GIT_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() {
        let message = stderr.trim();
        bail!(
            "git {} failed{}",
            display(&arguments),
            if message.is_empty() {
                String::new()
            } else {
                format!(": {message}")
            }
        );
    }
    Ok(stdout)
}

fn display(arguments: &[std::ffi::OsString]) -> String {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::extensions::{
        CommandId,
        permissions::RequestedCapabilities,
        preferences::{MemorySecrets, PreferenceScope, SecretStore as _},
    };

    /// A repository to install from: a bare repository and a working copy
    /// that commits and pushes to it.
    struct Remote {
        bare: PathBuf,
        work: PathBuf,
    }

    impl Remote {
        fn new(root: &Path, id: &str) -> Self {
            let bare = root.join("remote.git");
            let work = root.join("work");
            run(
                root,
                &[
                    "init",
                    "--quiet",
                    "--bare",
                    "--initial-branch=main",
                    "remote.git",
                ],
            );
            run(root, &["init", "--quiet", "--initial-branch=main", "work"]);
            let remote = Self { bare, work };
            remote.write(id, "1.0.0", "");
            std::fs::write(
                remote.work.join("launcher.json"),
                r#"{
                    "commands": [{ "name": "hello", "title": "Hello", "module": "main.js" }],
                    "preferences": [{ "name": "token", "title": "Token", "type": "password" }]
                }"#,
            )
            .unwrap();
            std::fs::write(remote.work.join("main.js"), "export default 1;").unwrap();
            remote.commit("initial");
            run(
                &remote.work,
                &["remote", "add", "origin", remote.bare.to_str().unwrap()],
            );
            run(&remote.work, &["push", "--quiet", "origin", "main"]);
            remote
        }

        fn write(&self, id: &str, version: &str, capabilities: &str) {
            std::fs::write(
                self.work.join("gpui-shell.json"),
                format!(
                    r#"{{ "id": "{id}", "name": "Hello", "version": "{version}", "entry": "main.js"{capabilities} }}"#
                ),
            )
            .unwrap();
        }

        fn commit(&self, message: &str) {
            run(&self.work, &["add", "."]);
            run(
                &self.work,
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--quiet",
                    "-m",
                    message,
                ],
            );
        }

        fn push(&self) {
            run(&self.work, &["push", "--quiet", "origin", "main"]);
        }

        fn source(&self) -> String {
            self.bare.to_str().unwrap().to_owned()
        }
    }

    fn run(directory: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(directory)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {arguments:?} failed");
    }

    #[test]
    fn test_sources_are_parsed_like_dependency_specifiers() {
        let github = GitSource::parse("longbridge/gpui-kit#main").unwrap();
        assert_eq!(github.url(), "https://github.com/longbridge/gpui-kit.git");
        assert_eq!(github.reference(), Some("main"));
        assert_eq!(
            GitSource::parse("https://example.com/a.git").unwrap().url(),
            "https://example.com/a.git"
        );
        assert_eq!(
            GitSource::parse("git@github.com:a/b.git#v1")
                .unwrap()
                .reference(),
            Some("v1")
        );
        assert!(GitSource::parse("not a repo").is_err());
        assert!(GitSource::parse("a/b/c").is_err());
        assert!(GitSource::parse("a/b#--upload-pack=x").is_err());
    }

    #[test]
    fn test_install_update_and_uninstall_from_a_local_repository() {
        let root = tempfile::tempdir().unwrap();
        let remote = Remote::new(root.path(), "com.example.hello");
        let data = DataDirectory::new(root.path().join("data"));

        let first = install(&data, &remote.source()).unwrap();
        assert_eq!(first.id(), "com.example.hello");
        assert_eq!(first.version(), "1.0.0");
        assert_eq!(first.directory(), data.extension_dir("com.example.hello"));
        let catalog = Catalog::discover(&[data.extensions_dir()]);
        assert!(
            catalog
                .command(&CommandId::new("com.example.hello", "hello"))
                .is_some()
        );
        assert!(
            !data
                .root()
                .join("staging")
                .read_dir()
                .unwrap()
                .any(|_| true),
            "nothing is left behind in staging"
        );
        let listed = installed(&data).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].source().unwrap().url(), remote.source());

        // Nothing new upstream: the update is a no-op that still succeeds.
        assert_eq!(
            update(&data, "com.example.hello").unwrap().version(),
            "1.0.0"
        );

        // A new release that asks for more.
        remote.write(
            "com.example.hello",
            "1.1.0",
            r#", "capabilities": { "network": { "hosts": ["api.example.com"] } }"#,
        );
        remote.commit("release 1.1.0");
        remote.push();
        let updated = update(&data, "com.example.hello").unwrap();
        assert_eq!(updated.version(), "1.1.0");
        let requested = RequestedCapabilities::read(updated.directory()).unwrap();
        assert_eq!(requested.items()[0].key(), "network.host:api.example.com");

        // A release that breaks its manifest does not replace the working one.
        std::fs::write(remote.work.join("launcher.json"), "{}").unwrap();
        remote.commit("broken");
        remote.push();
        assert!(update(&data, "com.example.hello").is_err());
        assert_eq!(installed(&data).unwrap()[0].version(), "1.1.0");

        // Another repository cannot claim the installed id.
        let other_root = root.path().join("other");
        std::fs::create_dir_all(&other_root).unwrap();
        let impostor = Remote::new(&other_root, "com.example.hello");
        let error = install(&data, &impostor.source()).unwrap_err();
        assert!(
            format!("{error:#}").contains("already installed"),
            "{error:#}"
        );

        // Uninstalling removes what the launcher kept for it.
        let permissions = PermissionStore::new(data.permissions_file());
        permissions
            .decide("com.example.hello", &requested, true)
            .unwrap();
        let secrets = Rc::new(MemorySecrets::default());
        let preferences = PreferenceStore::new(data.preferences_file(), secrets.clone());
        let catalog = Catalog::discover(&[data.extensions_dir()]);
        let (extension, _) = catalog
            .command(&CommandId::new("com.example.hello", "hello"))
            .unwrap();
        preferences
            .set(
                &PreferenceScope::extension("com.example.hello"),
                &extension.preferences()[0],
                Some("secret".into()),
            )
            .unwrap();
        std::fs::create_dir_all(data.cache_dir("com.example.hello")).unwrap();

        uninstall(&data, "com.example.hello", &permissions, &preferences).unwrap();
        assert!(!data.extension_dir("com.example.hello").exists());
        assert!(!data.extension_data_dir("com.example.hello").exists());
        assert!(secrets.read("com.example.hello/token").unwrap().is_none());
        assert!(matches!(
            permissions.state("com.example.hello", &requested).unwrap(),
            crate::extensions::permissions::PermissionState::Ask(_)
        ));
        assert!(installed(&data).unwrap().is_empty());
        assert!(update(&data, "com.example.hello").is_err());

        // With the id free again, the other repository installs.
        assert!(install(&data, &impostor.source()).is_ok());
    }
}
