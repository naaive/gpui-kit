//! The page nodes an extension builds: `List`, `ListSection`, `ListItem`,
//! `Action`.
//!
//! Each is a GPUI Shell component whose materializer draws nothing. Methods are
//! recorded as small operation enums; materializing replays them into the
//! launcher's model types and hands the result to the parent in a [`Carrier`].

use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use gpui_kit::{AnyElement, IntoElement as _, StyleRefinement};
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentCallbackArgument,
    ComponentDescriptor, ComponentMaterializer, ComponentPayload, ComponentRegistry,
    ConstructorDescriptor, MaterializeRequest, MethodDescriptor, RegistryError,
};

use super::carrier::{Carrier, take};
use crate::model::{
    Accessory, Action, Effect, Item, ItemId, ListModel, PageModel, RunHandler, Section,
    TextHandler, Toast,
};

const QUERY_CALLBACK: &str = "(query: string, cx: Context) => void";
const RUN_CALLBACK: &str = "(cx: Context) => void";

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    registry.register(list())?;
    registry.register(list_section())?;
    registry.register(list_item())?;
    registry.register(action())?;
    Ok(())
}

// MARK: List

#[derive(Clone)]
enum ListOp {
    Placeholder(String),
    Loading(bool),
    Filtering(bool),
    EmptyTitle(String),
    OnQueryChange(ComponentArgument),
}

fn list() -> ComponentDescriptor {
    ComponentDescriptor::new("List", Arc::new(ListMaterializer))
        .with_documentation(
            "A searchable list of items; the page an extension command returns from `render`. \
             Children are `ListSection` and `ListItem`.",
        )
        .with_constructors(vec![ConstructorDescriptor::new("List", vec![], |_| {
            Ok(ComponentPayload::new(()))
        })])
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
                "Text shown when no item matches.",
                ListOp::EmptyTitle,
            ),
            callback_method(
                "on_query_change",
                "Called with the search text whenever it changes.",
                QUERY_CALLBACK,
                ListOp::OnQueryChange,
            ),
        ])
}

struct ListMaterializer;

impl ComponentMaterializer for ListMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "List")?;
        let mut list = ListModel::new();
        let mut filtering = None;
        let mut on_query_change = None;
        for op in recorded::<ListOp>(&request) {
            match op {
                ListOp::Placeholder(text) => list = list.with_placeholder(text),
                ListOp::Loading(loading) => list = list.with_loading(loading),
                ListOp::Filtering(value) => filtering = Some(value),
                ListOp::EmptyTitle(text) => list = list.with_empty_title(text),
                ListOp::OnQueryChange(callback) => {
                    on_query_change = Some(request.resolve_callback(&callback)?)
                }
            }
        }
        list = list.with_filtering(filtering.unwrap_or(on_query_change.is_none()));

        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            list = match name {
                Some("ListSection") => list.with_section(taken(&mut element, "ListSection")?),
                Some("ListItem") => list.with_item(taken(&mut element, "ListItem")?),
                other => bail!(
                    "List accepts ListSection and ListItem children, not {}",
                    describe(other)
                ),
            };
        }

        if let Some(callback) = on_query_change {
            list = list.with_on_query_change(TextHandler::new(move |query, window, cx| {
                callback.invoke_and_report_with(
                    "List.on_query_change",
                    &[ComponentCallbackArgument::String(query.to_string())],
                    window,
                    cx,
                )
            }));
        }
        Ok(Carrier::new(PageModel::List(list)).into_any_element())
    }
}

// MARK: ListSection

#[derive(Clone)]
struct Title(String);

fn list_section() -> ComponentDescriptor {
    ComponentDescriptor::new("ListSection", Arc::new(SectionMaterializer))
        .with_documentation("A titled group of `ListItem`s inside a `List`.")
        .with_constructors(vec![ConstructorDescriptor::new(
            "ListSection",
            vec![ArgumentDescriptor::new("title", ArgumentSchema::String)],
            |arguments| match arguments {
                [ComponentArgument::String(title)] => {
                    Ok(ComponentPayload::new(Title(title.clone())))
                }
                _ => Err("ListSection expects a title".into()),
            },
        )])
}

struct SectionMaterializer;

impl ComponentMaterializer for SectionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ListSection")?;
        let Title(title) = payload::<Title>(&request, "ListSection")?;
        let mut section = Section::new().with_title(title);
        for mut child in request.take_typed_children()? {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            match name {
                Some("ListItem") => section = section.with_item(taken(&mut element, "ListItem")?),
                other => bail!(
                    "ListSection accepts ListItem children, not {}",
                    describe(other)
                ),
            }
        }
        Ok(Carrier::new(section).into_any_element())
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
    Keyword(String),
    Action(ComponentArgument),
}

fn list_item() -> ComponentDescriptor {
    ComponentDescriptor::new("ListItem", Arc::new(ItemMaterializer))
        .with_documentation(
            "One row of a `List`. The id must stay the same across renders so the selection \
             follows the item. The first action is the primary action (Enter), the second \
             the secondary one (Cmd/Ctrl-Enter).",
        )
        .with_constructors(vec![ConstructorDescriptor::new(
            "ListItem",
            vec![
                ArgumentDescriptor::new("id", ArgumentSchema::String),
                ArgumentDescriptor::new("title", ArgumentSchema::String),
            ],
            |arguments| match arguments {
                [
                    ComponentArgument::String(id),
                    ComponentArgument::String(title),
                ] if !id.is_empty() => Ok(ComponentPayload::new(ItemHead {
                    id: id.clone(),
                    title: title.clone(),
                })),
                _ => Err("ListItem expects a non-empty id and a title".into()),
            },
        )])
        .with_methods(vec![
            string_method(
                "subtitle",
                "Secondary text after the title.",
                ItemOp::Subtitle,
            ),
            string_method("icon", "A Lucide icon name, such as `globe`.", ItemOp::Icon),
            string_method(
                "accessory",
                "Short trailing text, such as a count.",
                ItemOp::Accessory,
            ),
            string_method(
                "keyword",
                "An extra word the item matches without showing it.",
                ItemOp::Keyword,
            ),
            MethodDescriptor::new(
                "action",
                vec![ArgumentDescriptor::new("action", ArgumentSchema::Element)],
                |arguments| match arguments {
                    [argument @ ComponentArgument::Element(_)] => {
                        Ok(ComponentPayload::new(ItemOp::Action(argument.clone())))
                    }
                    _ => Err("action expects an Action".into()),
                },
            )
            .with_documentation("Appends an `Action`. Order matters: first is primary."),
        ])
}

struct ItemMaterializer;

impl ComponentMaterializer for ItemMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "ListItem")?;
        let ItemHead { id, title } = payload::<ItemHead>(&request, "ListItem")?;
        let mut item = Item::new(ItemId::new(id), title);
        for op in recorded::<ItemOp>(&request) {
            item = match op {
                ItemOp::Subtitle(text) => item.with_subtitle(text),
                ItemOp::Icon(icon) => item.with_icon(icon),
                ItemOp::Accessory(text) => item.with_accessory(Accessory::text(text)),
                ItemOp::Keyword(word) => item.with_keyword(word),
                ItemOp::Action(argument) => {
                    let mut element = request.resolve_element(&argument)?;
                    item.with_action(
                        take::<Action>(&mut element)
                            .ok_or_else(|| anyhow!("ListItem.action expects an Action"))?,
                    )
                }
            };
        }
        Ok(Carrier::new(item).into_any_element())
    }
}

// MARK: Action

#[derive(Clone)]
enum ActionOp {
    Shortcut(String),
    OpenUrl(String),
    Copy(String),
    Toast(String),
    Run(ComponentArgument),
    Pop,
    CloseWindow,
}

fn action() -> ComponentDescriptor {
    ComponentDescriptor::new("Action", Arc::new(ActionMaterializer))
        .with_documentation(
            "Something the user can do to a `ListItem`. Give it exactly one effect: \
             `open_url`, `copy`, `toast`, `run`, `pop` or `close_window`.",
        )
        .with_constructors(vec![ConstructorDescriptor::new(
            "Action",
            vec![ArgumentDescriptor::new("title", ArgumentSchema::String)],
            |arguments| match arguments {
                [ComponentArgument::String(title)] if !title.trim().is_empty() => {
                    Ok(ComponentPayload::new(Title(title.clone())))
                }
                _ => Err("Action expects a non-empty title".into()),
            },
        )])
        .with_methods(vec![
            string_method(
                "shortcut",
                "A keystroke that performs the action, such as `cmd-shift-c`.",
                ActionOp::Shortcut,
            ),
            string_method(
                "open_url",
                "Opens a URL in the default browser.",
                ActionOp::OpenUrl,
            ),
            string_method("copy", "Copies text to the clipboard.", ActionOp::Copy),
            string_method("toast", "Shows a message in the launcher.", ActionOp::Toast),
            callback_method(
                "run",
                "Calls back into the extension.",
                RUN_CALLBACK,
                ActionOp::Run,
            ),
            unit_method("pop", "Returns to the previous page.", ActionOp::Pop),
            unit_method("close_window", "Hides the launcher.", ActionOp::CloseWindow),
        ])
}

struct ActionMaterializer;

impl ComponentMaterializer for ActionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "Action")?;
        let Title(title) = payload::<Title>(&request, "Action")?;
        let mut shortcut = None;
        let mut effect = None;
        for op in recorded::<ActionOp>(&request) {
            let next = match op {
                ActionOp::Shortcut(value) => {
                    shortcut = Some(value);
                    continue;
                }
                ActionOp::OpenUrl(url) => Effect::OpenUrl(url.into()),
                ActionOp::Copy(text) => Effect::Copy(text.into()),
                ActionOp::Toast(message) => {
                    Effect::ShowToast(Toast::new(Default::default(), message))
                }
                ActionOp::Run(argument) => {
                    let callback = request.resolve_callback(&argument)?;
                    Effect::Run(RunHandler::new(move |(), window, cx| {
                        callback.invoke_and_report_with("Action.run", &[], window, cx)
                    }))
                }
                ActionOp::Pop => Effect::Pop,
                ActionOp::CloseWindow => Effect::CloseWindow,
            };
            if effect.replace(next).is_some() {
                bail!("Action `{title}` has more than one effect; give it exactly one");
            }
        }
        let effect = effect.with_context(|| {
            format!(
                "Action `{title}` has no effect; call one of open_url, copy, toast, run, pop \
                 or close_window"
            )
        })?;
        let action = Action::new(title, effect);
        let action = match shortcut {
            Some(shortcut) => action.with_shortcut(shortcut),
            None => action,
        };
        Ok(Carrier::new(action).into_any_element())
    }
}

// MARK: Helpers

fn string_method<O: Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    op: fn(String) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new("value", ArgumentSchema::String)],
        move |arguments| match arguments {
            [ComponentArgument::String(value)] => Ok(ComponentPayload::new(op(value.clone()))),
            _ => Err(format!("{name} expects a string")),
        },
    )
    .with_documentation(documentation)
}

fn bool_method<O: Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    op: fn(bool) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new("value", ArgumentSchema::Boolean)],
        move |arguments| match arguments {
            [ComponentArgument::Boolean(value)] => Ok(ComponentPayload::new(op(*value))),
            _ => Err(format!("{name} expects a boolean")),
        },
    )
    .with_documentation(documentation)
}

fn callback_method<O: Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    signature: &'static str,
    op: fn(ComponentArgument) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new(
            "callback",
            ArgumentSchema::Callback(signature),
        )],
        move |arguments| match arguments {
            [argument @ ComponentArgument::Callback(_)] => {
                Ok(ComponentPayload::new(op(argument.clone())))
            }
            _ => Err(format!("{name} expects a function")),
        },
    )
    .with_documentation(documentation)
}

fn unit_method<O: Clone + Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    op: O,
) -> MethodDescriptor {
    MethodDescriptor::new(name, vec![], move |_| Ok(ComponentPayload::new(op.clone())))
        .with_documentation(documentation)
}

/// The recorded method calls of one kind, in call order.
fn recorded<O: Clone + 'static>(request: &MaterializeRequest<'_>) -> Vec<O> {
    request
        .methods()
        .filter_map(|method| method.payload().downcast_ref::<O>().cloned())
        .collect()
}

fn payload<T: Clone + 'static>(request: &MaterializeRequest<'_>, name: &str) -> Result<T> {
    request
        .payload()
        .downcast_ref::<T>()
        .cloned()
        .ok_or_else(|| anyhow!("{name} received an incompatible payload"))
}

fn taken<T: 'static>(element: &mut AnyElement, name: &str) -> Result<T> {
    take::<T>(element).ok_or_else(|| anyhow!("{name} materialized an incompatible child"))
}

/// These nodes describe data, not boxes, so a style method is a mistake worth
/// naming rather than silently dropping.
fn reject_style(style: StyleRefinement, name: &str) -> Result<()> {
    anyhow::ensure!(
        style == StyleRefinement::default(),
        "{name} is drawn by the launcher and does not take styles"
    );
    Ok(())
}

fn describe(component: Option<&str>) -> String {
    match component {
        Some(name) => format!("`{name}`"),
        None => "a plain element".into(),
    }
}
