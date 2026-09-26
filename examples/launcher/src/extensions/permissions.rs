//! What an extension may do: its request, the user's decision, the grant.
//!
//! `gpui-shell.json` states what an extension's code asks for; GPUI Shell
//! leaves the approval product to the host (GPUI Shell §18.5). The launcher
//! breaks the request into [`CapabilityRequest`]s a person can judge one by
//! one, asks before the code first runs, and remembers the answer by
//! extension id in its own data directory. The grant a command runs under is
//! the part of the request the user approved, so an update that asks for more
//! gets nothing new until the user says so.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};
use gpui_shell::{Capabilities, ExecuteGrant, HttpRequestGrant, plugin::MANIFEST_FILE};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::paths::{read_json, write_json};

const PLUGIN_DIR: &str = "${pluginDir}";
const DATA_DIR: &str = "${dataDir}";

/// How much a capability deserves the user's attention.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    Normal,
    /// Equivalent to running anything as the user, such as `execute: "*"`;
    /// shown apart from the rest so it cannot be approved in passing.
    High,
}

/// One thing an extension asks to do, in words a person can judge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityRequest {
    /// Stable across releases and independent of where the extension is
    /// installed, so an approval survives an update that asks for the same.
    key: String,
    description: String,
    severity: Severity,
}

impl CapabilityRequest {
    fn new(key: String, description: String) -> Self {
        Self {
            key,
            description,
            severity: Severity::Normal,
        }
    }

    fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// A Markdown sentence fragment, such as "Connect to `api.github.com`".
    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn severity(&self) -> Severity {
        self.severity
    }
}

/// The capability block of `gpui-shell.json`, read leniently: GPUI Shell's
/// own parser has already validated it strictly before the launcher reads it.
#[derive(Clone, Debug, Default, Deserialize)]
struct RequestFile {
    #[serde(default)]
    fs: FsRequest,
    #[serde(default)]
    network: NetworkRequest,
    #[serde(default = "granted_by_default")]
    storage: bool,
    #[serde(default)]
    clipboard: ClipboardRequest,
    #[serde(default)]
    process: ProcessRequest,
}

fn granted_by_default() -> bool {
    true
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FsRequest {
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    write: Vec<String>,
    /// A list of command names, or `"*"`.
    #[serde(default)]
    execute: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct NetworkRequest {
    #[serde(default)]
    hosts: Vec<String>,
    #[serde(default)]
    http: Vec<HttpRequest>,
}

#[derive(Clone, Debug, Deserialize)]
struct HttpRequest {
    #[serde(default = "https")]
    scheme: String,
    host: String,
    #[serde(default)]
    port: Option<u16>,
    methods: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default)]
    path_prefixes: Vec<String>,
}

fn https() -> String {
    "https".to_owned()
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ClipboardRequest {
    #[serde(default)]
    read: bool,
    #[serde(default)]
    write: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct ProcessRequest {
    #[serde(default)]
    exit: bool,
}

impl HttpRequest {
    /// `GET https://api.example.com/v1/account`, identifying the rule.
    fn key(&self) -> String {
        let mut methods = self.methods.clone();
        methods.sort();
        let mut paths: Vec<String> = self
            .paths
            .iter()
            .cloned()
            .chain(self.path_prefixes.iter().map(|prefix| format!("{prefix}*")))
            .collect();
        paths.sort();
        format!(
            "network.http:{} {}://{}{} {}",
            methods.join(","),
            self.scheme,
            self.host.to_ascii_lowercase(),
            self.port.map(|port| format!(":{port}")).unwrap_or_default(),
            paths.join(",")
        )
    }

    fn describe(&self) -> String {
        let origin = format!(
            "{}://{}{}",
            self.scheme,
            self.host,
            self.port.map(|port| format!(":{port}")).unwrap_or_default()
        );
        let paths: Vec<String> = self
            .paths
            .iter()
            .map(|path| format!("`{path}`"))
            .chain(
                self.path_prefixes
                    .iter()
                    .map(|prefix| format!("anything under `{prefix}`")),
            )
            .collect();
        format!(
            "Send {} requests to `{origin}`: {}",
            self.methods.join(" and "),
            paths.join(", ")
        )
    }
}

/// Everything an extension's `gpui-shell.json` asks for.
#[derive(Clone, Debug)]
pub struct RequestedCapabilities {
    request: RequestFile,
    items: Vec<CapabilityRequest>,
}

impl RequestedCapabilities {
    /// Reads the request of the extension in `directory`.
    pub fn read(directory: &Path) -> Result<Self> {
        let path = directory.join(MANIFEST_FILE);
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        Self::parse(&source).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn parse(manifest: &str) -> Result<Self> {
        let manifest: serde_json::Value = serde_json::from_str(manifest)?;
        let request: RequestFile = match manifest.get("capabilities") {
            Some(block) => serde_json::from_value(block.clone())?,
            None => RequestFile {
                storage: true,
                ..RequestFile::default()
            },
        };
        let items = Self::collect_items(&request);
        Ok(Self { request, items })
    }

    fn collect_items(request: &RequestFile) -> Vec<CapabilityRequest> {
        let mut items = Vec::new();
        if let Some(execute) = &request.fs.execute {
            match execute {
                serde_json::Value::String(_) => items.push(
                    CapabilityRequest::new(
                        "fs.execute:*".into(),
                        "Run **any program** on your computer, with your permissions".into(),
                    )
                    .with_severity(Severity::High),
                ),
                serde_json::Value::Array(commands) => {
                    items.extend(commands.iter().filter_map(|command| {
                        let command = command.as_str()?;
                        Some(CapabilityRequest::new(
                            format!("fs.execute:{command}"),
                            format!("Run the `{command}` program"),
                        ))
                    }));
                }
                _ => {}
            }
        }
        items.extend(request.fs.write.iter().map(|path| {
            CapabilityRequest::new(
                format!("fs.write:{path}"),
                format!("Change files in {}", describe_path(path)),
            )
        }));
        items.extend(request.fs.read.iter().map(|path| {
            CapabilityRequest::new(
                format!("fs.read:{path}"),
                format!("Read files in {}", describe_path(path)),
            )
        }));
        items.extend(request.network.hosts.iter().map(|host| {
            CapabilityRequest::new(
                format!("network.host:{}", host.to_ascii_lowercase()),
                format!("Connect to `{host}` in any way"),
            )
        }));
        items.extend(
            request
                .network
                .http
                .iter()
                .map(|rule| CapabilityRequest::new(rule.key(), rule.describe())),
        );
        if request.clipboard.read {
            items.push(CapabilityRequest::new(
                "clipboard.read".into(),
                "Read what you copy to the clipboard".into(),
            ));
        }
        if request.clipboard.write {
            items.push(CapabilityRequest::new(
                "clipboard.write".into(),
                "Replace what is on the clipboard".into(),
            ));
        }
        if request.process.exit {
            items.push(CapabilityRequest::new(
                "process.exit".into(),
                "Ask the launcher to quit".into(),
            ));
        }
        // Duplicates in the manifest would otherwise be asked twice.
        let mut seen = BTreeSet::new();
        items.retain(|item| seen.insert(item.key.clone()));
        items
    }

    /// The capabilities that need the user's approval, most severe first.
    pub fn items(&self) -> &[CapabilityRequest] {
        &self.items
    }

    /// Whether the extension keeps its own settings (`localStorage`).
    ///
    /// Granted without asking, as GPUI Shell's manifest layer grants it by
    /// default (GPUI Shell §17.3): the store is keyed by the extension's id,
    /// holds nothing from outside the extension, and denying it would only
    /// cost the extension its settings.
    pub fn has_storage(&self) -> bool {
        self.request.storage
    }

    /// Identifies what is requested, independent of order and of where the
    /// extension is installed.
    pub fn fingerprint(&self) -> String {
        fingerprint(self.items.iter().map(|item| item.key.as_str()))
    }

    /// The grant for the approved part of this request.
    ///
    /// `${pluginDir}` and `${dataDir}` are expanded here, by the host, as GPUI
    /// Shell expands them; a relative path is anchored to the extension.
    pub fn grant(
        &self,
        approved: &BTreeSet<String>,
        plugin_dir: &Path,
        data_dir: &Path,
    ) -> Capabilities {
        let allowed = |key: String| approved.contains(&key);
        let request = &self.request;
        let paths = |field: &str, paths: &[String]| -> Vec<PathBuf> {
            paths
                .iter()
                .filter(|path| allowed(format!("fs.{field}:{path}")))
                .map(|path| expand(path, plugin_dir, data_dir))
                .collect()
        };
        let execute = match &request.fs.execute {
            Some(serde_json::Value::String(_)) if allowed("fs.execute:*".into()) => {
                ExecuteGrant::Unrestricted
            }
            Some(serde_json::Value::Array(commands)) => {
                let commands: Vec<String> = commands
                    .iter()
                    .filter_map(|command| command.as_str())
                    .filter(|command| allowed(format!("fs.execute:{command}")))
                    .map(str::to_owned)
                    .collect();
                if commands.is_empty() {
                    ExecuteGrant::Denied
                } else {
                    ExecuteGrant::Allowed(commands)
                }
            }
            _ => ExecuteGrant::Denied,
        };
        Capabilities::new()
            .read_roots(paths("read", &request.fs.read))
            .write_roots(paths("write", &request.fs.write))
            .execute(execute)
            .network_hosts(
                request
                    .network
                    .hosts
                    .iter()
                    .filter(|host| allowed(format!("network.host:{}", host.to_ascii_lowercase())))
                    .cloned(),
            )
            .http_requests(
                request
                    .network
                    .http
                    .iter()
                    .filter(|rule| allowed(rule.key()))
                    .map(|rule| {
                        let grant = HttpRequestGrant::new(
                            rule.host.clone(),
                            rule.methods.clone(),
                            rule.paths.clone(),
                            rule.path_prefixes.clone(),
                        )
                        .scheme(rule.scheme.clone());
                        match rule.port {
                            Some(port) => grant.port(port),
                            None => grant,
                        }
                    }),
            )
            .storage(request.storage)
            .clipboard_read(request.clipboard.read && allowed("clipboard.read".into()))
            .clipboard_write(request.clipboard.write && allowed("clipboard.write".into()))
            .exit(request.process.exit && allowed("process.exit".into()))
    }
}

fn fingerprint<'a>(keys: impl Iterator<Item = &'a str>) -> String {
    let keys: BTreeSet<&str> = keys.collect();
    let mut hasher = Sha256::new();
    for key in keys {
        hasher.update(key.as_bytes());
        hasher.update(b"\n");
    }
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn describe_path(path: &str) -> String {
    match path {
        PLUGIN_DIR => "its own folder".to_owned(),
        DATA_DIR => "its data folder".to_owned(),
        path => format!(
            "`{}`",
            path.replace(PLUGIN_DIR, "<extension>")
                .replace(DATA_DIR, "<data>")
        ),
    }
}

fn expand(raw: &str, plugin_dir: &Path, data_dir: &Path) -> PathBuf {
    let expanded = raw
        .replace(PLUGIN_DIR, &plugin_dir.to_string_lossy())
        .replace(DATA_DIR, &data_dir.to_string_lossy());
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        plugin_dir.join(path)
    }
}

/// What the user decided for one extension.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Decision {
    /// The fingerprint of the request the user last answered.
    fingerprint: String,
    /// Every capability the user was shown, allowed or not.
    asked: BTreeSet<String>,
    /// The capabilities the user allowed.
    approved: BTreeSet<String>,
}

/// Whether a command may run now, and under what.
#[derive(Clone, Debug, PartialEq)]
pub enum PermissionState {
    /// Run with these approved capability keys.
    Decided(BTreeSet<String>),
    /// Ask first.
    Ask(PermissionQuestion),
}

/// The permission page's content: what is requested and what is new.
#[derive(Clone, Debug, PartialEq)]
pub struct PermissionQuestion {
    requested: Vec<CapabilityRequest>,
    new: BTreeSet<String>,
    first_time: bool,
}

impl PermissionQuestion {
    pub fn requested(&self) -> &[CapabilityRequest] {
        &self.requested
    }

    /// Whether the user has not been asked about `item` before.
    pub fn is_new(&self, item: &CapabilityRequest) -> bool {
        self.new.contains(item.key())
    }

    /// Whether this extension has never been asked about before, as opposed
    /// to an update that asks for more.
    pub fn is_first_time(&self) -> bool {
        self.first_time
    }
}

/// `<data>/permissions.json`: decisions by extension id.
#[derive(Clone, Debug)]
pub struct PermissionStore {
    path: PathBuf,
}

impl PermissionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> Result<BTreeMap<String, Decision>> {
        read_json(&self.path)
    }

    /// Compares `requested` with what the user decided before.
    pub fn state(&self, id: &str, requested: &RequestedCapabilities) -> Result<PermissionState> {
        if requested.items().is_empty() {
            return Ok(PermissionState::Decided(BTreeSet::new()));
        }
        let decisions = self.load()?;
        let keys: BTreeSet<String> = requested
            .items()
            .iter()
            .map(|item| item.key.clone())
            .collect();
        let Some(decision) = decisions.get(id) else {
            return Ok(PermissionState::Ask(PermissionQuestion {
                requested: requested.items().to_vec(),
                new: keys,
                first_time: true,
            }));
        };
        let new: BTreeSet<String> = keys.difference(&decision.asked).cloned().collect();
        if decision.fingerprint == requested.fingerprint() || new.is_empty() {
            // A request that shrank needs no question: everything left was
            // already answered.
            let approved = decision.approved.intersection(&keys).cloned().collect();
            return Ok(PermissionState::Decided(approved));
        }
        Ok(PermissionState::Ask(PermissionQuestion {
            requested: requested.items().to_vec(),
            new,
            first_time: false,
        }))
    }

    /// Records the user's answer to `requested`.
    ///
    /// Allowing approves everything requested. Not allowing withholds what was
    /// new and keeps what the user allowed before, so declining an update's
    /// extra request does not take away what the extension already had.
    pub fn decide(
        &self,
        id: &str,
        requested: &RequestedCapabilities,
        allow: bool,
    ) -> Result<BTreeSet<String>> {
        let mut decisions = self.load()?;
        let keys: BTreeSet<String> = requested
            .items()
            .iter()
            .map(|item| item.key.clone())
            .collect();
        let previous = decisions.remove(id).unwrap_or_default();
        let approved: BTreeSet<String> = if allow {
            keys.clone()
        } else {
            previous.approved.intersection(&keys).cloned().collect()
        };
        decisions.insert(
            id.to_owned(),
            Decision {
                fingerprint: requested.fingerprint(),
                asked: keys,
                approved: approved.clone(),
            },
        );
        write_json(&self.path, &decisions)?;
        Ok(approved)
    }

    /// Forgets an extension, so a reinstall asks again.
    pub fn forget(&self, id: &str) -> Result<()> {
        let mut decisions = self.load()?;
        if decisions.remove(id).is_some() {
            write_json(&self.path, &decisions)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB: &str = r#"{
        "id": "com.example.github", "name": "GitHub", "entry": "main.js",
        "capabilities": {
            "network": {
                "hosts": ["api.github.com"],
                "http": [{ "host": "uploads.github.com", "methods": ["POST"], "path_prefixes": ["/repos/"] }]
            },
            "fs": { "read": ["${dataDir}"], "execute": ["git"] }
        }
    }"#;

    fn keys(requested: &RequestedCapabilities) -> Vec<&str> {
        requested
            .items()
            .iter()
            .map(CapabilityRequest::key)
            .collect()
    }

    #[test]
    fn test_request_is_described_item_by_item() {
        let requested = RequestedCapabilities::parse(GITHUB).unwrap();
        assert_eq!(
            keys(&requested),
            [
                "fs.execute:git",
                "fs.read:${dataDir}",
                "network.host:api.github.com",
                "network.http:POST https://uploads.github.com /repos/*",
            ]
        );
        assert!(requested.has_storage(), "storage is granted by default");
        assert_eq!(
            requested.items()[1].description(),
            "Read files in its data folder"
        );

        let everything = RequestedCapabilities::parse(
            r#"{ "id": "a", "name": "A", "entry": "a.js",
                 "capabilities": { "fs": { "execute": "*" }, "storage": false } }"#,
        )
        .unwrap();
        assert_eq!(everything.items()[0].severity(), Severity::High);
        assert!(!everything.has_storage());

        let nothing =
            RequestedCapabilities::parse(r#"{ "id": "a", "name": "A", "entry": "a.js" }"#).unwrap();
        assert!(nothing.items().is_empty());
        assert!(nothing.has_storage());
    }

    #[test]
    fn test_fingerprint_ignores_order_and_follows_content() {
        let reordered = RequestedCapabilities::parse(
            r#"{ "id": "com.example.github", "name": "GitHub", "entry": "main.js",
                 "capabilities": {
                    "fs": { "execute": ["git"], "read": ["${dataDir}"] },
                    "network": {
                        "http": [{ "host": "UPLOADS.github.com", "methods": ["POST"], "path_prefixes": ["/repos/"] }],
                        "hosts": ["api.github.com"]
                    }
                 } }"#,
        )
        .unwrap();
        let original = RequestedCapabilities::parse(GITHUB).unwrap();
        assert_eq!(original.fingerprint(), reordered.fingerprint());

        let more = RequestedCapabilities::parse(&GITHUB.replace(r#"["git"]"#, r#"["git", "gh"]"#))
            .unwrap();
        assert_ne!(original.fingerprint(), more.fingerprint());
    }

    #[test]
    fn test_decisions_persist_and_updates_ask_only_about_what_is_new() {
        let data = tempfile::tempdir().unwrap();
        let store = PermissionStore::new(data.path().join("permissions.json"));
        let id = "com.example.github";
        let original = RequestedCapabilities::parse(GITHUB).unwrap();

        let PermissionState::Ask(question) = store.state(id, &original).unwrap() else {
            panic!("a first run asks");
        };
        assert!(question.is_first_time());
        assert!(
            question
                .requested()
                .iter()
                .all(|item| question.is_new(item))
        );

        store.decide(id, &original, true).unwrap();
        // A second store over the same file sees the decision.
        let reopened = PermissionStore::new(data.path().join("permissions.json"));
        let PermissionState::Decided(approved) = reopened.state(id, &original).unwrap() else {
            panic!("an allowed request is not asked again");
        };
        assert_eq!(approved.len(), 4);

        let update =
            RequestedCapabilities::parse(&GITHUB.replace(r#"["git"]"#, r#"["git", "gh"]"#))
                .unwrap();
        let PermissionState::Ask(question) = reopened.state(id, &update).unwrap() else {
            panic!("an update that asks for more asks again");
        };
        assert!(!question.is_first_time());
        let new: Vec<&str> = question
            .requested()
            .iter()
            .filter(|item| question.is_new(item))
            .map(CapabilityRequest::key)
            .collect();
        assert_eq!(new, ["fs.execute:gh"]);

        // Declining the update keeps what was allowed before.
        let approved = reopened.decide(id, &update, false).unwrap();
        assert!(approved.contains("fs.execute:git"));
        assert!(!approved.contains("fs.execute:gh"));
        assert_eq!(
            reopened.state(id, &update).unwrap(),
            PermissionState::Decided(approved.clone()),
            "a declined request is not asked again either"
        );

        // A request that shrank runs with what is left, without asking.
        let PermissionState::Decided(approved) = reopened.state(id, &original).unwrap() else {
            panic!("a smaller request is not asked");
        };
        assert!(!approved.contains("fs.execute:gh"));

        reopened.forget(id).unwrap();
        assert!(matches!(
            reopened.state(id, &original).unwrap(),
            PermissionState::Ask(_)
        ));
    }

    #[test]
    fn test_grant_is_the_approved_part_of_the_request() {
        let requested = RequestedCapabilities::parse(GITHUB).unwrap();
        let plugin = Path::new("/extensions/github");
        let data = Path::new("/data/github");

        let nothing = requested.grant(&BTreeSet::new(), plugin, data);
        assert!(!nothing.has_read_access());
        assert!(!nothing.may_run("git"));
        assert!(!nothing.may_reach("api.github.com"));
        assert!(nothing.has_storage(), "storage needs no approval");

        let approved: BTreeSet<String> = [
            "fs.execute:git",
            "network.http:POST https://uploads.github.com /repos/*",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let some = requested.grant(&approved, plugin, data);
        assert!(some.may_run("git"));
        assert!(!some.may_reach("api.github.com"));
        assert!(some.may_request("https", "uploads.github.com", None, "POST", "/repos/a"));
        assert!(!some.has_read_access());

        let all: BTreeSet<String> = requested
            .items()
            .iter()
            .map(|item| item.key().to_owned())
            .collect();
        let everything = requested.grant(&all, plugin, data);
        assert!(everything.has_read_access());
        assert!(everything.may_reach("api.github.com"));
    }
}
