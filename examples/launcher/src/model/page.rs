use gpui_kit::SharedString;

use super::Action;

/// One screen of the launcher.
#[derive(Clone, Debug)]
pub enum PageModel {
    List(ListModel),
    /// The page could not produce a model; `message` says why.
    Failure {
        title: SharedString,
        message: SharedString,
    },
}

impl PageModel {
    pub fn failure(title: impl Into<SharedString>, message: impl Into<SharedString>) -> Self {
        Self::Failure {
            title: title.into(),
            message: message.into(),
        }
    }
}

/// Identifies an item across renders.
///
/// Selection is kept by id rather than by position, so a page that reloads its
/// results keeps the item the user was on.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ItemId(SharedString);

impl ItemId {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A searchable, selectable list of items, optionally grouped into sections.
#[derive(Clone, Debug, Default)]
pub struct ListModel {
    sections: Vec<Section>,
    placeholder: Option<SharedString>,
    loading: bool,
    filtering: bool,
    empty_title: Option<SharedString>,
}

impl ListModel {
    pub fn new() -> Self {
        Self {
            filtering: true,
            ..Self::default()
        }
    }

    pub fn with_section(mut self, section: Section) -> Self {
        self.sections.push(section);
        self
    }

    /// Adds an item to the trailing untitled section, creating it if needed.
    pub fn with_item(mut self, item: Item) -> Self {
        match self.sections.last_mut() {
            Some(section) if section.title.is_none() => section.items.push(item),
            _ => self.sections.push(Section::new().with_item(item)),
        }
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn with_loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// Whether the launcher filters the items by the query. A page that
    /// searches for itself, such as a remote search, turns this off.
    pub fn with_filtering(mut self, filtering: bool) -> Self {
        self.filtering = filtering;
        self
    }

    pub fn with_empty_title(mut self, title: impl Into<SharedString>) -> Self {
        self.empty_title = Some(title.into());
        self
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn placeholder(&self) -> Option<&SharedString> {
        self.placeholder.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn is_filtering(&self) -> bool {
        self.filtering
    }

    pub fn empty_title(&self) -> Option<&SharedString> {
        self.empty_title.as_ref()
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.sections
            .iter()
            .flat_map(|section| section.items.iter())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Section {
    title: Option<SharedString>,
    items: Vec<Item>,
}

impl Section {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn with_item(mut self, item: Item) -> Self {
        self.items.push(item);
        self
    }

    pub fn with_items(mut self, items: impl IntoIterator<Item = Item>) -> Self {
        self.items.extend(items);
        self
    }

    pub fn title(&self) -> Option<&SharedString> {
        self.title.as_ref()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    id: ItemId,
    title: SharedString,
    subtitle: Option<SharedString>,
    icon: Option<SharedString>,
    accessory: Option<SharedString>,
    keywords: Vec<SharedString>,
    actions: Vec<Action>,
}

impl Item {
    pub fn new(id: ItemId, title: impl Into<SharedString>) -> Self {
        Self {
            id,
            title: title.into(),
            subtitle: None,
            icon: None,
            accessory: None,
            keywords: Vec::new(),
            actions: Vec::new(),
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// A Lucide icon name, such as `globe`.
    pub fn with_icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Short trailing text, such as a count or a kind.
    pub fn with_accessory(mut self, accessory: impl Into<SharedString>) -> Self {
        self.accessory = Some(accessory.into());
        self
    }

    /// Extra words the item matches, without showing them.
    pub fn with_keyword(mut self, keyword: impl Into<SharedString>) -> Self {
        self.keywords.push(keyword.into());
        self
    }

    pub fn with_action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    pub fn id(&self) -> &ItemId {
        &self.id
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn subtitle(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    pub fn icon(&self) -> Option<&SharedString> {
        self.icon.as_ref()
    }

    pub fn accessory(&self) -> Option<&SharedString> {
        self.accessory.as_ref()
    }

    pub fn keywords(&self) -> &[SharedString] {
        &self.keywords
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    pub fn primary_action(&self) -> Option<&Action> {
        self.actions.first()
    }

    pub fn secondary_action(&self) -> Option<&Action> {
        self.actions.get(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effect;

    #[test]
    fn test_list_model_builder() {
        let list = ListModel::new()
            .with_placeholder("Search")
            .with_item(Item::new(ItemId::new("a"), "Alpha"))
            .with_item(Item::new(ItemId::new("b"), "Beta"))
            .with_section(
                Section::new()
                    .with_title("More")
                    .with_item(Item::new(ItemId::new("c"), "Gamma")),
            )
            .with_item(Item::new(ItemId::new("d"), "Delta"));

        assert!(list.is_filtering(), "a list filters unless it opts out");
        assert_eq!(list.placeholder().map(|p| p.as_ref()), Some("Search"));
        // Loose items join the untitled section they follow, and start a new
        // one after a titled section rather than joining it.
        assert_eq!(list.sections().len(), 3);
        assert_eq!(list.sections()[0].items().len(), 2);
        assert_eq!(
            list.items()
                .map(|item| item.id().as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
    }

    #[test]
    fn test_item_primary_and_secondary_actions_follow_order() {
        let item = Item::new(ItemId::new("x"), "X")
            .with_action(Action::new(
                "Open",
                Effect::OpenUrl("https://gpui-kit.com".into()),
            ))
            .with_action(Action::new("Copy", Effect::Copy("x".into())));

        assert_eq!(item.primary_action().unwrap().title().as_ref(), "Open");
        assert_eq!(item.secondary_action().unwrap().title().as_ref(), "Copy");
    }
}
