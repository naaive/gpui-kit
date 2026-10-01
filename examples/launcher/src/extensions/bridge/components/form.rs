//! `Form` and its fields: `TextField`, `TextArea`, `PasswordField`,
//! `Checkbox`, `Dropdown` (with `DropdownItem`), `DatePicker`, `FilePicker`,
//! `TagPicker` (with `TagPickerItem`); and what arranges them:
//! `FormSeparator` and `FormDescription`.
//!
//! Fields are controlled by the page, as GPUI Shell's controls are: the
//! launcher keeps what the user typed and reports it through `on_change`, and
//! a submit action receives every value at once. A field's `error` comes back
//! on the next render, which is how validation is expressed.

use anyhow::{Result, bail};
use gpui_kit::AnyElement;
use gpui_shell::{
    ArgumentDescriptor, ArgumentSchema, ComponentArgument, ComponentDescriptor,
    ComponentMaterializer, ComponentPayload, ComponentRegistry, MaterializeRequest,
    MethodDescriptor, RegistryError,
};

use super::{
    action::resolve_panel,
    list::{ChoiceMaterializer, choice_constructor},
    shared::{
        bool_method, callback_method, carry, describe, element_method, empty_constructor,
        form_value_data, optional_string, payload, recorded, reject_style, reporting,
        string_method, strings_constructor, taken,
    },
};
use crate::model::{Callback, Choice, Control, Field, FormModel, FormValue, PageModel};

const CHANGE_CALLBACK: &str = "(value: string | string[] | boolean | null, cx: Context) => void";

pub(super) fn register(registry: &mut ComponentRegistry) -> Result<(), RegistryError> {
    registry.register(form())?;
    registry.register(field(FieldControl::Text))?;
    registry.register(field(FieldControl::TextArea))?;
    registry.register(field(FieldControl::Password))?;
    registry.register(field(FieldControl::Checkbox))?;
    registry.register(field(FieldControl::Dropdown))?;
    registry.register(field(FieldControl::Date))?;
    registry.register(field(FieldControl::Files))?;
    registry.register(field(FieldControl::Tags))?;
    registry.register(dropdown_item())?;
    registry.register(tag_picker_item())?;
    registry.register(form_separator())?;
    registry.register(form_description())?;
    Ok(())
}

// MARK: Form

#[derive(Clone)]
enum FormOp {
    Loading(bool),
    Actions(ComponentArgument),
}

fn form() -> ComponentDescriptor {
    ComponentDescriptor::new("Form", reporting(FormMaterializer))
        .with_documentation(
            "Fields to fill in and submit. Return it from `render`. Children are `TextField`, \
             `TextArea`, `PasswordField`, `Checkbox`, `Dropdown` and `DatePicker`; give it an \
             `ActionPanel` whose first action `submit`s.",
        )
        .with_constructors(vec![empty_constructor("Form")])
        .with_methods(vec![
            bool_method(
                "loading",
                "Shows that the form is working, such as while it saves.",
                FormOp::Loading,
            ),
            element_method(
                "actions",
                "panel",
                "The form's `ActionPanel`; its first action is performed by Cmd/Ctrl-Enter.",
                FormOp::Actions,
            ),
        ])
}

struct FormMaterializer;

impl ComponentMaterializer for FormMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "Form")?;
        let mut form = FormModel::new();
        for op in recorded::<FormOp>(&request) {
            form = match op {
                FormOp::Loading(loading) => form.with_loading(loading),
                FormOp::Actions(panel) => {
                    form.with_actions(resolve_panel(&mut request, &panel, "Form")?)
                }
            };
        }
        for (ix, mut child) in request.take_typed_children()?.into_iter().enumerate() {
            let name = child.component_name();
            let mut element = request.materialize_child(&mut child)?;
            let field: Field = match name {
                Some(name) if FieldControl::from_export(name).is_some() => {
                    taken(&mut element, "a form field")?
                }
                Some("FormSeparator" | "FormDescription") => {
                    // Arrangement has no value, so no id of the author's;
                    // one no field id can take keeps the form's state apart.
                    let field: Field = taken(&mut element, "a form arrangement")?;
                    Field::new(
                        format!("#{ix}"),
                        field.title().clone(),
                        field.control().clone(),
                    )
                }
                other => bail!(
                    "Form accepts TextField, TextArea, PasswordField, Checkbox, Dropdown, \
                     DatePicker, FilePicker, TagPicker, FormSeparator and FormDescription \
                     children, not {}",
                    describe(other)
                ),
            };
            if form.fields().iter().any(|each| each.id() == field.id()) {
                bail!("Form has two fields with the id `{}`", field.id());
            }
            form = form.with_field(field);
        }
        Ok(carry(PageModel::Form(form)))
    }
}

// MARK: Fields

/// Which control a field node draws; one materializer serves every field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FieldControl {
    Text,
    TextArea,
    Password,
    Checkbox,
    Dropdown,
    Date,
    Files,
    Tags,
}

impl FieldControl {
    const ALL: [Self; 8] = [
        Self::Text,
        Self::TextArea,
        Self::Password,
        Self::Checkbox,
        Self::Dropdown,
        Self::Date,
        Self::Files,
        Self::Tags,
    ];

    fn export(self) -> &'static str {
        match self {
            Self::Text => "TextField",
            Self::TextArea => "TextArea",
            Self::Password => "PasswordField",
            Self::Checkbox => "Checkbox",
            Self::Dropdown => "Dropdown",
            Self::Date => "DatePicker",
            Self::Files => "FilePicker",
            Self::Tags => "TagPicker",
        }
    }

    fn from_export(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|control| control.export() == name)
    }

    fn documentation(self) -> &'static str {
        match self {
            Self::Text => "A one-line text field of a `Form`.",
            Self::TextArea => "A multi-line text field of a `Form`.",
            Self::Password => "A text field of a `Form` whose text is hidden.",
            Self::Checkbox => {
                "A checkbox of a `Form`: `title` labels the field, `label` sits beside the box."
            }
            Self::Dropdown => "A choice of a `Form`. Children are `DropdownItem`.",
            Self::Date => {
                "A date of a `Form`, as an ISO 8601 `YYYY-MM-DD` string, or \
                 `YYYY-MM-DDTHH:MM` with `include_time`."
            }
            Self::Files => {
                "Files or folders of a `Form`, chosen in the system's open panel, as an array \
                 of paths."
            }
            Self::Tags => {
                "Any number of choices of a `Form`, as an array of values. Children are \
                 `TagPickerItem`."
            }
        }
    }

    /// Whether the value is an array of strings.
    fn is_list(self) -> bool {
        matches!(self, Self::Files | Self::Tags)
    }

    fn is_text(self) -> bool {
        matches!(self, Self::Text | Self::TextArea | Self::Password)
    }
}

#[derive(Clone)]
struct FieldHead {
    id: String,
    title: String,
    /// The text beside a checkbox; empty for other fields.
    label: String,
}

#[derive(Clone)]
enum FieldValue {
    Text(String),
    Bool(bool),
    List(Vec<String>),
}

#[derive(Clone)]
enum FieldOp {
    Placeholder(String),
    Value(FieldValue),
    DefaultValue(FieldValue),
    Info(String),
    Error(String),
    OnChange(ComponentArgument),
    IncludeTime(bool),
    Directories(bool),
    Multiple(bool),
    /// `value(null)` or `default_value(null)`: nothing, as if not called.
    Unset,
}

fn field(control: FieldControl) -> ComponentDescriptor {
    let export = control.export();
    let constructor = if control == FieldControl::Checkbox {
        strings_constructor(export, &["id", "title", "label"], &["id"], |values| {
            FieldHead {
                id: values[0].clone(),
                title: values[1].clone(),
                label: values[2].clone(),
            }
        })
    } else {
        strings_constructor(export, &["id", "title"], &["id"], |values| FieldHead {
            id: values[0].clone(),
            title: values[1].clone(),
            label: String::new(),
        })
    };

    let mut methods = Vec::new();
    if control.is_text() {
        methods.push(string_method(
            "placeholder",
            "Text shown while the field is empty.",
            FieldOp::Placeholder,
        ));
    }
    match control {
        FieldControl::Date => methods.push(bool_method(
            "include_time",
            "Asks for a time of day too; the value becomes `YYYY-MM-DDTHH:MM`.",
            FieldOp::IncludeTime,
        )),
        FieldControl::Files => {
            methods.push(bool_method(
                "directories",
                "Chooses folders instead of files.",
                FieldOp::Directories,
            ));
            methods.push(bool_method(
                "multiple",
                "Lets the user choose more than one.",
                FieldOp::Multiple,
            ));
        }
        _ => {}
    }
    let (value_schema, value_type) = if control == FieldControl::Checkbox {
        (ArgumentSchema::Boolean, "a boolean")
    } else if control.is_list() {
        (
            ArgumentSchema::Array(Box::new(ArgumentSchema::String)),
            "an array of strings",
        )
    } else {
        (ArgumentSchema::String, "a string")
    };
    methods.push(value_method(
        "value",
        "The field's value, kept by the page: render the value `on_change` reported.",
        value_schema.clone(),
        value_type,
        FieldOp::Value,
    ));
    methods.push(value_method(
        "default_value",
        "The value the field starts with when the page does not keep it; `value` wins.",
        value_schema,
        value_type,
        FieldOp::DefaultValue,
    ));
    methods.push(string_method(
        "info",
        "Help text under the field.",
        FieldOp::Info,
    ));
    methods.push(string_method(
        "error",
        "A validation message under the field; render it after a failed submit.",
        FieldOp::Error,
    ));
    methods.push(callback_method(
        "on_change",
        "Called with the new value whenever the user changes it: a string, a boolean for a \
         checkbox, or null for an empty dropdown or date.",
        CHANGE_CALLBACK,
        FieldOp::OnChange,
    ));

    ComponentDescriptor::new(export, reporting(FieldMaterializer(control)))
        .with_documentation(control.documentation())
        .with_constructors(vec![constructor])
        .with_methods(methods)
}

fn value_method(
    name: &'static str,
    documentation: &'static str,
    schema: ArgumentSchema,
    value_type: &'static str,
    op: fn(FieldValue) -> FieldOp,
) -> MethodDescriptor {
    MethodDescriptor::new(
        name,
        vec![ArgumentDescriptor::new(
            "value",
            ArgumentSchema::Optional(Box::new(schema)),
        )],
        move |arguments| match arguments {
            [ComponentArgument::Optional(None)] => Ok(ComponentPayload::new(FieldOp::Unset)),
            [ComponentArgument::Optional(Some(value))] => match value.as_ref() {
                ComponentArgument::String(value) => {
                    Ok(ComponentPayload::new(op(FieldValue::Text(value.clone()))))
                }
                ComponentArgument::Boolean(value) => {
                    Ok(ComponentPayload::new(op(FieldValue::Bool(*value))))
                }
                ComponentArgument::Array(values) => values
                    .iter()
                    .map(|value| match value {
                        ComponentArgument::String(value) => Ok(value.clone()),
                        _ => Err(format!("{name} expects {value_type}")),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|values| ComponentPayload::new(op(FieldValue::List(values)))),
                _ => Err(format!("{name} expects {value_type}")),
            },
            [ComponentArgument::String(value)] => {
                Ok(ComponentPayload::new(op(FieldValue::Text(value.clone()))))
            }
            [ComponentArgument::Boolean(value)] => {
                Ok(ComponentPayload::new(op(FieldValue::Bool(*value))))
            }
            [ComponentArgument::Array(values)] => values
                .iter()
                .map(|value| match value {
                    ComponentArgument::String(value) => Ok(value.clone()),
                    _ => Err(format!("{name} expects {value_type}")),
                })
                .collect::<Result<Vec<_>, _>>()
                .map(|values| ComponentPayload::new(op(FieldValue::List(values)))),
            _ => Err(format!("{name} expects {value_type}")),
        },
    )
    .with_documentation(documentation)
}

struct FieldMaterializer(FieldControl);

impl ComponentMaterializer for FieldMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        let export = self.0.export();
        reject_style(request.take_style(), export)?;
        let FieldHead { id, title, label } = payload(&request, export)?;
        let mut placeholder = None;
        let mut value = None;
        let mut default_value = None;
        let mut info = None;
        let mut error = None;
        let mut on_change = None;
        let (mut include_time, mut directories, mut multiple) = (false, false, false);
        for op in recorded::<FieldOp>(&request) {
            match op {
                FieldOp::Placeholder(text) => placeholder = Some(text.into()),
                FieldOp::Value(next) => value = Some(next),
                FieldOp::DefaultValue(next) => default_value = Some(next),
                FieldOp::Info(text) => info = Some(text),
                FieldOp::Error(text) => error = Some(text),
                FieldOp::OnChange(callback) => {
                    on_change = Some(request.resolve_callback(&callback)?)
                }
                FieldOp::IncludeTime(value) => include_time = value,
                FieldOp::Directories(value) => directories = value,
                FieldOp::Multiple(value) => multiple = value,
                FieldOp::Unset => {}
            }
        }
        let value = value.or(default_value);
        let text = |value: Option<FieldValue>| match value {
            Some(FieldValue::Text(text)) => text,
            _ => String::new(),
        };
        let list = |value: Option<FieldValue>| match value {
            Some(FieldValue::List(values)) => values.into_iter().map(Into::into).collect(),
            _ => Vec::new(),
        };

        let control = match self.0 {
            FieldControl::Text => Control::Text {
                placeholder,
                value: text(value).into(),
            },
            FieldControl::TextArea => Control::TextArea {
                placeholder,
                value: text(value).into(),
            },
            FieldControl::Password => Control::Password {
                placeholder,
                value: text(value).into(),
            },
            FieldControl::Checkbox => Control::Checkbox {
                label: label.into(),
                value: matches!(value, Some(FieldValue::Bool(true))),
            },
            FieldControl::Dropdown => {
                let mut choices: Vec<Choice> = Vec::new();
                for mut child in request.take_typed_children()? {
                    let name = child.component_name();
                    let mut element = request.materialize_child(&mut child)?;
                    match name {
                        Some("DropdownItem") => {
                            choices.push(taken(&mut element, "a DropdownItem")?)
                        }
                        other => bail!(
                            "Dropdown `{id}` accepts DropdownItem children, not {}",
                            describe(other)
                        ),
                    }
                }
                let value = Some(text(value)).filter(|value| !value.is_empty());
                if let Some(value) = &value
                    && !choices
                        .iter()
                        .any(|choice| choice.value().as_ref() == value)
                {
                    bail!("Dropdown `{id}` has no DropdownItem with the value `{value}`");
                }
                Control::Dropdown {
                    choices,
                    value: value.map(Into::into),
                }
            }
            FieldControl::Date if include_time => {
                let value = Some(text(value)).filter(|value| !value.is_empty());
                if let Some(value) = &value
                    && chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M").is_err()
                {
                    bail!("DatePicker `{id}` value `{value}` is not a YYYY-MM-DDTHH:MM time");
                }
                Control::DateTime {
                    value: value.map(Into::into),
                }
            }
            FieldControl::Date => {
                let value = Some(text(value)).filter(|value| !value.is_empty());
                if let Some(value) = &value
                    && !is_iso_date(value)
                {
                    bail!("DatePicker `{id}` value `{value}` is not a YYYY-MM-DD date");
                }
                Control::Date {
                    value: value.map(Into::into),
                }
            }
            FieldControl::Files => Control::Files {
                value: list(value),
                directories,
                multiple,
            },
            FieldControl::Tags => {
                let mut choices: Vec<Choice> = Vec::new();
                for mut child in request.take_typed_children()? {
                    let name = child.component_name();
                    let mut element = request.materialize_child(&mut child)?;
                    match name {
                        Some("TagPickerItem") => {
                            choices.push(taken(&mut element, "a TagPickerItem")?)
                        }
                        other => bail!(
                            "TagPicker `{id}` accepts TagPickerItem children, not {}",
                            describe(other)
                        ),
                    }
                }
                let value: Vec<gpui_kit::SharedString> = list(value);
                if let Some(missing) = value
                    .iter()
                    .find(|value| !choices.iter().any(|choice| choice.value() == *value))
                {
                    bail!("TagPicker `{id}` has no TagPickerItem with the value `{missing}`");
                }
                Control::Tags { choices, value }
            }
        };

        let mut field = Field::new(id, title, control);
        if let Some(info) = info {
            field = field.with_info(info);
        }
        if let Some(error) = error {
            field = field.with_error(error);
        }
        if let Some(callback) = on_change {
            let context = self.0.export();
            field = field.with_on_change(Callback::new(move |value: FormValue, window, cx| {
                if let Err(error) =
                    callback.invoke_data_with(&[form_value_data(&value)], window, cx)
                {
                    tracing::error!("{context}.on_change: {error:#}");
                }
            }));
        }
        Ok(carry(field))
    }
}

/// `YYYY-MM-DD` with a plausible month and day. Calendar arithmetic belongs
/// to the date picker; this only catches a value in the wrong shape.
fn is_iso_date(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    let number = |text: &str, digits: usize| {
        (text.len() == digits && text.chars().all(|c| c.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    matches!(
        (number(year, 4), number(month, 2), number(day, 2)),
        (Some(_), Some(1..=12), Some(1..=31))
    )
}

fn dropdown_item() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "DropdownItem",
        reporting(ChoiceMaterializer("DropdownItem")),
    )
    .with_documentation("One choice of a form `Dropdown`: the value submitted and the title shown.")
    .with_constructors(vec![choice_constructor("DropdownItem")])
}

fn tag_picker_item() -> ComponentDescriptor {
    ComponentDescriptor::new(
        "TagPickerItem",
        reporting(ChoiceMaterializer("TagPickerItem")),
    )
    .with_documentation("One choice of a `TagPicker`: the value submitted and the title shown.")
    .with_constructors(vec![choice_constructor("TagPickerItem")])
}

// MARK: Arrangement

fn form_separator() -> ComponentDescriptor {
    ComponentDescriptor::new("FormSeparator", reporting(SeparatorMaterializer))
        .with_documentation("A line between groups of fields of a `Form`.")
        .with_constructors(vec![empty_constructor("FormSeparator")])
}

struct SeparatorMaterializer;

impl ComponentMaterializer for SeparatorMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "FormSeparator")?;
        Ok(carry(Field::new("", "", Control::Separator)))
    }
}

#[derive(Clone)]
struct DescriptionHead {
    title: String,
    text: String,
}

fn form_description() -> ComponentDescriptor {
    ComponentDescriptor::new("FormDescription", reporting(DescriptionMaterializer))
        .with_documentation(
            "Text that explains the fields around it, in a `Form`. With one argument it is \
             the text; with two, a label and the text.",
        )
        .with_constructors(vec![gpui_shell::ConstructorDescriptor::new(
            "FormDescription",
            vec![
                ArgumentDescriptor::new("text", ArgumentSchema::String),
                ArgumentDescriptor::new(
                    "more",
                    ArgumentSchema::Optional(Box::new(ArgumentSchema::String)),
                ),
            ],
            |arguments| match arguments {
                [ComponentArgument::String(first), more] => {
                    Ok(ComponentPayload::new(match optional_string(more) {
                        Some(text) => DescriptionHead {
                            title: first.clone(),
                            text,
                        },
                        None => DescriptionHead {
                            title: String::new(),
                            text: first.clone(),
                        },
                    }))
                }
                _ => Err("FormDescription expects a text, or a label and a text".into()),
            },
        )])
}

struct DescriptionMaterializer;

impl ComponentMaterializer for DescriptionMaterializer {
    fn materialize(&self, mut request: MaterializeRequest<'_>) -> Result<AnyElement> {
        reject_style(request.take_style(), "FormDescription")?;
        let DescriptionHead { title, text } = payload(&request, "FormDescription")?;
        Ok(carry(Field::new(
            "",
            title,
            Control::Description { text: text.into() },
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_iso_date() {
        assert!(is_iso_date("2026-09-26"));
        assert!(!is_iso_date("2026-9-26"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("26/09/2026"));
    }
}
