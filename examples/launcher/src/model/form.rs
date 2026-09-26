use std::collections::BTreeMap;

use gpui_kit::SharedString;

use super::{ActionPanel, Callback};

/// A page that collects input: fields and the actions that submit them.
///
/// Fields are controlled by the page: the window keeps what the user typed,
/// reports changes through each field's `on_change`, and hands the complete
/// values to [`Effect::SubmitForm`](super::Effect::SubmitForm). Validation
/// errors come back as `Field::error` on the next render.
#[derive(Clone, Debug, Default)]
pub struct FormModel {
    fields: Vec<Field>,
    actions: ActionPanel,
    loading: bool,
}

impl FormModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_field(mut self, field: Field) -> Self {
        self.fields.push(field);
        self
    }

    pub fn with_actions(mut self, actions: ActionPanel) -> Self {
        self.actions = actions;
        self
    }

    pub fn with_loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    pub fn actions(&self) -> &ActionPanel {
        &self.actions
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }
}

#[derive(Clone, Debug)]
pub struct Field {
    id: SharedString,
    title: SharedString,
    control: Control,
    info: Option<SharedString>,
    error: Option<SharedString>,
    on_change: Option<Callback<FormValue>>,
}

impl Field {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        control: Control,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            control,
            info: None,
            error: None,
            on_change: None,
        }
    }

    /// Help text under the field.
    pub fn with_info(mut self, info: impl Into<SharedString>) -> Self {
        self.info = Some(info.into());
        self
    }

    pub fn with_error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn with_on_change(mut self, on_change: Callback<FormValue>) -> Self {
        self.on_change = Some(on_change);
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn control(&self) -> &Control {
        &self.control
    }

    pub fn info(&self) -> Option<&SharedString> {
        self.info.as_ref()
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn on_change(&self) -> Option<&Callback<FormValue>> {
        self.on_change.as_ref()
    }
}

/// The control a field is edited with, and the value it starts with.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Text {
        placeholder: Option<SharedString>,
        value: SharedString,
    },
    TextArea {
        placeholder: Option<SharedString>,
        value: SharedString,
    },
    Password {
        placeholder: Option<SharedString>,
        value: SharedString,
    },
    Checkbox {
        label: SharedString,
        value: bool,
    },
    Dropdown {
        choices: Vec<Choice>,
        value: Option<SharedString>,
    },
    /// An ISO 8601 date, `YYYY-MM-DD`.
    Date {
        value: Option<SharedString>,
    },
}

impl Control {
    /// The value this field starts with.
    pub fn initial_value(&self) -> FormValue {
        match self {
            Self::Text { value, .. }
            | Self::TextArea { value, .. }
            | Self::Password { value, .. } => FormValue::Text(value.clone()),
            Self::Checkbox { value, .. } => FormValue::Bool(*value),
            Self::Dropdown { value, .. } | Self::Date { value } => value
                .clone()
                .map(FormValue::Text)
                .unwrap_or(FormValue::Empty),
        }
    }
}

/// One option of a dropdown: the value submitted and the title shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    value: SharedString,
    title: SharedString,
}

impl Choice {
    pub fn new(value: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            title: title.into(),
        }
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FormValue {
    Empty,
    Text(SharedString),
    Bool(bool),
}

/// The values of a form, by field id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormValues(BTreeMap<SharedString, FormValue>);

impl FormValues {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, id: impl Into<SharedString>, value: FormValue) -> Self {
        self.set(id, value);
        self
    }

    pub fn set(&mut self, id: impl Into<SharedString>, value: FormValue) {
        self.0.insert(id.into(), value);
    }

    pub fn get(&self, id: &str) -> Option<&FormValue> {
        self.0.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&SharedString, &FormValue)> {
        self.0.iter()
    }
}
