//! `Cmd-K`: every action of the selected item or page, searchable.
//!
//! The panel is GPUI Component's command palette, so its search field, row
//! geometry, keyboard navigation and selection are the ones every palette in
//! the ecosystem has. The launcher adds what its model can express on top:
//! sections, submenus, pictures, shortcuts and destructive actions.

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, Keystroke, ParentElement as _, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    component::{
        ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _,
        command::{Command, CommandGroup, CommandItem, CommandState},
        h_flex,
        kbd::Kbd,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{
    LauncherWindow,
    picture::{PictureSize, picture},
};
use crate::model::{Action, ActionEntry, ActionPanel, ActionStyle, ItemId, Submenu};

/// Where an action panel entry sits: its section and position, and inside a
/// submenu, its position there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EntryPath {
    section: usize,
    entry: usize,
}

/// What the user picked in the panel.
pub(super) enum Choice {
    Perform(Action),
    OpenSubmenu(EntryPath),
}

/// An open action panel: which object's actions it lists, and which submenu
/// is showing. The actions themselves are read from the current model on
/// every render, so a page that rebuilds its model never leaves the panel
/// holding stale callbacks.
pub(super) struct OpenPanel {
    command: Entity<CommandState>,
    /// The item whose actions are listed; `None` for a page's own actions.
    item: Option<ItemId>,
    submenu: Option<EntryPath>,
    /// What had the keyboard before the panel took it, to give it back.
    return_focus: Option<FocusHandle>,
}

impl OpenPanel {
    pub(super) fn new(item: Option<ItemId>, window: &mut Window, cx: &mut App) -> Self {
        let return_focus = window.focused(cx);
        let command = cx.new(|cx| CommandState::new(window, cx));
        command.update(cx, |command, cx| command.focus(window, cx));
        Self {
            command,
            item,
            submenu: None,
            return_focus,
        }
    }

    pub(super) fn return_focus(&self) -> Option<&FocusHandle> {
        self.return_focus.as_ref()
    }

    pub(super) fn item(&self) -> Option<&ItemId> {
        self.item.as_ref()
    }

    pub(super) fn is_in_submenu(&self) -> bool {
        self.submenu.is_some()
    }

    pub(super) fn query_is_empty(&self, cx: &App) -> bool {
        self.command.read(cx).query(cx).is_empty()
    }

    /// Shows a submenu's actions in place of the panel's, with a fresh search.
    pub(super) fn open_submenu(&mut self, path: EntryPath, window: &mut Window, cx: &mut App) {
        self.submenu = Some(path);
        self.reset_query(window, cx);
    }

    /// Returns from a submenu to the panel. Returns whether there was one.
    pub(super) fn close_submenu(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if self.submenu.take().is_none() {
            return false;
        }
        self.reset_query(window, cx);
        true
    }

    fn reset_query(&self, window: &mut Window, cx: &mut App) {
        self.command.update(cx, |command, cx| {
            command.set_query("", window, cx);
            command.set_selected_index(Some(IndexPath::default()), window, cx);
        });
    }

    /// Resolves a palette row to the entry it shows in `panel`.
    pub(super) fn choice(&self, panel: &ActionPanel, index: IndexPath) -> Option<Choice> {
        match self.submenu.and_then(|path| submenu(panel, path)) {
            Some(submenu) => submenu
                .actions()
                .get(index.row)
                .cloned()
                .map(Choice::Perform),
            None => {
                let entry = panel
                    .sections()
                    .get(index.section)?
                    .entries()
                    .get(index.row)?;
                Some(match entry {
                    ActionEntry::Action(action) => Choice::Perform(action.clone()),
                    ActionEntry::Submenu(_) => Choice::OpenSubmenu(EntryPath {
                        section: index.section,
                        entry: index.row,
                    }),
                })
            }
        }
    }

    /// The highlighted row, when it opens a submenu.
    pub(super) fn highlighted_submenu(&self, panel: &ActionPanel, cx: &App) -> Option<EntryPath> {
        if self.submenu.is_some() {
            return None;
        }
        let index = self.command.read(cx).selected_index()?;
        match self.choice(panel, index)? {
            Choice::OpenSubmenu(path) => Some(path),
            Choice::Perform(_) => None,
        }
    }

    /// Draws the panel for `panel`, the actions it currently lists.
    pub(super) fn render(
        &self,
        panel: &ActionPanel,
        launcher: WeakEntity<LauncherWindow>,
        cx: &App,
    ) -> AnyElement {
        let submenu = self.submenu.and_then(|path| submenu(panel, path));
        let title: SharedString = submenu
            .map(|submenu| submenu.title().clone())
            .unwrap_or_else(|| "Actions".into());
        let command = match submenu {
            Some(submenu) => Command::new(&self.command)
                .group(CommandGroup::new().items(submenu.actions().iter().map(action_item))),
            None => {
                panel
                    .sections()
                    .iter()
                    .fold(Command::new(&self.command), |command, section| {
                        command.group(
                            match section.title() {
                                Some(title) => CommandGroup::new().label(title.clone()),
                                None => CommandGroup::new(),
                            }
                            .items(section.entries().iter().map(
                                |entry| match entry {
                                    ActionEntry::Action(action) => action_item(action),
                                    ActionEntry::Submenu(submenu) => submenu_item(submenu),
                                },
                            )),
                        )
                    })
            }
        };

        div()
            .id("action-panel")
            .role(Role::Dialog)
            .aria_label(title.clone())
            .w_80()
            .rounded(cx.theme().radius_lg)
            .shadow_lg()
            .child(
                command
                    .placeholder(match submenu {
                        Some(_) => SharedString::from(format!("Search {title}…")),
                        None => "Search actions…".into(),
                    })
                    .when(submenu.is_some(), |this| {
                        let title = title.clone();
                        this.header(move |_, _, cx| {
                            h_flex()
                                .gap_1()
                                .px_3()
                                .pt_2()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(Icon::new(IconName::ChevronLeft).xsmall())
                                .child(title.clone())
                        })
                    })
                    .on_confirm(move |index, window, cx| {
                        launcher
                            .update(cx, |launcher, cx| {
                                launcher.choose_panel_entry(index, window, cx)
                            })
                            .ok();
                    }),
            )
            .into_any_element()
    }
}

fn submenu(panel: &ActionPanel, path: EntryPath) -> Option<&Submenu> {
    match panel
        .sections()
        .get(path.section)?
        .entries()
        .get(path.entry)?
    {
        ActionEntry::Submenu(submenu) => Some(submenu),
        ActionEntry::Action(_) => None,
    }
}

/// How many entries a panel lists at its top level, submenus included.
pub(super) fn entry_count(panel: &ActionPanel) -> usize {
    panel
        .sections()
        .iter()
        .map(|section| section.entries().len())
        .sum()
}

fn action_item(action: &Action) -> CommandItem {
    let action = action.clone();
    CommandItem::new()
        .label(action.title().clone())
        .child(move |_, cx| {
            let destructive = action.style() == ActionStyle::Destructive;
            entry_row(
                action.image(),
                action.title().clone(),
                action.shortcut(),
                false,
                destructive,
                cx,
            )
        })
}

fn submenu_item(submenu: &Submenu) -> CommandItem {
    let submenu = submenu.clone();
    CommandItem::new()
        .label(submenu.title().clone())
        .keywords(
            submenu
                .actions()
                .iter()
                .map(|action| action.title().clone()),
        )
        .child(move |_, cx| {
            entry_row(
                submenu.image(),
                submenu.title().clone(),
                submenu.shortcut(),
                true,
                false,
                cx,
            )
        })
}

/// One panel row: picture, title, then the shortcut and, for a submenu, the
/// chevron that says it opens rather than performs.
fn entry_row(
    image: Option<&crate::model::Image>,
    title: SharedString,
    shortcut: Option<&SharedString>,
    opens_submenu: bool,
    destructive: bool,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let color = match destructive {
        true => theme.danger,
        false => theme.popover_foreground,
    };
    h_flex()
        .flex_1()
        .min_w_0()
        .gap_2()
        .text_color(color)
        .child(
            div()
                .flex_none()
                .size_4()
                .flex()
                .items_center()
                .justify_center()
                .when_some(image, |this, image| {
                    this.child(picture(
                        image,
                        PictureSize::Row,
                        match destructive {
                            true => theme.danger,
                            false => theme.muted_foreground,
                        },
                        theme,
                    ))
                }),
        )
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when_some(
            shortcut.and_then(|shortcut| Keystroke::parse(shortcut).ok()),
            |this, keystroke| this.child(Kbd::new(keystroke)),
        )
        .when(opens_submenu, |this| {
            this.child(
                Icon::new(IconName::ChevronRight)
                    .xsmall()
                    .text_color(theme.muted_foreground),
            )
        })
        .into_any_element()
}

impl LauncherWindow {
    /// The launcher's end of the panel's `on_confirm`.
    pub(super) fn choose_panel_entry(
        &mut self,
        index: IndexPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.current_panel_actions(window, cx) else {
            return;
        };
        let Some(open) = self.action_panel.as_mut() else {
            return;
        };
        match open.choice(&panel, index) {
            Some(Choice::OpenSubmenu(path)) => {
                open.open_submenu(path, window, cx);
                cx.notify();
            }
            Some(Choice::Perform(action)) => {
                let item = open.item().cloned();
                self.close_action_panel(window, cx);
                self.perform_panel_action(item, action, window, cx);
            }
            None => {}
        }
    }
}
