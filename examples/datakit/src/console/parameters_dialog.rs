//! Asking for the values of a statement's `$1` and `:name` placeholders.

use std::{collections::HashMap, rc::Rc};

use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{field, v_form},
    h_flex,
    input::{Input, InputState},
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div, rems,
};
use rust_i18n::t;

/// One field per placeholder; each value is SQL text.
pub struct ParametersDialog {
    fields: Vec<(SharedString, Entity<InputState>)>,
}

impl ParametersDialog {
    /// Ask for `names`, starting from the `remembered` values, then call
    /// `run` with every value.
    pub fn open(
        names: Vec<String>,
        remembered: &HashMap<String, String>,
        run: impl Fn(HashMap<String, String>, &mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) {
        let dialog = cx.new(|cx| Self {
            fields: names
                .into_iter()
                .map(|name| {
                    let value =
                        SharedString::from(remembered.get(&name).cloned().unwrap_or_default());
                    let input = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("NULL")
                            .default_value(value)
                    });
                    (SharedString::from(name), input)
                })
                .collect(),
        });
        let run = Rc::new(run);
        let width = rems(28.).to_pixels(window.rem_size());
        window.open_dialog(cx, {
            let dialog = dialog.clone();
            move |modal, _, _| {
                let run = run.clone();
                let values = dialog.clone();
                modal
                    .title(t!("parameters.title").to_string())
                    .w(width)
                    .child(dialog.clone())
                    .footer(
                        DialogFooter::new().child(
                            h_flex()
                                .gap_2()
                                .child(
                                    DialogClose::new().child(
                                        Button::new("cancel")
                                            .outline()
                                            .label(t!("common.cancel").to_string()),
                                    ),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("run")
                                            .primary()
                                            .label(t!("parameters.run").to_string()),
                                    ),
                                ),
                        ),
                    )
                    .on_ok(move |_, window, cx| {
                        let values: HashMap<String, String> = values
                            .read(cx)
                            .fields
                            .iter()
                            .map(|(name, input)| {
                                let value = input.read(cx).value().trim().to_string();
                                let value = if value.is_empty() {
                                    "NULL".to_string()
                                } else {
                                    value
                                };
                                (name.to_string(), value)
                            })
                            .collect();
                        run(values, window, cx);
                        true
                    })
            }
        });
        let first = dialog
            .read(cx)
            .fields
            .first()
            .map(|(_, input)| input.clone());
        if let Some(first) = first {
            first.update(cx, |input, cx| input.focus(window, cx));
        }
    }
}

impl Render for ParametersDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("parameters.description").to_string()),
            )
            .child(
                v_form().children(
                    self.fields
                        .iter()
                        .map(|(name, input)| field().label(name.clone()).child(Input::new(input))),
                ),
            )
    }
}
