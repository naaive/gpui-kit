//! What every page node shares: method descriptors for the common argument
//! shapes, reading recorded calls back, and handing values between nodes.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use gpui_kit::{AnyElement, IntoElement as _, StyleRefinement};
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentCallback,
    ComponentCallbackArgument, ComponentDataValue, ComponentMaterializer, ComponentPayload,
    ConstructorDescriptor, MaterializeRequest, MethodDescriptor,
};

use super::super::carrier::{Carrier, record_failure, take};
use crate::model::{FormValue, FormValues, RunHandler, TextHandler, Tone};

pub(super) const RUN_CALLBACK: &str = "(cx: Context) => void";
pub(super) const TEXT_CALLBACK: &str = "(value: string, cx: Context) => void";

/// The tones a tag accepts, spelled as `Tone::parse` reads them.
pub(super) const TONES: &[&str] = &["neutral", "accent", "success", "warning", "danger"];

/// Registers a materializer so that its failure is reported to the author by
/// `take_page_model`, rather than only logged.
pub(super) fn reporting(
    materializer: impl ComponentMaterializer,
) -> Arc<dyn ComponentMaterializer> {
    Arc::new(Reporting(materializer))
}

struct Reporting<M>(M);

impl<M: ComponentMaterializer> ComponentMaterializer for Reporting<M> {
    fn materialize(&self, request: MaterializeRequest<'_>) -> Result<AnyElement> {
        self.0.materialize(request).inspect_err(|error| {
            record_failure(format!("{error:#}"));
        })
    }
}

/// A constructor whose arguments are all strings, each required non-empty
/// where `required` says so. Most nodes are named by a few strings.
pub(super) fn strings_constructor<P: Send + Sync + 'static>(
    export: &'static str,
    names: &'static [&'static str],
    required: &'static [&'static str],
    build: fn(Vec<String>) -> P,
) -> ConstructorDescriptor {
    ConstructorDescriptor::new(
        export,
        names
            .iter()
            .map(|name| ArgumentDescriptor::new(name, ArgumentSchema::String))
            .collect(),
        move |arguments| {
            let values = arguments
                .iter()
                .map(|argument| match argument {
                    ComponentArgument::String(value) => Ok(value.clone()),
                    _ => Err(format!("{export} expects {}", names.join(", "))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (name, value) in names.iter().zip(&values) {
                if required.contains(name) && value.trim().is_empty() {
                    return Err(format!("{export}: `{name}` must not be empty"));
                }
            }
            Ok(ComponentPayload::new(build(values)))
        },
    )
}

/// A constructor taking nothing.
pub(super) fn empty_constructor(export: &'static str) -> ConstructorDescriptor {
    ConstructorDescriptor::new(export, vec![], |_| Ok(ComponentPayload::new(())))
}

pub(super) fn string_method<O: Send + Sync + 'static>(
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

/// A boolean method whose argument may be left out to mean `true`, so
/// `.loading()` reads as naturally as `.loading(this.busy)`.
pub(super) fn bool_method<O: Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    op: fn(bool) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new(
            "value",
            ArgumentSchema::Optional(Box::new(ArgumentSchema::Boolean)),
        )],
        move |arguments| match arguments {
            [ComponentArgument::Optional(None)] => Ok(ComponentPayload::new(op(true))),
            [ComponentArgument::Optional(Some(value))] => match value.as_ref() {
                ComponentArgument::Boolean(value) => Ok(ComponentPayload::new(op(*value))),
                _ => Err(format!("{name} expects a boolean")),
            },
            _ => Err(format!("{name} expects a boolean")),
        },
    )
    .with_documentation(documentation)
}

pub(super) fn callback_method<O: Send + Sync + 'static>(
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

/// A method taking another page node, such as `ListItem.detail(new Detail(…))`.
pub(super) fn element_method<O: Send + Sync + 'static>(
    name: &'static str,
    argument: &'static str,
    documentation: &'static str,
    op: fn(ComponentArgument) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new(argument, ArgumentSchema::Element)],
        move |arguments| match arguments {
            [value @ ComponentArgument::Element(_)] => Ok(ComponentPayload::new(op(value.clone()))),
            _ => Err(format!("{name} expects {argument}")),
        },
    )
    .with_documentation(documentation)
}

pub(super) fn unit_method<O: Clone + Send + Sync + 'static>(
    name: &'static str,
    documentation: &'static str,
    op: O,
) -> MethodDescriptor {
    MethodDescriptor::new(name, vec![], move |_| Ok(ComponentPayload::new(op.clone())))
        .with_documentation(documentation)
}

/// `text, tone?`: a tag on an item or in a detail's metadata.
pub(super) fn tag_method<O: Send + Sync + 'static>(
    documentation: &'static str,
    op: fn(String, Tone) -> O,
) -> MethodDescriptor {
    MethodDescriptor::new(
        "tag",
        vec![
            ArgumentDescriptor::new("text", ArgumentSchema::String),
            ArgumentDescriptor::new(
                "tone",
                ArgumentSchema::Optional(Box::new(ArgumentSchema::Enum(TONES))),
            ),
        ],
        move |arguments| match arguments {
            [ComponentArgument::String(text), tone] => {
                let tone = match optional_string(tone) {
                    None => Tone::Neutral,
                    Some(tone) => Tone::parse(&tone).ok_or_else(|| {
                        format!("unknown tone `{tone}`; use {}", TONES.join(", "))
                    })?,
                };
                Ok(ComponentPayload::new(op(text.clone(), tone)))
            }
            _ => Err("tag expects a text and an optional tone".into()),
        },
    )
    .with_documentation(documentation)
}

/// The text inside an optional string or enum argument.
pub(super) fn optional_string(argument: &ComponentArgument) -> Option<String> {
    match argument {
        ComponentArgument::Optional(Some(inner)) => optional_string(inner),
        ComponentArgument::String(value) | ComponentArgument::Enum(value) => Some(value.clone()),
        _ => None,
    }
}

/// The recorded method calls of one node, in call order.
pub(super) fn recorded<O: Clone + 'static>(request: &MaterializeRequest<'_>) -> Vec<O> {
    request
        .methods()
        .filter_map(|method| method.payload().downcast_ref::<O>().cloned())
        .collect()
}

pub(super) fn payload<T: Clone + 'static>(
    request: &MaterializeRequest<'_>,
    name: &str,
) -> Result<T> {
    request
        .payload()
        .downcast_ref::<T>()
        .cloned()
        .ok_or_else(|| anyhow!("{name} received an incompatible payload"))
}

/// Wraps a model value for the parent node.
pub(super) fn carry<T: 'static>(value: T) -> AnyElement {
    Carrier::new(value).into_any_element()
}

/// Takes the model value a child or argument element carries, naming what was
/// expected when it carries something else.
pub(super) fn taken<T: 'static>(element: &mut AnyElement, expected: &str) -> Result<T> {
    take::<T>(element).ok_or_else(|| anyhow!("expected {expected}"))
}

/// Materializes a node passed as a method argument and takes its value.
pub(super) fn resolved<T: 'static>(
    request: &mut MaterializeRequest<'_>,
    argument: &ComponentArgument,
    expected: &str,
) -> Result<T> {
    let mut element = request.resolve_element(argument)?;
    taken(&mut element, expected)
}

/// These nodes describe data, not boxes, so a style method is a mistake worth
/// naming rather than silently dropping.
pub(super) fn reject_style(style: StyleRefinement, name: &str) -> Result<()> {
    anyhow::ensure!(
        style == StyleRefinement::default(),
        "{name} is drawn by the launcher and does not take styles"
    );
    Ok(())
}

/// Names a child for an error message.
pub(super) fn describe(component: Option<&str>) -> String {
    match component {
        Some(name) => format!("`{name}`"),
        None => "a plain element".into(),
    }
}

/// A handler that runs a script callback with no argument. `context` names
/// the callback in the log if the script throws, since there is nobody else to
/// tell.
pub(super) fn run_handler(callback: ComponentCallback, context: &'static str) -> RunHandler {
    RunHandler::new(move |(), window, cx| callback.invoke_and_report_with(context, &[], window, cx))
}

pub(super) fn text_handler(callback: ComponentCallback, context: &'static str) -> TextHandler {
    TextHandler::new(move |text, window, cx| {
        callback.invoke_and_report_with(
            context,
            &[ComponentCallbackArgument::String(text.to_string())],
            window,
            cx,
        )
    })
}

/// A form value as the script sees it: a string, a boolean, or `null`.
pub(super) fn form_value_data(value: &FormValue) -> ComponentDataValue {
    match value {
        FormValue::Empty => ComponentDataValue::Null,
        FormValue::Text(text) => ComponentDataValue::String(text.to_string()),
        FormValue::Bool(value) => ComponentDataValue::Boolean(*value),
    }
}

/// A form's values as one plain object keyed by field id, which is what a
/// submit handler destructures.
pub(super) fn form_values_data(values: &FormValues) -> ComponentDataValue {
    ComponentDataValue::Object(
        values
            .iter()
            .map(|(id, value)| (id.to_string(), form_value_data(value)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form_values_reach_script_as_a_plain_object() {
        let values = FormValues::new()
            .with("title", FormValue::Text("Groceries".into()))
            .with("pinned", FormValue::Bool(true))
            .with("due", FormValue::Empty);
        assert_eq!(
            form_values_data(&values),
            ComponentDataValue::Object(vec![
                ("due".into(), ComponentDataValue::Null),
                ("pinned".into(), ComponentDataValue::Boolean(true)),
                (
                    "title".into(),
                    ComponentDataValue::String("Groceries".into())
                ),
            ])
        );
    }
}
