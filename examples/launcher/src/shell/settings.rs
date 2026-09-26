//! The launcher's own settings: the summon shortcut, the theme, where
//! extensions are loaded from.

use anyhow::Result;
use gpui_kit::{App, Window};

use crate::pages::PageHandle;

/// Builds the settings page.
pub fn settings_page(_window: &mut Window, _cx: &mut App) -> Result<PageHandle> {
    anyhow::bail!("settings are not available in this build")
}
