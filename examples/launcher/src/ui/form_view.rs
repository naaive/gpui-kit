//! Forms: one labelled row per field, with the field's help and error text.
//!
//! What the user types lives here, in retained control state keyed by the
//! page's stack entry and the field id, so a page re-rendering its model
//! never resets a half-filled form. Pages hear about changes through each
//! field's `on_change` and receive the values when the form is submitted.

use std::collections::HashMap;

use chrono::NaiveDate;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, IndexPath,
        checkbox::Checkbox,
        date_picker::{DatePicker, DatePickerEvent, DatePickerState},
        h_flex,
        input::{
            IndentInline, Input, InputEvent, InputState, OutdentInline, Textarea, TextareaState,
        },
        select::{Select, SelectEvent, SelectItem, SelectState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{LauncherWindow, keyed_id};
use crate::{
    model::{Choice, Control, Field, FormModel, FormValue, FormValues},
    session::EntryId,
};

/// The format of `Control::Date` values.
const DATE_FORMAT: &str = "%Y-%m-%d";

/// A dropdown choice as the Select component lists it.
#[derive(Clone)]
pub(super) struct ChoiceItem(Choice);

impl SelectItem for ChoiceItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.0.title().clone()
    }

    fn value(&self) -> &Self::Value {
        self.0.value()
    }
}

pub(super) fn choice_items(choices: &[Choice]) -> Vec<ChoiceItem> {
    choices.iter().cloned().map(ChoiceItem).collect()
}

pub(super) fn choice_index(choices: &[Choice], value: Option<&SharedString>) -> Option<IndexPath> {
    let value = value?;
    choices
        .iter()
        .position(|choice| choice.value() == value)
        .map(IndexPath::new)
}

/// The retained state of one field's control.
enum FieldState {
    Text(Entity<InputState>),
    TextArea(Entity<TextareaState>),
    Checkbox(bool),
    Dropdown(Entity<SelectState<Vec<ChoiceItem>>>),
    Date(Entity<DatePickerState>),
}

impl FieldState {
    /// Whether this state can edit `control`; a field whose control changed
    /// kind gets fresh state.
    fn fits(&self, control: &Control) -> bool {
        matches!(
            (self, control),
            (
                Self::Text(_),
                Control::Text { .. } | Control::Password { .. }
            ) | (Self::TextArea(_), Control::TextArea { .. })
                | (Self::Checkbox(_), Control::Checkbox { .. })
                | (Self::Dropdown(_), Control::Dropdown { .. })
                | (Self::Date(_), Control::Date { .. })
        )
    }

    fn value(&self, cx: &App) -> FormValue {
        match self {
            Self::Text(input) => FormValue::Text(input.read(cx).value()),
            Self::TextArea(input) => FormValue::Text(input.read(cx).value()),
            Self::Checkbox(checked) => FormValue::Bool(*checked),
            Self::Dropdown(select) => select
                .read(cx)
                .selected_value()
                .cloned()
                .map(FormValue::Text)
                .unwrap_or(FormValue::Empty),
            Self::Date(picker) => date_value(picker.read(cx).date_time().start()),
        }
    }

    fn focus_handle(&self, cx: &App) -> Option<FocusHandle> {
        match self {
            Self::Text(input) => Some(input.focus_handle(cx)),
            Self::TextArea(input) => Some(input.focus_handle(cx)),
            Self::Dropdown(select) => Some(select.focus_handle(cx)),
            Self::Date(picker) => Some(picker.focus_handle(cx)),
            // The checkbox keeps its own focus handle; it is reached by Tab.
            Self::Checkbox(_) => None,
        }
    }
}

fn date_value(value: Option<chrono::NaiveDateTime>) -> FormValue {
    value
        .map(|value| FormValue::Text(value.date().format(DATE_FORMAT).to_string().into()))
        .unwrap_or(FormValue::Empty)
}

/// The fields of one form page.
#[derive(Default)]
pub(super) struct FormFields {
    fields: HashMap<SharedString, FieldState>,
    _subscriptions: HashMap<SharedString, Subscription>,
}

impl FormFields {
    /// The values of `form`'s fields, in the form's terms.
    pub(super) fn values(&self, form: &FormModel, cx: &App) -> FormValues {
        form.fields()
            .iter()
            .fold(FormValues::new(), |values, field| {
                let value = self
                    .fields
                    .get(field.id())
                    .map(|state| state.value(cx))
                    .unwrap_or_else(|| field.control().initial_value());
                values.with(field.id().clone(), value)
            })
    }

    pub(super) fn first_focus_handle(&self, form: &FormModel, cx: &App) -> Option<FocusHandle> {
        form.fields()
            .iter()
            .filter_map(|field| self.fields.get(field.id()))
            .find_map(|state| state.focus_handle(cx))
    }
}

impl LauncherWindow {
    /// Creates control state for fields seen for the first time, starting
    /// from each control's initial value.
    pub(super) fn ensure_form_fields(
        &mut self,
        entry: EntryId,
        form: &FormModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for field in form.fields() {
            let fits = self
                .forms
                .get(&entry)
                .and_then(|fields| fields.fields.get(field.id()))
                .is_some_and(|state| state.fits(field.control()));
            if fits {
                continue;
            }
            let (state, subscription) = self.new_field_state(entry, field, window, cx);
            let fields = self.forms.entry(entry).or_default();
            fields.fields.insert(field.id().clone(), state);
            match subscription {
                Some(subscription) => {
                    fields
                        ._subscriptions
                        .insert(field.id().clone(), subscription);
                }
                None => {
                    fields._subscriptions.remove(field.id());
                }
            }
        }
    }

    fn new_field_state(
        &mut self,
        entry: EntryId,
        field: &Field,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (FieldState, Option<Subscription>) {
        let id = field.id().clone();
        match field.control() {
            Control::Text { placeholder, value } | Control::Password { placeholder, value } => {
                let masked = matches!(field.control(), Control::Password { .. });
                let input = cx.new(|cx| {
                    let input = InputState::new(window, cx)
                        .masked(masked)
                        .default_value(value.clone());
                    match placeholder {
                        Some(placeholder) => input.placeholder(placeholder.clone()),
                        None => input,
                    }
                });
                let subscription =
                    cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = FormValue::Text(input.read(cx).value());
                            this.field_changed(entry, &id, value, window, cx);
                        }
                    });
                (FieldState::Text(input), Some(subscription))
            }
            Control::TextArea { placeholder, value } => {
                let input = cx.new(|cx| {
                    let input = TextareaState::new(window, cx)
                        .auto_grow(3, 8)
                        .default_value(value.clone());
                    match placeholder {
                        Some(placeholder) => input.placeholder(placeholder.clone()),
                        None => input,
                    }
                });
                let subscription =
                    cx.subscribe_in(&input, window, move |this, input, event, window, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = FormValue::Text(input.read(cx).value());
                            this.field_changed(entry, &id, value, window, cx);
                        }
                    });
                (FieldState::TextArea(input), Some(subscription))
            }
            Control::Checkbox { value, .. } => (FieldState::Checkbox(*value), None),
            Control::Dropdown { choices, value } => {
                let selected = choice_index(choices, value.as_ref());
                let items = choice_items(choices);
                let select = cx.new(|cx| SelectState::new(items, selected, window, cx));
                let subscription = cx.subscribe_in(
                    &select,
                    window,
                    move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        let SelectEvent::Confirm(value) = event;
                        let value = value
                            .clone()
                            .map(FormValue::Text)
                            .unwrap_or(FormValue::Empty);
                        this.field_changed(entry, &id, value, window, cx);
                    },
                );
                (FieldState::Dropdown(select), Some(subscription))
            }
            Control::Date { value } => {
                let date = value
                    .as_ref()
                    .and_then(|value| NaiveDate::parse_from_str(value, DATE_FORMAT).ok());
                let picker = cx.new(|cx| {
                    let mut picker = DatePickerState::new(window, cx);
                    if let Some(date) = date {
                        picker.set_date(date, window, cx);
                    }
                    picker
                });
                let subscription = cx.subscribe_in(
                    &picker,
                    window,
                    move |this, _, event: &DatePickerEvent, window, cx| {
                        let DatePickerEvent::Change(value) = event;
                        this.field_changed(entry, &id, date_value(value.start()), window, cx);
                    },
                );
                (FieldState::Date(picker), Some(subscription))
            }
        }
    }

    /// Reports an edit to the field's `on_change`, read from the model the
    /// page currently shows so the callback is never a stale one.
    fn field_changed(
        &mut self,
        entry: EntryId,
        id: &SharedString,
        value: FormValue,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.navigator.current().id() != entry {
            return;
        }
        let model = self.navigator.current().page().model(window, cx);
        let crate::model::PageModel::Form(form) = model else {
            return;
        };
        if let Some(on_change) = form
            .fields()
            .iter()
            .find(|field| field.id() == id)
            .and_then(Field::on_change)
        {
            on_change.call(value, window, cx);
        }
        cx.notify();
    }

    fn toggle_checkbox(
        &mut self,
        entry: EntryId,
        id: &SharedString,
        checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(FieldState::Checkbox(value)) = self
            .forms
            .get_mut(&entry)
            .and_then(|fields| fields.fields.get_mut(id))
        {
            *value = checked;
        }
        self.field_changed(entry, id, FormValue::Bool(checked), window, cx);
    }

    /// Draws the form. Tab and Shift-Tab move between fields even from a
    /// multi-line field, where they would otherwise indent: in a launcher
    /// form, Tab is navigation.
    pub(super) fn render_form(
        &self,
        entry: EntryId,
        form: &FormModel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fields = self.forms.get(&entry);
        div()
            .id("form")
            .size_full()
            .overflow_y_scroll()
            .capture_action(cx.listener(|_, _: &IndentInline, window, cx| {
                cx.stop_propagation();
                window.focus_next(cx);
            }))
            .capture_action(cx.listener(|_, _: &OutdentInline, window, cx| {
                cx.stop_propagation();
                window.focus_prev(cx);
            }))
            .child(
                v_flex()
                    .py_4()
                    .px_6()
                    .gap_4()
                    .children(form.fields().iter().map(|field| {
                        let state = fields.and_then(|fields| fields.fields.get(field.id()));
                        self.field_row(entry, field, state, cx)
                    })),
            )
            .into_any_element()
    }

    fn field_row(
        &self,
        entry: EntryId,
        field: &Field,
        state: Option<&FieldState>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let control = match (state, field.control()) {
            (Some(FieldState::Text(input)), _) => Input::new(input).into_any_element(),
            (Some(FieldState::TextArea(input)), _) => Textarea::new(input).into_any_element(),
            (Some(FieldState::Checkbox(checked)), Control::Checkbox { label, .. }) => {
                let id = field.id().clone();
                Checkbox::new(keyed_id("field", field.id().clone()))
                    .label(label.clone())
                    .accessibility_label(field.title().clone())
                    .checked(*checked)
                    .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                        this.toggle_checkbox(entry, &id, *checked, window, cx);
                    }))
                    .map(|checkbox| h_flex().h_8().child(checkbox))
                    .into_any_element()
            }
            (Some(FieldState::Dropdown(select)), _) => Select::new(select)
                .accessibility_label(field.title().clone())
                .into_any_element(),
            (Some(FieldState::Date(picker)), _) => DatePicker::new(picker).into_any_element(),
            // State is created before the first draw; a mismatch lasts one frame.
            _ => div().into_any_element(),
        };
        let theme = cx.theme();
        h_flex()
            .items_start()
            .gap_4()
            .child(
                // The label box is as tall as a control row, so the label
                // centers on the control's frame rather than its help text.
                h_flex()
                    .flex_none()
                    .w_1_4()
                    .h_8()
                    .justify_end()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(field.title().clone()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(control)
                    .when_some(field.error().cloned(), |this, error| {
                        this.child(div().text_xs().text_color(theme.danger).child(error))
                    })
                    .when_some(field.info().cloned(), |this, info| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(info),
                        )
                    }),
            )
            .into_any_element()
    }
}
