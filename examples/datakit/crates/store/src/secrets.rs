use std::{
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, anyhow};
use datakit_driver::DataSourceId;

/// Where data source passwords are kept, by data source.
pub trait SecretStore: Send + Sync {
    fn read(&self, data_source: &DataSourceId) -> Result<Option<String>>;
    fn write(&self, data_source: &DataSourceId, password: &str) -> Result<()>;
    fn delete(&self, data_source: &DataSourceId) -> Result<()>;

    /// Whether a written password survives quitting DataKit.
    fn is_persistent(&self) -> bool;
}

/// The keychain service every DataKit password is filed under.
const SERVICE: &str = "datakit";

/// The system keychain: Keychain Services on macOS, the Credential Manager on
/// Windows, the Secret Service on Linux.
///
/// A session without a keychain (a bare Linux window manager, a container)
/// keeps passwords in memory until DataKit quits instead, and
/// [`is_persistent`](SecretStore::is_persistent) says so. Passwords are never
/// written to a plain file.
#[derive(Default)]
pub struct KeychainSecrets {
    fallback: MemorySecrets,
    unavailable: AtomicBool,
}

impl KeychainSecrets {
    pub fn new() -> Self {
        Self::default()
    }

    fn entry(&self, data_source: &DataSourceId) -> Option<keyring::Entry> {
        if self.unavailable.load(Ordering::Relaxed) {
            return None;
        }
        match keyring::Entry::new(SERVICE, data_source.as_str()) {
            Ok(entry) => Some(entry),
            Err(error) => {
                self.give_up(&error);
                None
            }
        }
    }

    fn give_up(&self, error: &keyring::Error) {
        if !self.unavailable.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                "the system keychain is unavailable ({error}); passwords are kept in memory \
                 until DataKit quits"
            );
        }
    }

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
    fn read(&self, data_source: &DataSourceId) -> Result<Option<String>> {
        let Some(entry) = self.entry(data_source) else {
            return self.fallback.read(data_source);
        };
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => self.fallback.read(data_source),
            Err(error) if Self::is_unavailable(&error) => {
                self.give_up(&error);
                self.fallback.read(data_source)
            }
            Err(error) => Err(anyhow!(
                "cannot read the password from the keychain: {error}"
            )),
        }
    }

    fn write(&self, data_source: &DataSourceId, password: &str) -> Result<()> {
        let Some(entry) = self.entry(data_source) else {
            return self.fallback.write(data_source, password);
        };
        match entry.set_password(password) {
            Ok(()) => self.fallback.delete(data_source),
            Err(error) if Self::is_unavailable(&error) => {
                self.give_up(&error);
                self.fallback.write(data_source, password)
            }
            Err(error) => Err(anyhow!(
                "cannot store the password in the keychain: {error}"
            )),
        }
    }

    fn delete(&self, data_source: &DataSourceId) -> Result<()> {
        if let Some(entry) = self.entry(data_source) {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(error) if Self::is_unavailable(&error) => self.give_up(&error),
                Err(error) => {
                    return Err(anyhow!(
                        "cannot delete the password from the keychain: {error}"
                    ));
                }
            }
        }
        self.fallback.delete(data_source)
    }

    fn is_persistent(&self) -> bool {
        !self.unavailable.load(Ordering::Relaxed)
    }
}

/// Passwords that live as long as the process.
#[derive(Default)]
pub struct MemorySecrets(Mutex<HashMap<DataSourceId, String>>);

impl SecretStore for MemorySecrets {
    fn read(&self, data_source: &DataSourceId) -> Result<Option<String>> {
        Ok(self.lock().get(data_source).cloned())
    }

    fn write(&self, data_source: &DataSourceId, password: &str) -> Result<()> {
        self.lock()
            .insert(data_source.clone(), password.to_string());
        Ok(())
    }

    fn delete(&self, data_source: &DataSourceId) -> Result<()> {
        self.lock().remove(data_source);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

impl MemorySecrets {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<DataSourceId, String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
