use gpui_kit::SharedString;

use super::{ActionPanel, Choice, DetailModel, FormModel, Image, RunHandler, TextHandler, Tone};

/// One screen of the launcher.
#[derive(Clone, Debug)]
pub enum PageModel {
    /// A searchable list or grid of items.
    List(ListModel),
    /// One object in full.
    Detail(DetailModel),
    /// Fields to fill in and submit.
    Form(FormModel),
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

    /// The panel `Cmd-K` shows when nothing is selected, or the page has no
    /// items: a detail's or a form's own actions.
    pub fn page_actions(&self) -> Option<&ActionPanel> {
        match self {
            Self::Detail(detail) => Some(detail.actions()),
            Self::Form(form) => Some(form.actions()),
            Self::List(_) | Self::Failure { .. } => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        match self {
            Self::List(list) => list.is_loading(),
            Self::Detail(detail) => detail.is_loading(),
            Self::Form(form) => form.is_loading(),
            Self::Failure { .. } => false,
        }
    }
}

impl From<ListModel> for PageModel {
    fn from(list: ListModel) -> Self {
        Self::List(list)
    }
}

impl From<DetailModel> for PageModel {
    fn from(detail: DetailModel) -> Self {
        Self::Detail(detail)
    }
}

impl From<FormModel> for PageModel {
    fn from(form: FormModel) -> Self {
        Self::Form(form)
    }
}

/// Identifies an item across renders.
///
/// Selection is kept by id rather than by position, so a page that reloads its
/// results keeps the item the user was on.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ItemId(SharedString);

impl ItemId {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_shared(&self) -> &SharedString {
        &self.0
    }
}

/// How a list arranges its items.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Layout {
    #[default]
    List,
    /// Square cells, `columns` per row; for images, emoji, colors.
    Grid { columns: u8 },
}

/// A searchable, selectable collection of items, optionally grouped into
/// sections and drawn as a list or a grid.
#[derive(Clone, Debug, Default)]
pub struct ListModel {
    sections: Vec<Section>,
    layout: Layout,
    placeholder: Option<SharedString>,
    loading: bool,
    filtering: bool,
    showing_detail: bool,
    empty_title: Option<SharedString>,
    empty_description: Option<SharedString>,
    dropdown: Option<Dropdown>,
    selected: Option<ItemId>,
    search_text: Option<SharedString>,
    on_query_change: Option<TextHandler>,
    on_selection_change: Option<TextHandler>,
    on_load_more: Option<RunHandler>,
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

    pub fn with_layout(mut self, layout: Layout) -> Self {
        self.layout = layout;
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

    /// Shows the selected item's detail beside the list.
    pub fn with_showing_detail(mut self, showing_detail: bool) -> Self {
        self.showing_detail = showing_detail;
        self
    }

    pub fn with_empty_title(mut self, title: impl Into<SharedString>) -> Self {
        self.empty_title = Some(title.into());
        self
    }

    pub fn with_empty_description(mut self, description: impl Into<SharedString>) -> Self {
        self.empty_description = Some(description.into());
        self
    }

    /// A filter beside the search field, such as "My Repositories".
    pub fn with_dropdown(mut self, dropdown: Dropdown) -> Self {
        self.dropdown = Some(dropdown);
        self
    }

    /// Asks the launcher to select this item. The user's own selection wins
    /// afterwards, until the page asks again with a different id.
    pub fn with_selected(mut self, selected: ItemId) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Asks the launcher to put this text in the search field, once per
    /// different text, such as the selected text a command starts from.
    pub fn with_search_text(mut self, text: impl Into<SharedString>) -> Self {
        self.search_text = Some(text.into());
        self
    }

    pub fn search_text(&self) -> Option<&SharedString> {
        self.search_text.as_ref()
    }

    pub fn with_on_query_change(mut self, handler: TextHandler) -> Self {
        self.on_query_change = Some(handler);
        self
    }

    /// Called with the id of the newly selected item.
    pub fn with_on_selection_change(mut self, handler: TextHandler) -> Self {
        self.on_selection_change = Some(handler);
        self
    }

    /// Called when the selection nears the end, to load the next page.
    pub fn with_on_load_more(mut self, handler: RunHandler) -> Self {
        self.on_load_more = Some(handler);
        self
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn layout(&self) -> Layout {
        self.layout
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

    pub fn is_showing_detail(&self) -> bool {
        self.showing_detail
    }

    pub fn empty_title(&self) -> Option<&SharedString> {
        self.empty_title.as_ref()
    }

    pub fn empty_description(&self) -> Option<&SharedString> {
        self.empty_description.as_ref()
    }

    pub fn dropdown(&self) -> Option<&Dropdown> {
        self.dropdown.as_ref()
    }

    pub fn selected(&self) -> Option<&ItemId> {
        self.selected.as_ref()
    }

    pub fn on_query_change(&self) -> Option<&TextHandler> {
        self.on_query_change.as_ref()
    }

    pub fn on_selection_change(&self) -> Option<&TextHandler> {
        self.on_selection_change.as_ref()
    }

    pub fn on_load_more(&self) -> Option<&RunHandler> {
        self.on_load_more.as_ref()
    }

    /// Every item, in order, across sections.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.sections
            .iter()
            .flat_map(|section| section.items.iter())
    }
}

/// A choice beside the search field that narrows a list.
#[derive(Clone, Debug)]
pub struct Dropdown {
    tooltip: SharedString,
    choices: Vec<Choice>,
    value: Option<SharedString>,
    on_change: Option<TextHandler>,
}

impl Dropdown {
    pub fn new(tooltip: impl Into<SharedString>) -> Self {
        Self {
            tooltip: tooltip.into(),
            choices: Vec::new(),
            value: None,
            on_change: None,
        }
    }

    pub fn with_choice(mut self, choice: Choice) -> Self {
        self.choices.push(choice);
        self
    }

    pub fn with_value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn with_on_change(mut self, handler: TextHandler) -> Self {
        self.on_change = Some(handler);
        self
    }

    pub fn tooltip(&self) -> &SharedString {
        &self.tooltip
    }

    pub fn choices(&self) -> &[Choice] {
        &self.choices
    }

    pub fn value(&self) -> Option<&SharedString> {
        self.value.as_ref()
    }

    pub fn on_change(&self) -> Option<&TextHandler> {
        self.on_change.as_ref()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Section {
    title: Option<SharedString>,
    subtitle: Option<SharedString>,
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

    pub fn with_subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
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

    pub fn subtitle(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

/// Trailing information on a row: text, a date, a tag, with an optional icon.
#[derive(Clone, Debug, PartialEq)]
pub struct Accessory {
    text: Option<SharedString>,
    image: Option<Image>,
    tone: Option<Tone>,
    tooltip: Option<SharedString>,
}

impl Accessory {
    pub fn text(text: impl Into<SharedString>) -> Self {
        Self {
            text: Some(text.into()),
            image: None,
            tone: None,
            tooltip: None,
        }
    }

    /// Text drawn as a tag in the given tone.
    pub fn tag(text: impl Into<SharedString>, tone: Tone) -> Self {
        Self {
            tone: Some(tone),
            ..Self::text(text)
        }
    }

    pub fn image(image: Image) -> Self {
        Self {
            text: None,
            image: Some(image),
            tone: None,
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn label(&self) -> Option<&SharedString> {
        self.text.as_ref()
    }

    pub fn picture(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    /// `Some` when drawn as a tag.
    pub fn tone(&self) -> Option<Tone> {
        self.tone
    }

    pub fn tooltip(&self) -> Option<&SharedString> {
        self.tooltip.as_ref()
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    id: ItemId,
    title: SharedString,
    subtitle: Option<SharedString>,
    image: Option<Image>,
    accessories: Vec<Accessory>,
    keywords: Vec<SharedString>,
    detail: Option<DetailModel>,
    actions: ActionPanel,
}

impl Item {
    pub fn new(id: ItemId, title: impl Into<SharedString>) -> Self {
        Self {
            id,
            title: title.into(),
            subtitle: None,
            image: None,
            accessories: Vec::new(),
            keywords: Vec::new(),
            detail: None,
            actions: ActionPanel::new(),
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn with_image(mut self, image: Image) -> Self {
        self.image = Some(image);
        self
    }

    /// A Lucide icon; shorthand for `with_image(Image::Icon(..))`.
    pub fn with_icon(self, icon: impl Into<SharedString>) -> Self {
        self.with_image(Image::Icon(icon.into()))
    }

    pub fn with_accessory(mut self, accessory: Accessory) -> Self {
        self.accessories.push(accessory);
        self
    }

    /// Puts an accessory before the others, such as a user's alias.
    pub fn with_leading_accessory(mut self, accessory: Accessory) -> Self {
        self.accessories.insert(0, accessory);
        self
    }

    /// Extra words the item matches, without showing them.
    pub fn with_keyword(mut self, keyword: impl Into<SharedString>) -> Self {
        self.keywords.push(keyword.into());
        self
    }

    /// Shown beside the list when the list is showing details.
    pub fn with_detail(mut self, detail: DetailModel) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn with_actions(mut self, actions: ActionPanel) -> Self {
        self.actions = actions;
        self
    }

    /// Appends an action to the item's panel.
    pub fn with_action(mut self, action: super::Action) -> Self {
        self.actions = self.actions.with_action(action);
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

    pub fn image(&self) -> Option<&Image> {
        self.image.as_ref()
    }

    pub fn accessories(&self) -> &[Accessory] {
        &self.accessories
    }

    pub fn keywords(&self) -> &[SharedString] {
        &self.keywords
    }

    pub fn detail(&self) -> Option<&DetailModel> {
        self.detail.as_ref()
    }

    pub fn actions(&self) -> &ActionPanel {
        &self.actions
    }

    pub fn primary_action(&self) -> Option<&super::Action> {
        self.actions.primary()
    }

    pub fn secondary_action(&self) -> Option<&super::Action> {
        self.actions.secondary()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Effect};

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
            .with_item(Item::new(ItemId::new("d"), "Delta"))
            .with_layout(Layout::Grid { columns: 5 });

        assert!(list.is_filtering(), "a list filters unless it opts out");
        assert_eq!(list.layout(), Layout::Grid { columns: 5 });
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
