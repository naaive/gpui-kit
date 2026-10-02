//! Drawing the current page. Nothing here knows extensions exist.
//!
//! [`LauncherWindow`] owns every interaction: the search field, selection,
//! keyboard, overlays and the effects of actions. The other modules draw one
//! region each from the current [`PageModel`](crate::model::PageModel).

mod action_panel;
mod detail_view;
mod footer;
mod form_view;
pub(crate) mod keycaps;
mod launcher_window;
mod list_view;
pub(crate) mod picture;
mod toast;

pub use launcher_window::{LauncherWindow, Snapshot};

use std::sync::Arc;

use gpui_kit::{App, ElementId, KeyBinding, SharedString, actions};

pub(crate) const CONTEXT: &str = "Launcher";

actions!(
    launcher,
    [
        /// Moves the selection up, or scrolls a detail page up.
        SelectPrevious,
        /// Moves the selection down, or scrolls a detail page down.
        SelectNext,
        /// Performs the selected item's primary action.
        Confirm,
        /// Performs the selected item's secondary action; on a form, submits.
        ConfirmSecondary,
        /// Closes the topmost layer: the action panel, the search text, the
        /// page, and finally the window.
        Back,
        /// Shows or hides the actions of the selected item or page.
        ToggleActions,
        /// Ctrl-P: moves the selection up when the Emacs keys are chosen in
        /// settings.
        EmacsSelectPrevious,
        /// Ctrl-N: moves the selection down when the Emacs keys are chosen.
        EmacsSelectNext,
        /// Ctrl-K: moves the selection up when the Vim keys are chosen.
        VimSelectPrevious,
        /// Ctrl-J: moves the selection down when the Vim keys are chosen.
        VimSelectNext,
    ]
);

/// An element id for something the model identifies by string, such as an
/// item, a field or a stack entry.
pub(crate) fn keyed_id(name: &'static str, key: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(Arc::new(ElementId::from(name)), key.into())
}

/// Both pairs of navigation keys are bound; the one not chosen in settings
/// lets its keystroke through, so switching takes effect without binding
/// keys again.
pub fn init(cx: &mut App) {
    let context = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, context),
        KeyBinding::new("down", SelectNext, context),
        KeyBinding::new("ctrl-p", EmacsSelectPrevious, context),
        KeyBinding::new("ctrl-n", EmacsSelectNext, context),
        KeyBinding::new("ctrl-k", VimSelectPrevious, context),
        KeyBinding::new("ctrl-j", VimSelectNext, context),
        KeyBinding::new("enter", Confirm, context),
        KeyBinding::new("secondary-enter", ConfirmSecondary, context),
        // A multi-line field takes Cmd/Ctrl-Enter for a line break and keeps
        // it, so a form could not be submitted from its text area. In the
        // launcher the key submits, from any field.
        KeyBinding::new(
            "secondary-enter",
            ConfirmSecondary,
            Some("Launcher > Input"),
        ),
        KeyBinding::new("escape", Back, context),
        KeyBinding::new("secondary-k", ToggleActions, context),
    ]);
}
