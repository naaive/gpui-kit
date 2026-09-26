//! `List` and its parts: `ListSection`, `ListItem`, `ListDropdown`,
//! `ListDropdownItem`.
//!
//! A `List` with `grid(columns)` is the grid layout: the same items, drawn as
//! cells. Keeping one node for both means an extension switches layouts
//! without rewriting its items.

use anyhow::{Result, bail};
use gpui_kit::AnyElement;
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDescriptor,
    ComponentMaterializer, ComponentPayload, ComponentRegistry, MaterializeRequest,
    MethodDescriptor, RegistryError,
};

use super::{
    action::{resolve_action, resolve_panel},
    detail::resolve_detail,
    shared::{
        RUN_CALLBACK, TEXT_CALLBACK, bool_method, callback_method, carry, describe, element_method,
        empty_constructor, payload, recorded, reject_style, reporting, resolved, run_handler,
        string_method, strings_constructor, tag_method, taken, text_handler,
    },
};
use crate::model::{
    Accessory, Choice, Dropdown, Image, Item, ItemId, Layout, ListModel, PageModel, Section, Tone,
};

const SELECTION_CALLBACK: &str = "(id: string, cx: Context) => void";
/// Wider than this and a cell is too small to show an icon and its title.
const MAX_GRID_COLUMNS: u8 = 12;

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    registry.register(list())?;
    registry.register(list_section())?;
    registry.register(list_item())?;
    registry.register(list_dropdown())?;
    registry.register(list_dropdown_item())?;
    Ok(())
}

// MARK: List

#[derive(Clone)]
enum ListOp {
    Placeholder(String),
    Loading(bool),
    Filtering(bool),
    EmptyTitle(String),
    EmptyDescription(String),
    ShowingDetail(bool),
    Grid(u8),
    SelectedItem(String),
    Dropdown(ComponentArgument),
    OnQueryChange(ComponentArgument),
    OnSelectionChange(ComponentArgument),
    OnLoadMore(ComponentArgument),
}

fn list() -> ComponentDescriptor {
    ComponentDescriptor::new("List", reporting(ListMaterializer))
        .with_documentation(
            "A searchable list, or grid, of items. Return it from `render`. Children are \
             `ListSection` and `ListItem`.",
        )
        .with_constructors(vec![empty_constructor("List")])
        .with_methods(vec![
            string_method(
                "placeholder",
                "Placeholder of the search field.",
                ListOp::Placeholder,
            ),
            bool_method(
                "loading",
                "Shows that results are on their way.",
                ListOp::Loading,
            ),
            bool_method(
                "filtering",
                "Whether the launcher filters items by the query. Defaults to true, or to \
                 false when `on_query_change` is set.",
                ListOp::Filtering,
            ),
            string_method(
                "empty_title",
                "Title shown when no item matches.",
                ListOp::EmptyTitle,
            ),
            string_method(
                "empty_description",
                "Text under the empty title, such as what to try instead.",
                ListOp::EmptyDescription,
            ),
            bool_method(
                "showing_detail",
                "Shows the selected item's `detail` beside the list.",
                ListOp::ShowingDetail,
            ),
            MethodDescriptor::new(
                "grid",
                vec![ArgumentDescriptor::new("columns", ArgumentSchema::Number)],
                |arguments| match arguments {
                    [ComponentArgument::Number(columns)]
                        if columns.fract() == 0.0
                            && (1.0..=f64::from(MAX_GRID_COLUMNS)).contains(columns) =>
                    {
                        Ok(ComponentPayload::new(ListOp::Grid(*columns as u8)))
                    }
                    _ => Err(format!(
                        "grid expects a whole number of columns from 1 to {MAX_GRID_COLUMNS}"
                    )),
                },
            )
            .with_documentation(
                "Draws the items as a grid of square cells, `columns` per row; for emoji, \
                 colors and images.",
            ),
            string_method(
                "selected_item",
                "Asks the launcher to select the item with this id. The user's own selection \
                 wins afterwards, until a render asks for a different id.",
                ListOp::SelectedItem,
            ),
            element_method(
                "dropdown",
                "dropdown",
                "A `ListDropdown` beside the search field that narrows the list.",
                ListOp::Dropdown,
            ),
            callback_method(
                "on_query_change",
                "Called with the search text whenever it changes. Setting it turns the \
                 launcher's own filtering off unless `filtering(true)` says otherwise.",
                TEXT_CALLBACK,
                ListOp::OnQueryChange,
            ),
            callback_method(
                "on_selection_change",
                "Called with the id of the newly selected item.",
                SELECTION_CALLBACK,
                ListOp::OnSelectionChange,
            ),
            callback_method(
                "on_load_more",
                "Called when the selection nears the end of the list, to load the next page.",
                RUN_CALLBACK,
                ListOp::OnLoadMore,
            ),
        ])
}

struct ListMaterializer;

impl ComponentMaterializer for ListMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "List")?;
        let mut list = ListModel::new();
        let mut filtering = None;
        let mut searches_itself = false;
        for op in recorded::<ListOp>(&request) {
            list = match op {
                ListOp::Placeholder(text) => list.with_placeholder(text),
                ListOp::Loading(loading) => list.with_loading(loading),
                ListOp::Filtering(value) => {
                    filtering = Some(value);
                    list
                }
                ListOp::EmptyTitle(text) => list.with_empty_title(text),
                ListOp::EmptyDescription(text) => list.with_empty_description(text),
                ListOp::ShowingDetail(value) => list.with_showing_detail(value),
                ListOp::Grid(columns) => list.with_layout(Layout::Grid { columns }),
                ListOp::SelectedItem(id) => list.with_selected(ItemId::new(id)),
                ListOp::Dropdown(argument) => list.with_dropdown(
                    resolved(&mut request, &argument, "a ListDropdown")
                        .map_err(|_| anyhow::anyhow!("List.dropdown expects a ListDropdown"))?,
                ),
                ListOp::OnQueryChange(callback) => {
                    searches_itself = true;
                    list.with_on_query_change(text_handler(
                        request.resolve_callback(&callback)?,
                        "List.on_query_change",
                    ))
                }
                ListOp::OnSelectionChange(callback) => list.with_on_selection_change(text_handler(
                    request.resolve_callback(&callback)?,
                    "List.on_selection_change",
                )),
                ListOp::OnLoadMore(callback) => list.with_on_load_more(run_handler(
                    request.resolve_callback(&callback)?,
                    "List.on_load_more",
                )),
            };
        }
        list = list.with_filtering(filtering.unwrap_or(!searches_itself));

        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            list = match name {
                Some("ListSection") => list.with_section(taken(&mut element, "a ListSection")?),
                Some("ListItem") => list.with_item(taken(&mut element, "a ListItem")?),
                other => bail!(
                    "List accepts ListSection and ListItem children, not {}",
                    describe(other)
                ),
            };
        }
        Ok(carry(PageModel::List(list)))
    }
}

// MARK: ListSection

#[derive(Clone)]
struct SectionTitle(String);

#[derive(Clone)]
enum SectionOp {
    Subtitle(String),
}

fn list_section() -> ComponentDescriptor {
    ComponentDescriptor::new("ListSection", reporting(SectionMaterializer))
        .with_documentation("A titled group of `ListItem`s inside a `List`.")
        .with_constructors(vec![strings_constructor(
            "ListSection",
            &["title"],
            &[],
            |mut values| SectionTitle(values.remove(0)),
        )])
        .with_methods(vec![string_method(
            "subtitle",
            "Secondary text after the section title, such as a count.",
            SectionOp::Subtitle,
        )])
}

struct SectionMaterializer;

impl ComponentMaterializer for SectionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ListSection")?;
        let SectionTitle(title) = payload(&request, "ListSection")?;
        let mut section = Section::new().with_title(title);
        for op in recorded::<SectionOp>(&request) {
            section = match op {
                SectionOp::Subtitle(subtitle) => section.with_subtitle(subtitle),
            };
        }
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            match name {
                Some("ListItem") => section = section.with_item(taken(&mut element, "a ListItem")?),
                other => bail!(
                    "ListSection accepts ListItem children, not {}",
                    describe(other)
                ),
            }
        }
        Ok(carry(section))
    }
}

// MARK: ListItem

#[derive(Clone)]
struct ItemHead {
    id: String,
    title: String,
}

#[derive(Clone)]
enum ItemOp {
    Subtitle(String),
    Icon(String),
    Accessory(String),
    AccessoryIcon(String),
    AccessoryTooltip(String),
    Tag(String, Tone),
    Keyword(String),
    Detail(ComponentArgument),
    Actions(ComponentArgument),
    Action(ComponentArgument),
}

fn list_item() -> ComponentDescriptor {
    ComponentDescriptor::new("ListItem", reporting(ItemMaterializer))
        .with_documentation(
            "One row of a `List`, or one cell of a grid. The id must stay the same across \
             renders so the selection follows the item. The first action is the primary \
             action (Enter), the second the secondary one (Cmd/Ctrl-Enter).",
        )
        .with_constructors(vec![strings_constructor(
            "ListItem",
            &["id", "title"],
            &["id"],
            |values| ItemHead {
                id: values[0].clone(),
                title: values[1].clone(),
            },
        )])
        .with_methods(vec![
            string_method(
                "subtitle",
                "Secondary text after the title.",
                ItemOp::Subtitle,
            ),
            string_method(
                "icon",
                "A Lucide icon name such as `globe`, or a path to an image inside the \
                 extension such as `assets/logo.png`.",
                ItemOp::Icon,
            ),
            string_method(
                "accessory",
                "Short trailing text, such as a count or a date.",
                ItemOp::Accessory,
            ),
            string_method(
                "accessory_icon",
                "A trailing icon: a Lucide icon name or an image path.",
                ItemOp::AccessoryIcon,
            ),
            string_method(
                "accessory_tooltip",
                "Explains the accessory, tag or accessory icon added just before, on hover.",
                ItemOp::AccessoryTooltip,
            ),
            tag_method(
                "A trailing tag. `tone` is neutral (the default), accent, success, warning or \
                 danger; the theme decides the color.",
                ItemOp::Tag,
            ),
            string_method(
                "keyword",
                "An extra word the item matches without showing it.",
                ItemOp::Keyword,
            ),
            element_method(
                "detail",
                "detail",
                "A `Detail` shown beside the list while the item is selected and the list is \
                 `showing_detail`.",
                ItemOp::Detail,
            ),
            element_method(
                "actions",
                "panel",
                "The item's `ActionPanel`, replacing any actions added so far.",
                ItemOp::Actions,
            ),
            element_method(
                "action",
                "action",
                "Appends an `Action` to the item's panel. Order matters: first is primary.",
                ItemOp::Action,
            ),
        ])
}

struct ItemMaterializer;

impl ComponentMaterializer for ItemMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ListItem")?;
        let ItemHead { id, title } = payload(&request, "ListItem")?;
        let mut item = Item::new(ItemId::new(id), title);
        // Collected first, because a tooltip applies to the accessory before it.
        let mut accessories: Vec<Accessory> = Vec::new();
        for op in recorded::<ItemOp>(&request) {
            item = match op {
                ItemOp::Subtitle(text) => item.with_subtitle(text),
                ItemOp::Icon(icon) => item.with_image(Image::parse(&icon)),
                ItemOp::Accessory(text) => {
                    accessories.push(Accessory::text(text));
                    item
                }
                ItemOp::AccessoryIcon(icon) => {
                    accessories.push(Accessory::image(Image::parse(&icon)));
                    item
                }
                ItemOp::AccessoryTooltip(tooltip) => {
                    let accessory = accessories.pop().ok_or_else(|| {
                        anyhow::anyhow!(
                            "ListItem `{}`: accessory_tooltip must follow an accessory, tag or \
                             accessory_icon",
                            item.id().as_str()
                        )
                    })?;
                    accessories.push(accessory.with_tooltip(tooltip));
                    item
                }
                ItemOp::Tag(text, tone) => {
                    accessories.push(Accessory::tag(text, tone));
                    item
                }
                ItemOp::Keyword(word) => item.with_keyword(word),
                ItemOp::Detail(detail) => item.with_detail(resolve_detail(&mut request, &detail)?),
                ItemOp::Actions(panel) => {
                    item.with_actions(resolve_panel(&mut request, &panel, "ListItem")?)
                }
                ItemOp::Action(action) => {
                    item.with_action(resolve_action(&mut request, &action, "ListItem")?)
                }
            };
        }
        Ok(carry(
            accessories
                .into_iter()
                .fold(item, |item, accessory| item.with_accessory(accessory)),
        ))
    }
}

// MARK: ListDropdown

#[derive(Clone)]
struct Tooltip(String);

#[derive(Clone)]
enum DropdownOp {
    Value(String),
    OnChange(ComponentArgument),
}

fn list_dropdown() -> ComponentDescriptor {
    ComponentDescriptor::new("ListDropdown", reporting(DropdownMaterializer))
        .with_documentation(
            "A choice beside the search field that narrows a `List`, such as \"My \
             Repositories\". Children are `ListDropdownItem`.",
        )
        .with_constructors(vec![strings_constructor(
            "ListDropdown",
            &["tooltip"],
            &[],
            |mut values| Tooltip(values.remove(0)),
        )])
        .with_methods(vec![
            string_method(
                "value",
                "The value of the chosen item. Defaults to the first item.",
                DropdownOp::Value,
            ),
            callback_method(
                "on_change",
                "Called with the value the user chose.",
                TEXT_CALLBACK,
                DropdownOp::OnChange,
            ),
        ])
}

struct DropdownMaterializer;

impl ComponentMaterializer for DropdownMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ListDropdown")?;
        let Tooltip(tooltip) = payload(&request, "ListDropdown")?;
        let mut dropdown = Dropdown::new(tooltip);
        for op in recorded::<DropdownOp>(&request) {
            dropdown = match op {
                DropdownOp::Value(value) => dropdown.with_value(value),
                DropdownOp::OnChange(callback) => dropdown.with_on_change(text_handler(
                    request.resolve_callback(&callback)?,
                    "ListDropdown.on_change",
                )),
            };
        }
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            match name {
                Some("ListDropdownItem") => {
                    dropdown = dropdown.with_choice(taken(&mut element, "a ListDropdownItem")?)
                }
                other => bail!(
                    "ListDropdown accepts ListDropdownItem children, not {}",
                    describe(other)
                ),
            }
        }
        if let Some(value) = dropdown.value()
            && !dropdown
                .choices()
                .iter()
                .any(|choice| choice.value() == value)
        {
            bail!("ListDropdown.value `{value}` is not the value of any ListDropdownItem");
        }
        Ok(carry(dropdown))
    }
}

fn list_dropdown_item() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "ListDropdownItem",
        reporting(ChoiceMaterializer("ListDropdownItem")),
    )
    .with_documentation("One choice of a `ListDropdown`: the value reported and the title shown.")
    .with_constructors(vec![choice_constructor("ListDropdownItem")])
}

/// `(value, title)`: a choice of a list dropdown or of a form dropdown.
pub(super) fn choice_constructor(export: &'static str) -> gpui_shell::ConstructorDescriptor {
    strings_constructor(export, &["value", "title"], &["value"], |values| {
        ChoiceHead(values[0].clone(), values[1].clone())
    })
}

#[derive(Clone)]
pub(super) struct ChoiceHead(String, String);

pub(super) struct ChoiceMaterializer(pub(super) &'static str);

impl ComponentMaterializer for ChoiceMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), self.0)?;
        let ChoiceHead(value, title) = payload(&request, self.0)?;
        Ok(carry(Choice::new(value, title)))
    }
}
