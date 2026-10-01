use std::path::PathBuf;

use gpui_kit::SharedString;

use super::{FormHandler, Image, PushHandler, RunHandler};
use crate::extensions::LaunchRequest;

/// Something the user can do to an item, a detail page or a form.
///
/// The first action of a panel is its primary action (`Enter`) and the second
/// its secondary action (`Cmd/Ctrl-Enter`); that order is the whole contract,
/// so an author never marks an action as primary.
#[derive(Clone, Debug)]
pub struct Action {
    title: SharedString,
    image: Option<Image>,
    shortcut: Option<SharedString>,
    style: ActionStyle,
    effect: Effect,
}

impl Action {
    pub fn new(title: impl Into<SharedString>, effect: Effect) -> Self {
        Self {
            title: title.into(),
            image: None,
            shortcut: None,
            style: ActionStyle::Default,
            effect,
        }
    }

    pub fn with_image(mut self, image: Image) -> Self {
        self.image = Some(image);
        self
    }

    /// A GPUI keystroke, such as `cmd-shift-c` or `secondary-shift-c`.
    pub fn with_shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn with_style(mut self, style: ActionStyle) -> Self {
        self.style = style;
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn shortcut(&self) -> Option<&SharedString> {
        self.shortcut.as_ref()
    }

    pub fn style(&self) -> ActionStyle {
        self.style
    }

    pub fn effect(&self) -> &Effect {
        &self.effect
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionStyle {
    #[default]
    Default,
    /// Deletes or discards something; drawn as destructive.
    Destructive,
}

/// The actions available on one item, page or form, shown by `Cmd-K`.
#[derive(Clone, Debug, Default)]
pub struct ActionPanel {
    sections: Vec<ActionSection>,
}

impl ActionPanel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_section(mut self, section: ActionSection) -> Self {
        self.sections.push(section);
        self
    }

    /// Adds an action to the trailing untitled section, creating it if needed.
    pub fn with_action(mut self, action: Action) -> Self {
        self.untitled_tail()
            .entries
            .push(ActionEntry::Action(action));
        self
    }

    pub fn with_submenu(mut self, submenu: Submenu) -> Self {
        self.untitled_tail()
            .entries
            .push(ActionEntry::Submenu(submenu));
        self
    }

    fn untitled_tail(&mut self) -> &mut ActionSection {
        if !matches!(self.sections.last(), Some(section) if section.title.is_none()) {
            self.sections.push(ActionSection::new());
        }
        self.sections.last_mut().expect("just ensured")
    }

    pub fn sections(&self) -> &[ActionSection] {
        &self.sections
    }

    /// Every top-level action, in order, skipping submenus.
    pub fn actions(&self) -> impl Iterator<Item = &Action> {
        self.sections
            .iter()
            .flat_map(|section| section.entries.iter())
            .filter_map(|entry| match entry {
                ActionEntry::Action(action) => Some(action),
                ActionEntry::Submenu(_) => None,
            })
    }

    /// Every action including those inside submenus, for shortcut matching.
    pub fn all_actions(&self) -> impl Iterator<Item = &Action> {
        self.sections
            .iter()
            .flat_map(|section| section.entries.iter())
            .flat_map(|entry| match entry {
                ActionEntry::Action(action) => std::slice::from_ref(action).iter(),
                ActionEntry::Submenu(submenu) => submenu.actions.iter(),
            })
    }

    pub fn primary(&self) -> Option<&Action> {
        self.actions().next()
    }

    pub fn secondary(&self) -> Option<&Action> {
        self.actions().nth(1)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ActionSection {
    title: Option<SharedString>,
    entries: Vec<ActionEntry>,
}

impl ActionSection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_entry(mut self, entry: ActionEntry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn entries(&self) -> &[ActionEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug)]
pub enum ActionEntry {
    Action(Action),
    Submenu(Submenu),
}

/// A nested list of actions, such as "Set Priority ▸".
#[derive(Clone, Debug)]
pub struct Submenu {
    title: SharedString,
    image: Option<Image>,
    shortcut: Option<SharedString>,
    actions: Vec<Action>,
}

impl Submenu {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            image: None,
            shortcut: None,
            actions: Vec::new(),
        }
    }

    pub fn with_image(mut self, image: Image) -> Self {
        self.image = Some(image);
        self
    }

    pub fn with_shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn shortcut(&self) -> Option<&SharedString> {
        self.shortcut.as_ref()
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }
}

/// What performing an [`Action`] does.
///
/// Everything except the handler-carrying variants is carried out by the
/// launcher itself, so an extension that opens a link or copies text needs no
/// capability for it and never runs code to do it.
#[derive(Clone, Debug)]
pub enum Effect {
    OpenUrl(SharedString),
    /// Opens a file, folder or application with the system default.
    OpenPath(PathBuf),
    /// Shows a file in the file manager.
    RevealPath(PathBuf),
    /// Opens a file, folder or URL with a given application: its path, or
    /// its name as the system knows it.
    OpenWith {
        target: SharedString,
        application: SharedString,
    },
    /// Moves files to the Trash (the Recycle Bin on Windows).
    Trash(Vec<PathBuf>),
    /// Shows a file large, as Quick Look does.
    QuickLook(PathBuf),
    /// Opens the preferences of a command's extension, and of the command.
    OpenPreferences(crate::extensions::CommandId),
    /// Opens the Create Quicklink form filled in.
    CreateQuicklink {
        name: SharedString,
        link: SharedString,
    },
    /// Opens the Create Snippet form filled in.
    CreateSnippet {
        name: SharedString,
        text: SharedString,
    },
    Copy(SharedString),
    /// Pastes text into the application that was frontmost before the
    /// launcher, then hides the launcher.
    Paste(SharedString),
    /// Copies a secret: left out of Clipboard History and cleared from the
    /// clipboard after a while.
    CopyConcealed(SharedString),
    /// Pastes a secret into the previous application, as
    /// [`Effect::CopyConcealed`] copies it.
    PasteConcealed(SharedString),
    /// Puts anything on the clipboard: an image, files, or text.
    CopyItem(gpui_kit::ClipboardItem),
    /// Pastes anything into the previous application, as [`Effect::Paste`].
    PasteItem(gpui_kit::ClipboardItem),
    ShowToast(Toast),
    /// A short message shown after the launcher window hides.
    ShowHud(SharedString),
    /// Opens a command, pushing its page (or running it, for a no-view command).
    Launch(LaunchRequest),
    /// Pushes a page built by the page that produced the action.
    Push(PushHandler),
    Pop,
    PopToRoot,
    CloseWindow,
    /// Asks before performing `effect`.
    Confirm(Confirmation),
    /// Collects the current form's values and hands them over.
    SubmitForm(FormHandler),
    /// Calls back into the page that produced the action.
    Run(RunHandler),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToastStyle {
    #[default]
    Info,
    Success,
    Failure,
    /// Work in progress; a later toast with the same id replaces it.
    Progress,
}

#[derive(Clone, Debug)]
pub struct Toast {
    style: ToastStyle,
    title: SharedString,
    message: Option<SharedString>,
    /// Toasts with the same id replace each other, so a progress toast can
    /// turn into a success or failure one.
    id: Option<SharedString>,
    /// A button on the toast, and what pressing it does.
    action: Option<Box<(SharedString, RunHandler)>>,
    /// Called when the toast goes away without its button being pressed.
    on_dismiss: Option<RunHandler>,
}

impl Toast {
    pub fn new(style: ToastStyle, title: impl Into<SharedString>) -> Self {
        Self {
            style,
            title: title.into(),
            message: None,
            id: None,
            action: None,
            on_dismiss: None,
        }
    }

    pub fn with_action(mut self, title: impl Into<SharedString>, handler: RunHandler) -> Self {
        self.action = Some(Box::new((title.into(), handler)));
        self
    }

    pub fn with_on_dismiss(mut self, handler: RunHandler) -> Self {
        self.on_dismiss = Some(handler);
        self
    }

    pub fn action(&self) -> Option<&(SharedString, RunHandler)> {
        self.action.as_deref()
    }

    pub fn on_dismiss(&self) -> Option<&RunHandler> {
        self.on_dismiss.as_ref()
    }

    pub fn with_message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn style(&self) -> ToastStyle {
        self.style
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn message(&self) -> Option<&SharedString> {
        self.message.as_ref()
    }

    pub fn id(&self) -> Option<&SharedString> {
        self.id.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct Confirmation {
    title: SharedString,
    message: Option<SharedString>,
    confirm_title: SharedString,
    destructive: bool,
    effect: Box<Effect>,
    /// Called when the user cancels instead.
    on_cancel: Option<RunHandler>,
}

impl Confirmation {
    pub fn new(title: impl Into<SharedString>, effect: Effect) -> Self {
        Self {
            title: title.into(),
            message: None,
            confirm_title: "Confirm".into(),
            destructive: false,
            effect: Box::new(effect),
            on_cancel: None,
        }
    }

    pub fn with_on_cancel(mut self, handler: RunHandler) -> Self {
        self.on_cancel = Some(handler);
        self
    }

    pub fn on_cancel(&self) -> Option<&RunHandler> {
        self.on_cancel.as_ref()
    }

    pub fn with_message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn with_confirm_title(mut self, title: impl Into<SharedString>) -> Self {
        self.confirm_title = title.into();
        self
    }

    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn message(&self) -> Option<&SharedString> {
        self.message.as_ref()
    }

    pub fn confirm_title(&self) -> &SharedString {
        &self.confirm_title
    }

    pub fn is_destructive(&self) -> bool {
        self.destructive
    }

    pub fn effect(&self) -> &Effect {
        &self.effect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_panel_builder_and_order() {
        let panel = ActionPanel::new()
            .with_action(Action::new(
                "Open",
                Effect::OpenUrl("https://gpui-kit.com".into()),
            ))
            .with_submenu(Submenu::new("Priority").with_action(Action::new("High", Effect::Pop)))
            .with_action(Action::new("Copy", Effect::Copy("x".into())))
            .with_section(ActionSection::new().with_title("Danger").with_entry(
                ActionEntry::Action(
                    Action::new("Delete", Effect::Pop).with_style(ActionStyle::Destructive),
                ),
            ));

        assert_eq!(panel.sections().len(), 2);
        assert_eq!(panel.primary().unwrap().title().as_ref(), "Open");
        // Submenus are not primary or secondary; they are opened, not performed.
        assert_eq!(panel.secondary().unwrap().title().as_ref(), "Copy");
        assert_eq!(
            panel
                .all_actions()
                .map(|action| action.title().to_string())
                .collect::<Vec<_>>(),
            ["Open", "High", "Copy", "Delete"]
        );
    }
}
