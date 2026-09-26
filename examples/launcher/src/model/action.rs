use std::{fmt, rc::Rc};

use gpui_kit::{App, SharedString, Window};

use crate::extensions::CommandId;

/// Something the user can do to an item.
///
/// The first action of an item is its primary action (`Enter`) and the second
/// its secondary action (`Cmd/Ctrl-Enter`); that order is the whole contract,
/// so an author never marks an action as primary.
#[derive(Clone, Debug)]
pub struct Action {
    title: SharedString,
    shortcut: Option<SharedString>,
    effect: Effect,
}

impl Action {
    pub fn new(title: impl Into<SharedString>, effect: Effect) -> Self {
        Self {
            title: title.into(),
            shortcut: None,
            effect,
        }
    }

    /// A GPUI keystroke, such as `cmd-shift-c`.
    pub fn with_shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn shortcut(&self) -> Option<&SharedString> {
        self.shortcut.as_ref()
    }

    pub fn effect(&self) -> &Effect {
        &self.effect
    }
}

/// What performing an [`Action`] does.
///
/// Everything except [`Effect::Run`] is carried out by the launcher itself, so
/// an extension that opens a link or copies text needs no capability for it and
/// never runs code to do it.
#[derive(Clone, Debug)]
pub enum Effect {
    OpenUrl(SharedString),
    Copy(SharedString),
    ShowToast(ToastStyle, SharedString),
    /// Opens a command, pushing its page.
    Launch(CommandId),
    Pop,
    CloseWindow,
    /// Calls back into the page that produced the action.
    Run(RunHandler),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToastStyle {
    #[default]
    Info,
    Success,
    Failure,
}

/// A callback owned by the page that produced it.
#[derive(Clone)]
pub struct RunHandler(Rc<dyn Fn(&mut Window, &mut App)>);

impl RunHandler {
    pub fn new(handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self(Rc::new(handler))
    }

    pub fn run(&self, window: &mut Window, cx: &mut App) {
        (self.0)(window, cx)
    }
}

impl fmt::Debug for RunHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RunHandler")
    }
}
