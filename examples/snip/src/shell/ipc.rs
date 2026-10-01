//! One Snip per user: the running instance listens on a local socket, and
//! every later `snip …` process forwards its request there and exits.
//!
//! The socket is a Unix domain socket in the user's runtime directory, or a
//! named pipe on Windows. The protocol is one JSON line per connection,
//! answered by one line: `ok`, or `error: <reason>`. The listener runs on its
//! own thread, answers as soon as a message is decoded and hands the message
//! over; handling happens later on the GPUI main thread.

use std::io::{self, BufRead as _, BufReader, Write as _};
#[cfg(unix)]
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
pub use interprocess::local_socket::Listener;
use interprocess::local_socket::{ListenerOptions, Name, Stream, prelude::*};
use serde::{Deserialize, Serialize};

/// A request from another `snip` process.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Message {
    /// Started again with no command: nothing to do but be running.
    Start,
    Capture,
    PinClipboard,
    Settings,
    Quit,
}

impl Message {
    /// The line this message travels as, newline included.
    pub fn encode(&self) -> String {
        let mut line = serde_json::to_string(self).expect("a message always serializes");
        line.push('\n');
        line
    }

    pub fn decode(line: &str) -> Result<Self> {
        serde_json::from_str(line.trim_end()).with_context(|| format!("unknown request {line:?}"))
    }
}

const OK: &str = "ok";
const ERROR_PREFIX: &str = "error: ";

/// Where the running instance listens.
#[derive(Clone, Debug)]
pub struct Endpoint {
    /// The socket file, which must be removed when it is stale. Named pipes
    /// vanish with their process, so Windows has none.
    #[cfg(unix)]
    path: Option<PathBuf>,
    name: Name<'static>,
}

impl Endpoint {
    /// The current user's endpoint.
    pub fn for_current_user() -> io::Result<Self> {
        #[cfg(unix)]
        {
            let directory = dirs::runtime_dir()
                // `$TMPDIR` on macOS is private to the user and short enough
                // for the 104-byte socket path limit, unlike Application Support.
                .or_else(|| cfg!(target_os = "macos").then(std::env::temp_dir))
                .or_else(super::data_directory)
                .ok_or_else(|| io::Error::other("no directory for the snip socket"))?;
            Self::at(directory.join("gpui-kit-snip.sock"))
        }
        #[cfg(windows)]
        {
            use interprocess::local_socket::GenericNamespaced;
            let user = std::env::var("USERNAME").unwrap_or_default();
            let name = format!("gpui-kit-snip-{user}").to_ns_name::<GenericNamespaced>()?;
            Ok(Self {
                name: name.into_owned(),
            })
        }
    }

    /// An endpoint at a socket path.
    #[cfg(unix)]
    pub fn at(path: PathBuf) -> io::Result<Self> {
        use interprocess::local_socket::GenericFilePath;
        let name = path.clone().to_fs_name::<GenericFilePath>()?;
        Ok(Self {
            path: Some(path),
            name: name.into_owned(),
        })
    }

    /// Sends a message to the running instance.
    ///
    /// Returns `Ok(false)` when no instance is running, so the caller can
    /// start one; any other failure is an error.
    pub fn send(&self, message: &Message) -> Result<bool> {
        let stream = match Stream::connect(self.name.clone()) {
            Ok(stream) => stream,
            Err(error) if is_not_running(&error) => return Ok(false),
            Err(error) => return Err(error).context("cannot reach the running Snip"),
        };
        let mut stream = BufReader::new(stream);
        stream.get_mut().write_all(message.encode().as_bytes())?;
        let mut reply = String::new();
        stream.read_line(&mut reply)?;
        match reply.trim_end() {
            OK => Ok(true),
            reply => match reply.strip_prefix(ERROR_PREFIX) {
                Some(reason) => bail!("the running Snip refused the request: {reason}"),
                None => Err(anyhow!("the running Snip sent no answer")),
            },
        }
    }

    /// Becomes the running instance, or reports that one already is.
    ///
    /// A socket file left behind by an instance that crashed accepts no
    /// connection; it is removed and bound again.
    pub fn claim(&self) -> Result<Claim> {
        #[cfg(unix)]
        if let Some(directory) = self.path.as_ref().and_then(|path| path.parent()) {
            create_private_directory(directory)?;
        }
        match self.listen() {
            Ok(listener) => Ok(Claim::Listening(listener)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                if Stream::connect(self.name.clone()).is_ok() {
                    return Ok(Claim::Running);
                }
                #[cfg(unix)]
                if let Some(path) = &self.path {
                    std::fs::remove_file(path).with_context(|| {
                        format!("cannot remove the stale socket {}", path.display())
                    })?;
                }
                Ok(Claim::Listening(self.listen().context("cannot listen")?))
            }
            Err(error) => Err(error).context("cannot listen for other snip processes"),
        }
    }

    fn listen(&self) -> io::Result<Listener> {
        ListenerOptions::new().name(self.name.clone()).create_sync()
    }
}

/// The outcome of [`Endpoint::claim`].
pub enum Claim {
    /// This process is the instance; serve the listener.
    Listening(Listener),
    /// Another process already is.
    Running,
}

/// Answers other processes on a thread of its own, handing each decoded
/// message to `deliver`. Returns once the thread is started.
pub fn serve(listener: Listener, deliver: impl Fn(Message) + Send + 'static) {
    std::thread::Builder::new()
        .name("snip-ipc".into())
        .spawn(move || {
            for connection in listener.incoming() {
                match connection {
                    Ok(connection) => answer(connection, &deliver),
                    Err(error) => tracing::warn!("a snip process could not connect: {error}"),
                }
            }
        })
        .expect("cannot start the snip IPC thread");
}

fn answer(connection: Stream, deliver: &impl Fn(Message)) {
    let mut connection = BufReader::new(connection);
    let mut line = String::new();
    // A connection that closes without a line is `claim` probing for us.
    if connection.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let reply = match Message::decode(&line) {
        Ok(message) => {
            deliver(message);
            format!("{OK}\n")
        }
        Err(error) => format!("{ERROR_PREFIX}{error:#}\n"),
    };
    connection.get_mut().write_all(reply.as_bytes()).ok();
}

fn is_not_running(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

#[cfg(unix)]
fn create_private_directory(directory: &std::path::Path) -> Result<()> {
    if directory.exists() {
        return Ok(());
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(directory)
        .with_context(|| format!("cannot create {}", directory.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_codec() {
        let messages = [
            Message::Start,
            Message::Capture,
            Message::PinClipboard,
            Message::Settings,
            Message::Quit,
        ];
        for message in messages {
            let line = message.encode();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
            assert_eq!(Message::decode(&line).unwrap(), message);
        }
        assert_eq!(Message::Capture.encode(), "{\"command\":\"capture\"}\n");
        assert!(Message::decode("{\"command\":\"explode\"}").is_err());
        assert!(Message::decode("capture").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_claim_send_and_stale_socket() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested").join("snip.sock");
        let endpoint = Endpoint::at(path.clone()).unwrap();
        assert!(
            !endpoint.send(&Message::Start).unwrap(),
            "nothing is running yet"
        );

        drop(std::os::unix::net::UnixListener::bind({
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            &path
        }));
        let Claim::Listening(listener) = endpoint.claim().unwrap() else {
            panic!("a stale socket is reclaimed");
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        serve(listener, move |message| sender.send(message).unwrap());
        assert!(matches!(endpoint.claim().unwrap(), Claim::Running));
        assert!(endpoint.send(&Message::Capture).unwrap());
        assert_eq!(receiver.recv().unwrap(), Message::Capture);
    }
}
