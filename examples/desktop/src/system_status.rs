//! Battery, network and volume, as the top bar and quick settings show them.
//!
//! The prototype reads `/sys` and drives WirePlumber's `wpctl`. A full shell
//! would subscribe to UPower, NetworkManager and PipeWire over D-Bus instead of
//! polling, but the model's shape stays the same.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use gpui_kit::{App, Context, Task};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const AUDIO_SINK: &str = "@DEFAULT_AUDIO_SINK@";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    percent: u8,
    charging: bool,
}

impl Battery {
    pub fn percent(&self) -> u8 {
        self.percent
    }

    pub fn is_charging(&self) -> bool {
        self.charging
    }
}

/// How the machine reaches the network right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    Wired,
    Wireless,
    #[default]
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    level: f32,
    muted: bool,
}

impl Volume {
    /// The output level, where 1.0 is 100 %.
    pub fn level(&self) -> f32 {
        self.level
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Reading {
    battery: Option<Battery>,
    connection: Connection,
    volume: Option<Volume>,
}

pub struct SystemStatus {
    reading: Reading,
    _poll: Task<()>,
}

impl SystemStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let reading = cx.background_executor().spawn(async { read() }).await;
                let updated = this.update(cx, |this, cx| {
                    if this.reading != reading {
                        this.reading = reading;
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        });
        Self {
            reading: Reading::default(),
            _poll: poll,
        }
    }

    pub fn battery(&self) -> Option<Battery> {
        self.reading.battery
    }

    pub fn connection(&self) -> Connection {
        self.reading.connection
    }

    /// `None` when there is no audio server to control.
    pub fn volume(&self) -> Option<Volume> {
        self.reading.volume
    }

    pub fn set_volume_level(&mut self, level: f32, cx: &mut Context<Self>) {
        let Some(volume) = self.reading.volume.as_mut() else {
            return;
        };
        let level = level.clamp(0.0, 1.0);
        volume.level = level;
        cx.notify();
        run_wpctl(&["set-volume", AUDIO_SINK, &format!("{level:.2}")], cx);
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let Some(volume) = self.reading.volume.as_mut() else {
            return;
        };
        volume.muted = !volume.muted;
        cx.notify();
        run_wpctl(&["set-mute", AUDIO_SINK, "toggle"], cx);
    }
}

fn run_wpctl(args: &[&str], cx: &mut App) {
    let args: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
    cx.background_executor()
        .spawn(async move {
            if let Err(err) = Command::new("wpctl").args(&args).status() {
                eprintln!("desktop: wpctl {} failed: {err}", args.join(" "));
            }
        })
        .detach();
}

fn read() -> Reading {
    Reading {
        battery: read_battery(),
        connection: read_connection(),
        volume: read_volume(),
    }
}

fn read_battery() -> Option<Battery> {
    let entries = std::fs::read_dir("/sys/class/power_supply").ok()?;
    entries.flatten().find_map(|entry| {
        let path = entry.path();
        let kind = std::fs::read_to_string(path.join("type")).ok()?;
        if kind.trim() != "Battery" {
            return None;
        }
        let percent = std::fs::read_to_string(path.join("capacity"))
            .ok()?
            .trim()
            .parse::<u8>()
            .ok()?;
        let status = std::fs::read_to_string(path.join("status")).unwrap_or_default();
        Some(Battery {
            percent: percent.min(100),
            charging: matches!(status.trim(), "Charging" | "Full"),
        })
    })
}

fn read_connection() -> Connection {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Connection::Offline;
    };
    let mut connection = Connection::Offline;
    for entry in entries.flatten() {
        let path = entry.path();
        // Virtual interfaces (loopback, bridges, VPN tunnels) have no device.
        if !path.join("device").exists() || !is_up(&path) {
            continue;
        }
        if path.join("wireless").exists() || path.join("phy80211").exists() {
            connection = Connection::Wireless;
        } else {
            // A cable wins over Wi-Fi, as it does for the default route.
            return Connection::Wired;
        }
    }
    connection
}

fn is_up(interface: &Path) -> bool {
    std::fs::read_to_string(interface.join("operstate")).is_ok_and(|state| state.trim() == "up")
}

fn read_volume() -> Option<Volume> {
    let output = Command::new("wpctl")
        .args(["get-volume", AUDIO_SINK])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_volume(&String::from_utf8_lossy(&output.stdout))
}

/// Parses `wpctl get-volume`, which prints `Volume: 0.40` or
/// `Volume: 0.40 [MUTED]`.
fn parse_volume(text: &str) -> Option<Volume> {
    let rest = text.trim().strip_prefix("Volume:")?.trim();
    let mut parts = rest.split_whitespace();
    let level = parts.next()?.parse::<f32>().ok()?;
    Some(Volume {
        level,
        muted: parts.any(|part| part == "[MUTED]"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wpctl_volume() {
        assert_eq!(
            parse_volume("Volume: 0.40\n"),
            Some(Volume {
                level: 0.4,
                muted: false
            })
        );
        assert_eq!(
            parse_volume("Volume: 1.00 [MUTED]"),
            Some(Volume {
                level: 1.0,
                muted: true
            })
        );
        assert_eq!(parse_volume("Unknown node"), None);
    }
}
