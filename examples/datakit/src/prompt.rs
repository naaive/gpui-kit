//! Asking for one line of text in a dialog.

use std::rc::Rc;

use gpui_kit::component::{
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{DialogAction, DialogClose, DialogFooter},
    form::{field, v_form},
    h_flex,
    input::{Input, InputEvent, InputState},
};
use gpui_kit::{App, AppContext as _, ParentElement as _, SharedString, Styled as _, Window, rems};
use rust_i18n::t;

/// Ask for a value: a dialog titled `title` with one field, starting with
/// `initial` selected. `done` receives the trimmed text when the person
/// confirms with something other than the starting value.
pub fn prompt_text(
    title: SharedString,
    label: SharedString,
    initial: &str,
    confirm: SharedString,
    done: impl Fn(String, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let initial = initial.to_string();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
    let done = Rc::new(done);
    let width = rems(24.).to_pixels(window.rem_size());
    let submit = {
        let input = input.clone();
        let done = done.clone();
        Rc::new(move |window: &mut Window, cx: &mut App| {
            let value = input.read(cx).value().trim().to_string();
            if !value.is_empty() && value != initial {
                done(value, window, cx);
            }
        })
    };
    // Enter in the field confirms, like the button.
    let on_enter = submit.clone();
    window
        .subscribe(&input, cx, move |_, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                on_enter(window, cx);
                window.close_dialog(cx);
            }
        })
        .detach();
    window.open_dialog(cx, {
        let input = input.clone();
        move |dialog, _, _| {
            let submit = submit.clone();
            dialog
                .title(title.clone())
                .w(width)
                .child(v_form().child(field().label(label.clone()).child(Input::new(&input))))
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
                                DialogAction::new()
                                    .child(Button::new("confirm").primary().label(confirm.clone())),
                            ),
                    ),
                )
                .on_ok(move |_, window, cx| {
                    submit(window, cx);
                    true
                })
        }
    });
    input.update(cx, |input, cx| {
        input.focus(window, cx);
        input.select_all(window, cx);
    });
}
