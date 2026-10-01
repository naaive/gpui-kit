use gpui_kit::SharedString;

use super::{Action, Image};

/// What a menu-bar command shows: an icon in the system tray (the menu bar on
/// macOS) and the menu it opens.
///
/// It is not a page: the launcher window never draws it. The tray shows the
/// title beside the icon where the platform can (macOS) and in the tooltip
/// elsewhere.
#[derive(Clone, Debug, Default)]
pub struct MenuBarModel {
    icon: Option<Image>,
    title: Option<SharedString>,
    tooltip: Option<SharedString>,
    loading: bool,
    sections: Vec<MenuBarSection>,
}

impl MenuBarModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_icon(mut self, icon: Image) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn with_loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn with_section(mut self, section: MenuBarSection) -> Self {
        self.sections.push(section);
        self
    }

    /// Adds an entry to the trailing untitled section, creating it if needed.
    #[cfg(test)]
    pub fn with_entry(mut self, entry: MenuBarEntry) -> Self {
        match self.sections.last_mut() {
            Some(section) if section.title.is_none() => section.entries.push(entry),
            _ => self.sections.push(MenuBarSection::new().with_entry(entry)),
        }
        self
    }

    pub fn icon(&self) -> Option<&Image> {
        self.icon.as_ref()
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn tooltip(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn sections(&self) -> &[MenuBarSection] {
        &self.sections
    }
}

/// A group of menu entries, separated from the others and optionally titled.
#[derive(Clone, Debug, Default)]
pub struct MenuBarSection {
    title: Option<SharedString>,
    entries: Vec<MenuBarEntry>,
}

impl MenuBarSection {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_entry(mut self, entry: MenuBarEntry) -> Self {
        self.entries.push(entry);
        self
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn entries(&self) -> &[MenuBarEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug)]
pub enum MenuBarEntry {
    Item(MenuBarItem),
    /// A line between groups of a submenu.
    Separator,
    Submenu {
        title: SharedString,
        entries: Vec<MenuBarEntry>,
    },
}

/// One line of the menu. Without an action it is drawn disabled, as a
/// label.
#[derive(Clone, Debug)]
pub struct MenuBarItem {
    title: SharedString,
    subtitle: Option<SharedString>,
    checked: Option<bool>,
    action: Option<Box<Action>>,
}

impl MenuBarItem {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            checked: None,
            action: None,
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Draws a check mark beside the item, or leaves room for one.
    pub fn with_checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.action = Some(Box::new(action));
        self
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn subtitle(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    pub fn checked(&self) -> Option<bool> {
        self.checked
    }

    pub fn action(&self) -> Option<&Action> {
        self.action.as_deref()
    }
}
