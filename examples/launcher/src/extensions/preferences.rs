//! Preference values: where they are kept and what an extension receives.
//!
//! An extension declares its preferences in `launcher.json` and reads them
//! through `launch().preferences`; it never sees where they are kept. Plain
//! values live in `<data>/preferences.json`. `password` values live in the
//! system keychain, so a settings file that is backed up or shared never
//! carries a token.

use std::{cell::Cell, collections::BTreeMap, path::PathBuf, rc::Rc};

use anyhow::{Result, anyhow};
use serde_json::Value;

use super::{
    Catalog, Extension, ExtensionCommand, PreferenceInput, PreferenceManifest,
    paths::{read_json, write_file, write_json},
};

/// The keychain service every launcher secret is filed under.
const SERVICE: &str = "gpui-kit-launcher";

/// Where password preferences are kept.
///
/// A trait so tests keep secrets in memory rather than in the keychain of
/// whoever runs them.
pub trait SecretStore {
    fn read(&self, account: &str) -> Result<Option<String>>;
    fn write(&self, account: &str, secret: &str) -> Result<()>;
    fn delete(&self, account: &str) -> Result<()>;
}

/// The system keychain: Keychain Services on macOS, the Credential Manager on
/// Windows, the Secret Service on Linux.
///
/// A Linux session without a Secret Service (a bare window manager, a
/// container) has no keychain at all. Refusing to store the preference there
/// would make the extension unusable, so secrets fall back to
/// `<data>/secrets.json`, readable by the user only, with a warning: that file
/// is not encrypted at rest, which is exactly what the keychain was for.
pub struct KeychainSecrets {
    fallback: FileSecrets,
    warned: Cell<bool>,
}

impl KeychainSecrets {
    pub fn new(fallback: PathBuf) -> Self {
        Self {
            fallback: FileSecrets::new(fallback),
            warned: Cell::new(false),
        }
    }

    fn entry(&self, account: &str) -> Option<keyring::Entry> {
        match keyring::Entry::new(SERVICE, account) {
            Ok(entry) => Some(entry),
            Err(error) => {
                self.warn(&error);
                None
            }
        }
    }

    fn warn(&self, error: &keyring::Error) {
        if !self.warned.replace(true) {
            tracing::warn!(
                "the system keychain is unavailable ({error}); password preferences are \
                 stored unencrypted in {}, readable only by you",
                self.fallback.path.display()
            );
        }
    }

    /// Whether `error` means there is no keychain to use, rather than a
    /// problem with this one secret.
    fn is_unavailable(error: &keyring::Error) -> bool {
        matches!(
            error,
            keyring::Error::NoStorageAccess(_)
                | keyring::Error::PlatformFailure(_)
                | keyring::Error::NoDefaultStore
        )
    }
}

impl SecretStore for KeychainSecrets {
    fn read(&self, account: &str) -> Result<Option<String>> {
        let Some(entry) = self.entry(account) else {
            return self.fallback.read(account);
        };
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            // A secret written while the keychain was unavailable is still in
            // the file.
            Err(keyring::Error::NoEntry) => self.fallback.read(account),
            Err(error) if Self::is_unavailable(&error) => {
                self.warn(&error);
                self.fallback.read(account)
            }
            Err(error) => Err(anyhow!(
                "cannot read `{account}` from the keychain: {error}"
            )),
        }
    }

    fn write(&self, account: &str, secret: &str) -> Result<()> {
        let Some(entry) = self.entry(account) else {
            return self.fallback.write(account, secret);
        };
        match entry.set_password(secret) {
            // Moved into the keychain; no stale copy stays in the file.
            Ok(()) => self.fallback.delete(account),
            Err(error) if Self::is_unavailable(&error) => {
                self.warn(&error);
                self.fallback.write(account, secret)
            }
            Err(error) => Err(anyhow!("cannot store `{account}` in the keychain: {error}")),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        if let Some(entry) = self.entry(account) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) if Self::is_unavailable(&error) => self.warn(&error),
                Err(error) => {
                    return Err(anyhow!(
                        "cannot delete `{account}` from the keychain: {error}"
                    ));
                }
            }
        }
        self.fallback.delete(account)
    }
}

/// Secrets in a JSON file only its owner can read.
pub struct FileSecrets {
    path: PathBuf,
}

impl FileSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn load(&self) -> Result<BTreeMap<String, String>> {
        read_json(&self.path)
    }

    fn save(&self, secrets: &BTreeMap<String, String>) -> Result<()> {
        write_file(
            &self.path,
            serde_json::to_string_pretty(secrets)?.as_bytes(),
            true,
        )
    }
}

impl SecretStore for FileSecrets {
    fn read(&self, account: &str) -> Result<Option<String>> {
        Ok(self.load()?.remove(account))
    }

    fn write(&self, account: &str, secret: &str) -> Result<()> {
        let mut secrets = self.load()?;
        secrets.insert(account.to_owned(), secret.to_owned());
        self.save(&secrets)
    }

    fn delete(&self, account: &str) -> Result<()> {
        let mut secrets = self.load()?;
        if secrets.remove(account).is_some() {
            self.save(&secrets)?;
        }
        Ok(())
    }
}

/// Secrets that live as long as the process; for tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemorySecrets(std::cell::RefCell<BTreeMap<String, String>>);

#[cfg(test)]
impl SecretStore for MemorySecrets {
    fn read(&self, account: &str) -> Result<Option<String>> {
        Ok(self.0.borrow().get(account).cloned())
    }

    fn write(&self, account: &str, secret: &str) -> Result<()> {
        self.0
            .borrow_mut()
            .insert(account.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.0.borrow_mut().remove(account);
        Ok(())
    }
}

/// Whose preference a declaration is: the extension's, shared by its
/// commands, or one command's.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PreferenceScope {
    extension: String,
    command: Option<String>,
}

impl PreferenceScope {
    pub fn extension(id: impl Into<String>) -> Self {
        Self {
            extension: id.into(),
            command: None,
        }
    }

    pub fn command(extension: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            extension: extension.into(),
            command: Some(command.into()),
        }
    }

    /// The key in `preferences.json`: `<extension>` or `<extension>/<command>`.
    pub fn key(&self) -> String {
        match &self.command {
            None => self.extension.clone(),
            Some(command) => format!("{}/{command}", self.extension),
        }
    }

    /// The keychain account for a password: `<extension>/<name>`, or
    /// `<extension>/<command>/<name>` so a command's password never collides
    /// with an extension-wide one of the same name.
    fn account(&self, name: &str) -> String {
        format!("{}/{name}", self.key())
    }
}

/// A preference's value as an extension receives it.
pub type PreferenceValue = Value;

/// Resolved values by preference name: stored values, then defaults.
pub type ResolvedPreferences = BTreeMap<String, PreferenceValue>;

/// Preference values for every extension.
#[derive(Clone)]
pub struct PreferenceStore {
    path: PathBuf,
    secrets: Rc<dyn SecretStore>,
}

impl PreferenceStore {
    /// Where password preferences, and an extension's OAuth tokens, are kept.
    pub fn secrets(&self) -> Rc<dyn SecretStore> {
        self.secrets.clone()
    }

    pub fn new(path: PathBuf, secrets: Rc<dyn SecretStore>) -> Self {
        Self { path, secrets }
    }

    fn load(&self) -> Result<BTreeMap<String, BTreeMap<String, Value>>> {
        read_json(&self.path)
    }

    /// The stored value, or `None` when the user never set one.
    pub fn stored(
        &self,
        scope: &PreferenceScope,
        declaration: &PreferenceManifest,
    ) -> Result<Option<Value>> {
        if declaration.input == PreferenceInput::Password {
            return Ok(self
                .secrets
                .read(&scope.account(&declaration.name))?
                .map(Value::String));
        }
        Ok(self
            .load()?
            .remove(&scope.key())
            .and_then(|mut values| values.remove(&declaration.name)))
    }

    /// The value an extension receives: stored, else declared default, else
    /// — for a dropdown — its first choice.
    pub fn value(
        &self,
        scope: &PreferenceScope,
        declaration: &PreferenceManifest,
    ) -> Result<Option<Value>> {
        let value = self
            .stored(scope, declaration)?
            .or_else(|| declaration.default.clone())
            .or_else(|| match declaration.input {
                PreferenceInput::Checkbox => Some(Value::Bool(false)),
                PreferenceInput::Dropdown => declaration
                    .choices
                    .first()
                    .map(|choice| Value::String(choice.value.clone())),
                PreferenceInput::Text | PreferenceInput::Password => None,
            });
        Ok(value)
    }

    /// Stores one value; `None` clears it.
    pub fn set(
        &self,
        scope: &PreferenceScope,
        declaration: &PreferenceManifest,
        value: Option<Value>,
    ) -> Result<()> {
        if declaration.input == PreferenceInput::Password {
            let account = scope.account(&declaration.name);
            return match value.as_ref().and_then(Value::as_str) {
                Some(secret) if !secret.is_empty() => self.secrets.write(&account, secret),
                _ => self.secrets.delete(&account),
            };
        }
        let mut all = self.load()?;
        let values = all.entry(scope.key()).or_default();
        match value {
            Some(value) => values.insert(declaration.name.clone(), value),
            None => values.remove(&declaration.name),
        };
        if values.is_empty() {
            all.remove(&scope.key());
        }
        write_json(&self.path, &all)
    }

    /// Everything a command receives: the extension's preferences, then the
    /// command's, which win a shared name.
    pub fn resolve(
        &self,
        extension: &Extension,
        command: &ExtensionCommand,
    ) -> Result<ResolvedPreferences> {
        let mut resolved = ResolvedPreferences::new();
        for (scope, declarations) in scopes(extension, command) {
            for declaration in declarations {
                if let Some(value) = self.value(&scope, declaration)? {
                    resolved.insert(declaration.name.clone(), value);
                }
            }
        }
        Ok(resolved)
    }

    /// The required preferences that have no value yet.
    pub fn missing<'a>(
        &self,
        extension: &'a Extension,
        command: &'a ExtensionCommand,
    ) -> Result<Vec<&'a PreferenceManifest>> {
        let mut missing = Vec::new();
        for (scope, declarations) in scopes(extension, command) {
            for declaration in declarations.iter().filter(|each| each.required) {
                if is_empty(self.value(&scope, declaration)?.as_ref()) {
                    missing.push(declaration);
                }
            }
        }
        Ok(missing)
    }

    /// Removes every value of an extension, secrets included.
    pub fn forget(
        &self,
        id: &str,
        declarations: &[(PreferenceScope, Vec<PreferenceManifest>)],
    ) -> Result<()> {
        for (scope, declarations) in declarations {
            for declaration in declarations
                .iter()
                .filter(|each| each.input == PreferenceInput::Password)
            {
                self.secrets.delete(&scope.account(&declaration.name))?;
            }
        }
        let mut all = self.load()?;
        let before = all.len();
        let prefix = format!("{id}/");
        all.retain(|key, _| key != id && !key.starts_with(&prefix));
        if all.len() != before {
            write_json(&self.path, &all)?;
        }
        Ok(())
    }
}

/// An empty text is no value: a required field left blank is still missing.
pub fn is_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(_) => false,
    }
}

/// The declarations a command sees, by scope, extension first.
pub fn scopes<'a>(
    extension: &'a Extension,
    command: &'a ExtensionCommand,
) -> [(PreferenceScope, &'a [PreferenceManifest]); 2] {
    [
        (
            PreferenceScope::extension(extension.id().to_string()),
            extension.preferences(),
        ),
        (
            PreferenceScope::command(
                extension.id().to_string(),
                command.id().command().to_string(),
            ),
            command.preferences(),
        ),
    ]
}

/// Every declaration of the extension `id` in `catalog`, by scope, for
/// forgetting it whole.
pub fn all_scopes(catalog: &Catalog, id: &str) -> Vec<(PreferenceScope, Vec<PreferenceManifest>)> {
    let mut scopes = Vec::new();
    for (extension, command) in catalog
        .commands()
        .filter(|(extension, _)| extension.id().as_ref() == id)
    {
        if scopes.is_empty() {
            scopes.push((
                PreferenceScope::extension(id),
                extension.preferences().to_vec(),
            ));
        }
        scopes.push((
            PreferenceScope::command(id, command.id().command().to_string()),
            command.preferences().to_vec(),
        ));
    }
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::CommandId;

    fn extension(root: &std::path::Path) -> Catalog {
        let directory = root.join("github");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("gpui-shell.json"),
            r#"{ "id": "com.example.github", "name": "GitHub", "entry": "main.js" }"#,
        )
        .unwrap();
        std::fs::write(directory.join("main.js"), "").unwrap();
        std::fs::write(
            directory.join("launcher.json"),
            r#"{
                "commands": [{
                    "name": "search", "title": "Search", "module": "main.js",
                    "preferences": [
                        { "name": "sort", "title": "Sort", "type": "dropdown",
                          "choices": [{ "value": "stars", "title": "Stars" }, { "value": "updated", "title": "Updated" }] },
                        { "name": "command_token", "title": "Command Token", "type": "password" }
                    ]
                }],
                "preferences": [
                    { "name": "token", "title": "Token", "type": "password", "required": true },
                    { "name": "host", "title": "Host", "type": "text", "default": "github.com" },
                    { "name": "forks", "title": "Forks", "type": "checkbox", "label": "Include forks" }
                ]
            }"#,
        )
        .unwrap();
        Catalog::discover(&[root.to_path_buf()])
    }

    #[test]
    fn test_values_resolve_with_defaults_and_secrets_stay_out_of_the_file() {
        let data = tempfile::tempdir().unwrap();
        let catalog = extension(data.path());
        let (extension, command) = catalog
            .command(&CommandId::new("com.example.github", "search"))
            .unwrap();
        let secrets = Rc::new(MemorySecrets::default());
        let store = PreferenceStore::new(data.path().join("preferences.json"), secrets.clone());

        let missing: Vec<&str> = store
            .missing(extension, command)
            .unwrap()
            .into_iter()
            .map(|each| each.name.as_str())
            .collect();
        assert_eq!(missing, ["token"]);
        let resolved = store.resolve(extension, command).unwrap();
        assert_eq!(resolved["host"], "github.com");
        assert_eq!(resolved["forks"], false);
        assert_eq!(
            resolved["sort"], "stars",
            "a dropdown starts on its first choice"
        );
        assert!(!resolved.contains_key("token"));

        let scope = PreferenceScope::extension("com.example.github");
        let token = &extension.preferences()[0];
        store
            .set(&scope, token, Some(Value::from("ghp_secret")))
            .unwrap();
        store
            .set(
                &scope,
                &extension.preferences()[1],
                Some(Value::from("example.com")),
            )
            .unwrap();
        assert!(store.missing(extension, command).unwrap().is_empty());

        let file = std::fs::read_to_string(data.path().join("preferences.json")).unwrap();
        assert!(file.contains("example.com"));
        assert!(
            !file.contains("ghp_secret"),
            "a password never reaches the file"
        );
        assert_eq!(
            secrets.read("com.example.github/token").unwrap().as_deref(),
            Some("ghp_secret")
        );

        // A command's own secret is kept under the command, apart from the
        // extension's.
        let command_scope = PreferenceScope::command("com.example.github", "search");
        store
            .set(
                &command_scope,
                &command.preferences()[1],
                Some(Value::from("cmd")),
            )
            .unwrap();
        assert_eq!(
            secrets
                .read("com.example.github/search/command_token")
                .unwrap()
                .as_deref(),
            Some("cmd")
        );
        let resolved = store.resolve(extension, command).unwrap();
        assert_eq!(resolved["command_token"], "cmd");
        assert_eq!(resolved["token"], "ghp_secret");
        assert_eq!(resolved["host"], "example.com");

        // A reopened store reads the same values.
        let reopened = PreferenceStore::new(data.path().join("preferences.json"), secrets.clone());
        assert_eq!(reopened.resolve(extension, command).unwrap(), resolved);

        reopened
            .forget(
                "com.example.github",
                &all_scopes(&catalog, "com.example.github"),
            )
            .unwrap();
        assert!(secrets.0.borrow().is_empty());
        assert_eq!(
            reopened.resolve(extension, command).unwrap()["host"],
            "github.com"
        );
    }

    #[test]
    fn test_file_secrets_are_private_to_the_user() {
        let data = tempfile::tempdir().unwrap();
        let secrets = FileSecrets::new(data.path().join("secrets.json"));
        secrets.write("a/token", "one").unwrap();
        assert_eq!(secrets.read("a/token").unwrap().as_deref(), Some("one"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(data.path().join("secrets.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        secrets.delete("a/token").unwrap();
        assert_eq!(secrets.read("a/token").unwrap(), None);
    }
}
