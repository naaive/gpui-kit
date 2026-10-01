//! Snip as a desktop citizen: one process per user that lives in the tray,
//! answers system-wide shortcuts and `snip …` commands, and keeps settings.
//!
//! Everything platform-specific about being an application lives here; the
//! platform-specific parts of capturing live in `capture`.

pub mod cli;
pub mod hotkey;
pub mod hud;
pub mod ipc;
pub mod platform;
pub mod settings;
pub mod tray;

use std::path::PathBuf;

/// Snip's per-user data directory, which holds `settings.json`.
pub fn data_directory() -> Option<PathBuf> {
    dirs::data_dir().map(|directory| directory.join("gpui-kit-snip"))
}
