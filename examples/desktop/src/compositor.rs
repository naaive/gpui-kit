//! The compositor seam: workspaces, open application windows and the commands
//! that act on them.
//!
//! The prototype speaks the sway/i3 IPC protocol, which sway exposes through
//! `SWAYSOCK`. Wayland itself has no protocol for listing workspaces, and the
//! window list protocol (`wlr-foreign-toplevel`) is not wired into GPUI, so a
//! shell needs a compositor-specific channel either way. Without one the
//! compositor reports itself unavailable and the shell hides what depends on it.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow};
use gpui_kit::{App, Context, SharedString, Task};
use serde::Deserialize;
use serde_json::Value;

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_WORKSPACES: u32 = 1;
const SUBSCRIBE: u32 = 2;
const GET_TREE: u32 = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    name: SharedString,
    focused: bool,
    urgent: bool,
}

impl Workspace {
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn is_urgent(&self) -> bool {
        self.urgent
    }
}

/// An application window as the compositor identifies it: a Wayland `app_id`
/// or, for an X11 client, its `WM_CLASS` class.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WindowIdentity {
    AppId(String),
    Class(String),
}

impl WindowIdentity {
    pub fn name(&self) -> &str {
        match self {
            Self::AppId(name) | Self::Class(name) => name,
        }
    }

    fn criteria(&self) -> String {
        match self {
            Self::AppId(name) => format!("[app_id=\"^{}$\"]", escape_regex(name)),
            Self::Class(name) => format!("[class=\"^{}$\"]", escape_regex(name)),
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq)]
struct Snapshot {
    workspaces: Vec<Workspace>,
    windows: BTreeSet<WindowIdentity>,
}

pub struct Compositor {
    socket: Option<PathBuf>,
    snapshot: Snapshot,
    _watch: Option<Task<()>>,
}

impl Compositor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let socket = std::env::var_os("SWAYSOCK")
            .or_else(|| std::env::var_os("I3SOCK"))
            .map(PathBuf::from);
        let watch = socket.clone().map(|socket| Self::watch(socket, cx));
        Self {
            socket,
            snapshot: Snapshot::default(),
            _watch: watch,
        }
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.snapshot.workspaces
    }

    pub fn windows(&self) -> &BTreeSet<WindowIdentity> {
        &self.snapshot.windows
    }

    pub fn focus_workspace(&self, workspace: &Workspace, cx: &mut App) {
        let name = workspace.name.replace('"', "\\\"");
        self.run(format!("workspace \"{name}\""), cx);
    }

    pub fn focus_window(&self, window: &WindowIdentity, cx: &mut App) {
        self.run(format!("{} focus", window.criteria()), cx);
    }

    /// Starts a desktop entry's command line. The compositor runs it when it can,
    /// so the new window lands on the current workspace and inherits the
    /// session's environment; otherwise the shell spawns it itself.
    pub fn launch(&self, command_line: String, cx: &mut App) {
        if self.socket.is_some() {
            self.run(format!("exec {command_line}"), cx);
            return;
        }
        cx.background_executor()
            .spawn(async move {
                // `sh` returns as soon as the background job starts, so the
                // wait reaps it without holding on to the application.
                let status = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(format!("{command_line} &"))
                    .status();
                if let Err(err) = status {
                    eprintln!("desktop: couldn't start `{command_line}`: {err}");
                }
            })
            .detach();
    }

    fn run(&self, command: String, cx: &mut App) {
        let Some(socket) = self.socket.clone() else {
            return;
        };
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = request(&socket, RUN_COMMAND, command.as_bytes()) {
                    eprintln!("desktop: compositor command `{command}` failed: {err:#}");
                }
            })
            .detach();
    }

    /// Keeps the snapshot current: one connection subscribes to workspace and
    /// window events on its own thread, and each event triggers a fresh query.
    fn watch(socket: PathBuf, cx: &mut Context<Self>) -> Task<()> {
        let (events_tx, events_rx) = smol::channel::unbounded::<()>();
        let subscriber = socket.clone();
        std::thread::Builder::new()
            .name("compositor-events".into())
            .spawn(move || {
                if let Err(err) = subscribe(&subscriber, &events_tx) {
                    eprintln!("desktop: compositor event stream ended: {err:#}");
                }
            })
            .ok();

        cx.spawn(async move |this, cx| {
            loop {
                let query = socket.clone();
                let snapshot = cx
                    .background_executor()
                    .spawn(async move { query_snapshot(&query) })
                    .await;
                match snapshot {
                    Ok(snapshot) => {
                        let updated = this.update(cx, |this, cx| {
                            if this.snapshot != snapshot {
                                this.snapshot = snapshot;
                                cx.notify();
                            }
                        });
                        if updated.is_err() {
                            return;
                        }
                    }
                    Err(err) => eprintln!("desktop: couldn't read compositor state: {err:#}"),
                }

                if events_rx.recv().await.is_err() {
                    return;
                }
                // Coalesce a burst (a workspace switch emits several events)
                // into one query.
                while events_rx.try_recv().is_ok() {}
            }
        })
    }
}

fn subscribe(socket: &PathBuf, events: &smol::channel::Sender<()>) -> Result<()> {
    let mut stream = UnixStream::connect(socket).context("connect")?;
    write_message(&mut stream, SUBSCRIBE, br#"["workspace","window"]"#)?;
    let (_, reply) = read_message(&mut stream)?;
    let reply: Value = serde_json::from_slice(&reply)?;
    if reply.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("subscription refused: {reply}"));
    }
    loop {
        read_message(&mut stream)?;
        if events.send_blocking(()).is_err() {
            return Ok(());
        }
    }
}

fn query_snapshot(socket: &PathBuf) -> Result<Snapshot> {
    #[derive(Deserialize)]
    struct RawWorkspace {
        name: String,
        #[serde(default)]
        focused: bool,
        #[serde(default)]
        urgent: bool,
    }

    let workspaces: Vec<RawWorkspace> =
        serde_json::from_slice(&request(socket, GET_WORKSPACES, b"")?)?;
    let tree: Value = serde_json::from_slice(&request(socket, GET_TREE, b"")?)?;
    let mut windows = BTreeSet::new();
    collect_windows(&tree, &mut windows);

    Ok(Snapshot {
        workspaces: workspaces
            .into_iter()
            .map(|workspace| Workspace {
                name: workspace.name.into(),
                focused: workspace.focused,
                urgent: workspace.urgent,
            })
            .collect(),
        windows,
    })
}

fn collect_windows(node: &Value, windows: &mut BTreeSet<WindowIdentity>) {
    if node.get("pid").and_then(Value::as_i64).is_some() {
        let app_id = node.get("app_id").and_then(Value::as_str);
        let class = node
            .pointer("/window_properties/class")
            .and_then(Value::as_str);
        match (app_id, class) {
            (Some(app_id), _) if !app_id.is_empty() => {
                windows.insert(WindowIdentity::AppId(app_id.to_string()));
            }
            (_, Some(class)) if !class.is_empty() => {
                windows.insert(WindowIdentity::Class(class.to_string()));
            }
            _ => {}
        }
    }
    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(Value::as_array) {
            for child in children {
                collect_windows(child, windows);
            }
        }
    }
}

fn request(socket: &PathBuf, message_type: u32, payload: &[u8]) -> Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket).context("connect")?;
    write_message(&mut stream, message_type, payload)?;
    let (_, reply) = read_message(&mut stream)?;
    Ok(reply)
}

fn write_message(stream: &mut UnixStream, message_type: u32, payload: &[u8]) -> Result<()> {
    let mut message = Vec::with_capacity(14 + payload.len());
    message.extend_from_slice(MAGIC);
    message.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    message.extend_from_slice(&message_type.to_ne_bytes());
    message.extend_from_slice(payload);
    stream.write_all(&message)?;
    Ok(())
}

fn read_message(stream: &mut UnixStream) -> Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 14];
    stream.read_exact(&mut header)?;
    if &header[..6] != MAGIC {
        return Err(anyhow!("unexpected IPC header"));
    }
    let length = u32::from_ne_bytes(header[6..10].try_into()?) as usize;
    let message_type = u32::from_ne_bytes(header[10..14].try_into()?);
    let mut payload = vec![0u8; length];
    stream.read_exact(&mut payload)?;
    Ok((message_type, payload))
}

fn escape_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.+*?()|[]{}^$\"".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_app_ids_and_x11_classes_from_the_tree() {
        let tree = serde_json::json!({
            "nodes": [{
                "nodes": [
                    { "pid": 10, "app_id": "foot" },
                    { "pid": 11, "app_id": null, "window_properties": { "class": "Gimp" } },
                    { "nodes": [], "floating_nodes": [{ "pid": 12, "app_id": "org.gnome.Nautilus" }] },
                    { "app_id": "not-a-window" }
                ]
            }]
        });
        let mut windows = BTreeSet::new();
        collect_windows(&tree, &mut windows);
        assert_eq!(
            windows.into_iter().collect::<Vec<_>>(),
            vec![
                WindowIdentity::AppId("foot".into()),
                WindowIdentity::AppId("org.gnome.Nautilus".into()),
                WindowIdentity::Class("Gimp".into()),
            ]
        );
    }

    #[test]
    fn window_criteria_match_the_whole_identifier() {
        assert_eq!(
            WindowIdentity::AppId("org.gnome.Nautilus".into()).criteria(),
            r#"[app_id="^org\.gnome\.Nautilus$"]"#
        );
    }
}
