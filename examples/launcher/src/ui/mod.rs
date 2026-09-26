//! Drawing the current page. Nothing here knows extensions exist.

mod footer;
mod launcher_window;
mod list_view;

pub use launcher_window::LauncherWindow;

use gpui_kit::{App, KeyBinding, actions};

pub(crate) const CONTEXT: &str = "Launcher";

actions!(
    launcher,
    [
        /// Moves the selection up.
        SelectPrevious,
        /// Moves the selection down.
        SelectNext,
        /// Performs the selected item's primary action.
        Confirm,
        /// Performs the selected item's secondary action.
        ConfirmSecondary,
        /// Clears the search, then returns to the previous page.
        Back,
    ]
);

pub fn init(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("ctrl-p", SelectPrevious, context),
        KeyBinding::new("ctrl-n", SelectNext, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("secondary-enter", ConfirmSecondary, context),
        KeyBinding::new("escape", Back, context),
    ]);
}
