//! Platform services with a defined fallback where a platform lacks them.

use gpui_kit::{App, SharedString};

/// Pastes the clipboard into the application that was frontmost before the
/// launcher. The text is already on the clipboard; where synthesizing the
/// paste keystroke is not possible, it stays there for the user to paste.
pub fn paste_into_previous_application(_cx: &mut App) {}

/// Shows a short message after the launcher window hides.
pub fn show_hud(text: SharedString, _cx: &mut App) {
    tracing::info!("{text}");
}
