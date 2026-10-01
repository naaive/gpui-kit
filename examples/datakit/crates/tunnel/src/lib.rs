//! SSH tunnels: reaching a database that only a bastion host can see.
//!
//! A tunnel listens on a free port of `127.0.0.1` and forwards every
//! connection made to it through the SSH server to the database's host and
//! port. The driver connects to the local end as if the database were here.
//! [`connect`] wraps the driver's connection so the tunnel lives exactly as
//! long as the session that uses it.
//!
//! Host keys are trusted on first use: the first connection records the
//! server's key fingerprint in a known-hosts file, and a later key that does
//! not match is refused, as OpenSSH does.
//!
//! Like the drivers, everything here runs on Tokio.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result, anyhow, bail};
use datakit_catalog::Schema;
use datakit_driver::{BoxFuture, Connection, ConnectionProfile, Driver, StatementOutcome};
use russh::{
    client::{self, Handle},
    keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, load_secret_key},
};
use tokio::{net::TcpListener, task::JoinHandle};

/// The profile options that describe a tunnel. A profile with [`HOST`] set
/// connects through one.
pub const HOST: &str = "ssh.host";
pub const PORT: &str = "ssh.port";
pub const USER: &str = "ssh.user";
/// A private key file; without one, the SSH password is used.
pub const KEY_FILE: &str = "ssh.key-file";

/// Whether `profile` connects through an SSH tunnel.
pub fn uses_tunnel(profile: &ConnectionProfile) -> bool {
    profile.option(HOST).is_some_and(|host| !host.is_empty())
}

/// An open tunnel. Dropping it stops forwarding and closes the SSH session.
pub struct Tunnel {
    local: SocketAddr,
    accept: JoinHandle<()>,
    session: Arc<Handle<Client>>,
}

impl Tunnel {
    /// Where the driver connects instead of the database.
    pub fn local_address(&self) -> SocketAddr {
        self.local
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.accept.abort();
        let session = self.session.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = session
                    .disconnect(russh::Disconnect::ByApplication, "", "")
                    .await;
            });
        }
    }
}

/// Open the tunnel `profile` describes. `password` is the SSH password, or
/// the passphrase of the key file when there is one.
pub async fn open(
    profile: &ConnectionProfile,
    password: Option<String>,
    known_hosts: &Path,
) -> Result<Tunnel> {
    let host = profile
        .option(HOST)
        .filter(|host| !host.is_empty())
        .context("The data source has no SSH host")?
        .to_string();
    let port: u16 = match profile.option(PORT) {
        Some(port) if !port.is_empty() => port
            .parse()
            .with_context(|| format!("“{port}” is not an SSH port"))?,
        _ => 22,
    };
    let user = profile.option(USER).unwrap_or_default().to_string();
    let config = Arc::new(client::Config::default());
    let handler = Client {
        address: format!("{host}:{port}"),
        known_hosts: known_hosts.to_path_buf(),
        problem: None,
    };
    let mut session = client::connect(config, (host.as_str(), port), handler)
        .await
        .with_context(|| format!("Couldn’t reach the SSH server {host}:{port}"))?;

    let authenticated = match profile.option(KEY_FILE).filter(|file| !file.is_empty()) {
        Some(file) => {
            let key = load_secret_key(file, password.as_deref())
                .with_context(|| format!("Couldn’t read the SSH key {file}"))?;
            let hash = session
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten();
            session
                .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await?
        }
        None => {
            session
                .authenticate_password(&user, password.unwrap_or_default())
                .await?
        }
    };
    if !authenticated.success() {
        bail!("The SSH server refused {user}’s credentials");
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let local = listener.local_addr()?;
    let session = Arc::new(session);
    let target_host = profile.host().to_string();
    let target_port = profile.port();
    let forwarding = session.clone();
    let accept = tokio::spawn(async move {
        loop {
            let Ok((mut socket, origin)) = listener.accept().await else {
                return;
            };
            let session = forwarding.clone();
            let target_host = target_host.clone();
            tokio::spawn(async move {
                let channel = match session
                    .channel_open_direct_tcpip(
                        target_host,
                        u32::from(target_port),
                        origin.ip().to_string(),
                        u32::from(origin.port()),
                    )
                    .await
                {
                    Ok(channel) => channel,
                    Err(error) => {
                        tracing::warn!("the SSH server wouldn’t forward a connection: {error}");
                        return;
                    }
                };
                let mut stream = channel.into_stream();
                if let Err(error) = tokio::io::copy_bidirectional(&mut socket, &mut stream).await {
                    tracing::debug!("a tunnelled connection ended: {error}");
                }
            });
        }
    });
    Ok(Tunnel {
        local,
        accept,
        session,
    })
}

/// Connect with `driver` to `profile`'s database, through its SSH tunnel
/// when it has one.
pub async fn connect(
    driver: Arc<dyn Driver>,
    profile: ConnectionProfile,
    password: Option<String>,
    ssh_password: Option<String>,
    known_hosts: PathBuf,
) -> Result<Arc<dyn Connection>> {
    if !uses_tunnel(&profile) {
        return driver.connect(&profile, password).await;
    }
    let tunnel = open(&profile, ssh_password, &known_hosts).await?;
    let local = tunnel.local_address();
    let through = profile
        .clone()
        .with_host(local.ip().to_string())
        .with_port(local.port());
    let inner = driver.connect(&through, password).await?;
    Ok(Arc::new(TunneledConnection {
        inner,
        _tunnel: tunnel,
    }))
}

/// A connection and the tunnel it runs through.
struct TunneledConnection {
    inner: Arc<dyn Connection>,
    _tunnel: Tunnel,
}

impl Connection for TunneledConnection {
    fn server_version(&self) -> Arc<str> {
        self.inner.server_version()
    }

    fn is_closed(&self) -> bool {
        self.inner.is_closed()
    }

    fn execute(&self, sql: Arc<str>) -> BoxFuture<StatementOutcome> {
        self.inner.execute(sql)
    }

    fn cancel(&self) -> BoxFuture<()> {
        self.inner.cancel()
    }

    fn introspect_schemas(&self) -> BoxFuture<Vec<Schema>> {
        self.inner.introspect_schemas()
    }

    fn introspect_schema(&self, schema: Arc<str>) -> BoxFuture<Schema> {
        self.inner.introspect_schema(schema)
    }

    fn search_path(&self) -> BoxFuture<Vec<Arc<str>>> {
        self.inner.search_path()
    }
}

/// The SSH client's side of the conversation: deciding whether to trust the
/// server's key.
struct Client {
    address: String,
    known_hosts: PathBuf,
    problem: Option<String>,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fingerprint = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256),
            PublicKeyOrCertificate::Certificate(certificate) => {
                certificate.public_key().fingerprint(HashAlg::Sha256)
            }
        }
        .to_string();
        match check_known_host(&self.known_hosts, &self.address, &fingerprint) {
            Ok(()) => Ok(true),
            Err(error) => {
                self.problem = Some(error.to_string());
                Err(error)
            }
        }
    }
}

/// Accept `fingerprint` for `address` if the file has it or has nothing
/// for the address yet, recording it then.
fn check_known_host(path: &Path, address: &str, fingerprint: &str) -> Result<()> {
    let known = std::fs::read_to_string(path).unwrap_or_default();
    for line in known.lines() {
        if let Some((host, recorded)) = line.split_once(' ')
            && host == address
        {
            if recorded.trim() == fingerprint {
                return Ok(());
            }
            return Err(anyhow!(
                "The SSH server {address} presented a different host key ({fingerprint}) \
                 than before. If the server was really reinstalled, remove its line from {}.",
                path.display()
            ));
        }
    }
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)?;
    }
    let mut known = known;
    if !known.is_empty() && !known.ends_with('\n') {
        known.push('\n');
    }
    known.push_str(&format!("{address} {fingerprint}\n"));
    std::fs::write(path, known)?;
    tracing::info!("trusting the SSH host key of {address}: {fingerprint}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_keys_are_trusted_on_first_use_and_then_pinned() {
        let directory = std::env::temp_dir().join(format!(
            "datakit-tunnel-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join("known_hosts");
        check_known_host(&path, "bastion:22", "SHA256:abc").unwrap();
        check_known_host(&path, "bastion:22", "SHA256:abc").unwrap();
        check_known_host(&path, "other:22", "SHA256:xyz").unwrap();
        let error = check_known_host(&path, "bastion:22", "SHA256:evil").unwrap_err();
        assert!(error.to_string().contains("different host key"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn a_profile_uses_a_tunnel_only_with_a_host() {
        let profile = ConnectionProfile::new("postgresql", 5432);
        assert!(!uses_tunnel(&profile));
        assert!(uses_tunnel(&profile.with_option(HOST, "bastion")));
    }
}
