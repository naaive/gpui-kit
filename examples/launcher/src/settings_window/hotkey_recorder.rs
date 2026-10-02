//! A field that records a hotkey by pressing it, as Raycast's settings do,
//! rather than asking for `ctrl-alt-c` to be typed.

use std::rc::Rc;

use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    component::{
        ActiveTheme as _, IconName, Sizable as _, button::Button, button::ButtonVariants as _,
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::ui::keycaps::keycaps;

/// Saves a recorded hotkey, or `None` to remove it; an error is shown under
/// the field and the previous hotkey stays.
pub type SaveHotkey = Rc<dyn Fn(Option<String>, &mut Window, &mut App) -> Result<(), String>>;

pub struct HotkeyRecorder {
    focus_handle: FocusHandle,
    /// The saved hotkey, as GPUI writes it (`ctrl-alt-c`).
    value: Option<String>,
    placeholder: SharedString,
    /// Whether a hotkey without Ctrl, Alt or the system key is refused, as a
    /// global one would fire while typing.
    requires_modifier: bool,
    error: Option<SharedString>,
    save: SaveHotkey,
}

impl HotkeyRecorder {
    pub fn new(value: Option<String>, save: SaveHotkey, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            value,
            placeholder: "Record Hotkey".into(),
            requires_modifier: true,
            error: None,
            save,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        cx.stop_propagation();
        match keystroke.key.as_str() {
            // A modifier alone is the start of a hotkey, not one.
            "control" | "ctrl" | "alt" | "shift" | "platform" | "cmd" | "super" | "win" | "fn" => {
                return;
            }
            "escape" if !modifiers.modified() => {
                window.blur(cx);
                return;
            }
            "backspace" | "delete" if !modifiers.modified() => {
                self.commit(None, window, cx);
                return;
            }
            _ => {}
        }
        let has_modifier = modifiers.control || modifiers.alt || modifiers.platform;
        let is_function_key = keystroke
            .key
            .strip_prefix('f')
            .is_some_and(|number| number.parse::<u8>().is_ok());
        if self.requires_modifier && !has_modifier && !is_function_key {
            self.error = Some("Hold Ctrl, Alt or the system key with the key.".into());
            cx.notify();
            return;
        }
        let bare = gpui_kit::Keystroke {
            key_char: None,
            ..keystroke.clone()
        };
        self.commit(Some(bare.unparse()), window, cx);
    }

    fn commit(&mut self, value: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        match (self.save)(value.clone(), window, cx) {
            Ok(()) => {
                self.value = value;
                self.error = None;
                window.blur(cx);
            }
            Err(error) => self.error = Some(error.into()),
        }
        cx.notify();
    }
}

impl Focusable for HotkeyRecorder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HotkeyRecorder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let recording = self.focus_handle.is_focused(window);
        let theme = cx.theme();
        let content = match (&self.value, recording) {
            (_, true) => div()
                .text_color(theme.muted_foreground)
                .child("Press a hotkey…")
                .into_any_element(),
            (Some(value), false) => {
                keycaps(value, cx).unwrap_or_else(|| div().child(value.clone()).into_any_element())
            }
            (None, false) => div()
                .text_color(theme.muted_foreground)
                .child(self.placeholder.clone())
                .into_any_element(),
        };
        v_flex()
            .gap_1()
            .items_end()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("hotkey-recorder")
                            .track_focus(&self.focus_handle)
                            .on_key_down(cx.listener(Self::on_key_down))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.error = None;
                                window.focus(&this.focus_handle, cx);
                                cx.notify();
                            }))
                            .h(px(28.))
                            .min_w(px(140.))
                            .px_2()
                            .justify_center()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(match recording {
                                true => theme.ring,
                                false => theme.input,
                            })
                            .bg(theme.background)
                            .text_sm()
                            .cursor_default()
                            .child(content),
                    )
                    .when(self.value.is_some() && !recording, |this| {
                        this.child(
                            Button::new("clear-hotkey")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Remove Hotkey")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.commit(None, window, cx);
                                })),
                        )
                    }),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(div().text_xs().text_color(theme.danger).child(error))
            })
    }
}
