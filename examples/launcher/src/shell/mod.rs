//! The launcher as a desktop citizen: one process per user, one window that is
//! summoned and dismissed, and the platform services the pages rely on.
//!
//! Everything platform-specific lives behind this module so the rest of the
//! launcher states intent ("paste into the previous application") and never
//! branches on the operating system.

pub mod app_tray;
pub mod background;
pub mod backup;
pub mod cli;
pub mod deeplink;
pub mod hotkey;
pub mod ipc;
pub mod launcher;
pub mod platform;
pub mod settings;
pub mod tray;

use std::path::PathBuf;

/// The launcher's per-user data directory, which holds `settings.json`.
pub fn data_directory() -> Option<PathBuf> {
    dirs::data_dir().map(|directory| directory.join("gpui-kit-launcher"))
}
